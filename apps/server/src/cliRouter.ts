import { Router } from 'express';
import type { Request, Response } from 'express';
import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { dirname, join } from 'node:path';
import { homedir } from 'node:os';
import { CLI_ROUTER_STRATEGIES } from '@git-agent-harness/contracts';
import type { mutationSafety } from './mutationSafety.js';
import { requireOwner } from './authMiddleware.js';
import type {
  CliRouterSnapshot,
  CliRouterSettingsView,
  CliRouterAccount,
  CliRouterModel,
  CliRouterStrategy,
  CliRouterStoredSettings,
  CliRouterQuota,
  UpstreamAuthFile,
  UpstreamModel,
} from '@git-agent-harness/contracts';

// ---------------------------------------------------------------------------
// Configuration constants
// ---------------------------------------------------------------------------

/** Upstream HTTP calls abort after this many ms. */
const UPSTREAM_TIMEOUT_MS = 12_000;

/** Maximum bytes we will read from an upstream JSON response. */
const MAX_RESPONSE_BYTES = 512 * 1024; // 512 KiB

// ---------------------------------------------------------------------------
// Settings persistence (atomic, mode 0600)
// ---------------------------------------------------------------------------

function settingsPath(): string {
  return process.env.GAH_CLI_ROUTER_SETTINGS_PATH
    ?? join(homedir(), '.config', 'gah', 'cli-router.json');
}

/** Read persisted settings, returning null when unconfigured. */
export function readSettings(): CliRouterStoredSettings | null {
  let raw: string;
  try {
    raw = readFileSync(settingsPath(), 'utf8');
  } catch (err) {
    if (err && typeof err === 'object' && 'code' in err && err.code === 'ENOENT') return null;
    throw new Error('Failed to read CLI router settings file.');
  }
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    throw new Error('CLI router settings file is corrupted (invalid JSON).');
  }
  if (!data || typeof data !== 'object' || Array.isArray(data)) throw new Error('CLI router settings file is corrupted (not an object).');
  const d = data as Record<string, unknown>;
  if (typeof d.url !== 'string' || typeof d.apiKey !== 'string' || typeof d.managementKey !== 'string'
    || !validSecret(d.apiKey, true) || !validSecret(d.managementKey, true)) {
    throw new Error('CLI router settings file is corrupted (missing required fields).');
  }
  try { validateRouterUrl(d.url); } catch { throw new Error('CLI router settings file is corrupted (invalid URL).'); }
  return { url: d.url, apiKey: d.apiKey, managementKey: d.managementKey };
}

/** Keys are opaque secrets: bounded, no control chars. Stored files may hold blank (unconfigured) values. */
function validSecret(v: string, allowEmpty = false): boolean {
  return (allowEmpty || v.length > 0) && v.length <= 512 && !/[\x00-\x1F\x7F]/.test(v);
}

export function writeSettings(settings: CliRouterStoredSettings): void {
  const path = settingsPath();
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const tmp = path + `.tmp.${randomBytes(6).toString('hex')}`;
  // wx flags: open for writing, fails if file exists (no symlink attack on tmp)
  const fd = openSync(tmp, 'wx', 0o600);
  try {
    try {
      writeFileSync(fd, JSON.stringify(settings, null, 2) + '\n');
      fsyncSync(fd);
    } finally {
      closeSync(fd);
    }
    renameSync(tmp, path);
  } finally {
    rmSync(tmp, { force: true });
  }
  quotaCache.clear();
}

// ---------------------------------------------------------------------------
// In-memory quota cache (never persisted to disk)
// ---------------------------------------------------------------------------

interface QuotaCache {
  quotas: CliRouterQuota[];
  quotaError?: string;
  observedAt: string;
}
const quotaCache = new Map<string, QuotaCache>();
const cacheKey = (origin: string, file: { id: string; auth_index?: unknown }) => `${origin}\0${file.id}\0${String(file.auth_index ?? '')}`;

// ---------------------------------------------------------------------------
// URL validation
// ---------------------------------------------------------------------------

/** URL must be HTTPS origin only, or literal loopback HTTP. No creds, path, query, hash. */
export function validateRouterUrl(input: string): URL {
  if (typeof input !== 'string' || input.length > 2000 || /[\x00-\x1F\x7F]/.test(input)) {
    throw new Error('Invalid URL string.');
  }
  let url: URL;
  try { url = new URL(input); }
  catch { throw new Error('Invalid URL format.'); }
  if (!['http:', 'https:'].includes(url.protocol)) throw new Error('URL must use HTTP or HTTPS.');
  if (url.username || url.password) throw new Error('URL must not contain embedded credentials.');
  if (url.pathname !== '/' || url.search || url.hash) throw new Error('URL must be an origin only — no path, query, or hash.');
  if (url.protocol === 'http:') {
    const host = url.hostname.replace(/^\[|\]$/g, '');
    const isLoopback = host === '127.0.0.1' || host === '::1';
    if (!isLoopback) throw new Error('HTTP is only allowed for literal loopback addresses (127.0.0.1 or ::1). Use HTTPS for remote URLs.');
  }
  return url;
}

// ---------------------------------------------------------------------------
// Bounded upstream fetch (no redirects, timeout, size cap)
// ---------------------------------------------------------------------------

export interface UpstreamFetchOptions {
  url: string;
  headers?: Record<string, string>;
  method?: string;
  body?: string;
  /** Override for testing */
  fetchFn?: typeof globalThis.fetch;
}

// This is an owner-configured API client. Only the owner-only settings route
// can select its origin; callers of read/refresh routes cannot choose a target.
// Origin validation, disabled redirects, and server-built quota targets keep
// the selected router separate from arbitrary requests on behalf of readers.
export async function boundedUpstreamFetch(opts: UpstreamFetchOptions): Promise<{ status: number; body: unknown }> {
  const fetchFn = opts.fetchFn ?? globalThis.fetch;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), UPSTREAM_TIMEOUT_MS);
  try {
    const response = await fetchFn(opts.url, {
      method: opts.method ?? 'GET',
      headers: opts.headers,
      body: opts.body,
      signal: controller.signal,
      redirect: 'error',  // Never follow redirects
    });
    // Read response with size cap
    const chunks: Uint8Array[] = [];
    let totalBytes = 0;
    if (response.body) {
      const reader = response.body.getReader();
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        totalBytes += value.byteLength;
        if (totalBytes > MAX_RESPONSE_BYTES) {
          reader.cancel();
          throw new Error(`Upstream response exceeded ${MAX_RESPONSE_BYTES} bytes.`);
        }
        chunks.push(value);
      }
    }
    const text = new TextDecoder().decode(Buffer.concat(chunks));
    let body: unknown;
    try { body = JSON.parse(text); }
    catch { body = text; }
    return { status: response.status, body };
  } finally { clearTimeout(timer); }
}

// ---------------------------------------------------------------------------
// Upstream helpers
// ---------------------------------------------------------------------------

const boundedString = (v: unknown, max = 200): string | null =>
  typeof v === 'string' && v.length > 0 && v.length <= max && !/[\x00-\x1F\x7F]/.test(v) ? v : null;

async function fetchModels(baseUrl: string, apiKey: string, fetchFn?: typeof globalThis.fetch): Promise<CliRouterModel[]> {
  const result = await boundedUpstreamFetch({
    url: `${baseUrl}/v1/models`,
    headers: { 'Authorization': `Bearer ${apiKey}` },
    fetchFn,
  });
  if (result.status !== 200) throw new Error(`Upstream /v1/models returned ${result.status}`);
  const data = result.body as { data?: Array<Partial<UpstreamModel>> };
  if (!data?.data || !Array.isArray(data.data)) throw new Error('Invalid /v1/models response');
  const models: CliRouterModel[] = [];
  for (const m of data.data) {
    const id = boundedString(m?.id);
    if (id) models.push({ id, ownedBy: boundedString(m.owned_by) ?? 'unknown' });
  }
  return models;
}

async function fetchAuthFiles(baseUrl: string, managementKey: string, fetchFn?: typeof globalThis.fetch): Promise<UpstreamAuthFile[]> {
  const result = await boundedUpstreamFetch({
    url: `${baseUrl}/v0/management/auth-files`,
    headers: { 'Authorization': `Bearer ${managementKey}` },
    fetchFn,
  });
  if (result.status !== 200) throw new Error(`Upstream /v0/management/auth-files returned ${result.status}`);
  const data = result.body as { files?: unknown[] };
  if (!data?.files || !Array.isArray(data.files)) throw new Error('Invalid /v0/management/auth-files response');
  // Drop entries we cannot identify rather than inventing ids.
  return data.files.filter((f): f is UpstreamAuthFile =>
    !!f && typeof f === 'object' && boundedString((f as UpstreamAuthFile).id) !== null && boundedString((f as UpstreamAuthFile).name) !== null);
}

const providerOf = (file: UpstreamAuthFile): string =>
  boundedString(file.provider, 64) ?? boundedString(file.type, 64) ?? 'unknown';

/** Go zero time and unparseable values mean "no reset scheduled". */
function isoOrNull(v: unknown): string | null {
  if (typeof v !== 'string') return null;
  const t = Date.parse(v);
  return Number.isFinite(t) && t > Date.UTC(2000, 0, 1) ? new Date(t).toISOString() : null;
}

function mapAuthFileToAccount(file: UpstreamAuthFile, origin: string): CliRouterAccount {
  const cached = quotaCache.get(cacheKey(origin, file));
  return {
    id: file.id,
    name: file.name,
    provider: providerOf(file),
    label: boundedString(file.label) ?? file.name,
    disabled: file.disabled === true,
    unavailable: file.unavailable === true,
    resetAt: isoOrNull(file.next_retry_after),
    quotas: cached?.quotas ?? [],
    ...(cached?.quotaError ? { quotaError: cached.quotaError } : {}),
  };
}

async function fetchRoutingStrategy(baseUrl: string, managementKey: string, fetchFn?: typeof globalThis.fetch): Promise<{ strategy: CliRouterStrategy; sessionAffinity: boolean }> {
  const stratResult = await boundedUpstreamFetch({
    url: `${baseUrl}/v0/management/config`,
    headers: { 'Authorization': `Bearer ${managementKey}` },
    fetchFn,
  });
  if (stratResult.status !== 200) throw new Error('Upstream routing config request failed');
  if (!stratResult.body || typeof stratResult.body !== 'object') throw new Error('Invalid config format');

  const config = stratResult.body as Record<string, unknown>;
  const routing = config.routing as Record<string, unknown> | undefined;

  if (!routing) throw new Error('Missing routing config section');

  if (typeof routing.strategy !== 'string' || !(CLI_ROUTER_STRATEGIES as readonly string[]).includes(routing.strategy)) {
    throw new Error('Unknown or missing routing strategy');
  }

  return {
    strategy: routing.strategy as CliRouterStrategy,
    sessionAffinity: routing['session-affinity'] === true
  };
}

// ---------------------------------------------------------------------------
// Quota refresh per provider
// ---------------------------------------------------------------------------

interface QuotaRefreshDeps {
  fetchFn?: typeof globalThis.fetch;
  baseUrl: string;
  managementKey: string;
}

/** Message is safe to show to users; anything else becomes a generic failure. */
class QuotaFailure extends Error {}

type ProviderCall = { method: 'GET' | 'POST'; url: string; header: Record<string, string>; data?: string };

/** Percent in [0,100], else null (unknown) — never clamp garbage into a plausible value. */
const percent = (v: unknown): number | null =>
  typeof v === 'number' && Number.isFinite(v) && v >= 0 && v <= 100 ? Math.round(v) : null;

/** Server-built provider call through the proxy; the proxy swaps $TOKEN$ for the real credential. */
async function proxyCall(account: UpstreamAuthFile, call: ProviderCall, deps: QuotaRefreshDeps): Promise<unknown> {
  if (typeof account.auth_index !== 'string' && typeof account.auth_index !== 'number') {
    throw new QuotaFailure('Account has no auth index');
  }
  const result = await boundedUpstreamFetch({
    url: `${deps.baseUrl}/v0/management/api-call`,
    method: 'POST',
    headers: { 'Authorization': `Bearer ${deps.managementKey}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({
      auth_index: account.auth_index,
      method: call.method,
      url: call.url,
      header: { ...call.header, 'Authorization': 'Bearer $TOKEN$' },
      ...(call.data !== undefined ? { data: call.data } : {}),
    }),
    fetchFn: deps.fetchFn,
  });
  if (result.status !== 200) throw new Error('proxy status');
  const wrapper = result.body as { status_code?: number; body?: unknown } | null;
  if (wrapper?.status_code !== 200) throw new Error('provider status');
  return typeof wrapper.body === 'string' ? JSON.parse(wrapper.body) : wrapper.body;
}

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);

async function refreshAntigravityQuota(account: UpstreamAuthFile, deps: QuotaRefreshDeps): Promise<CliRouterQuota[]> {
  const project = boundedString(account.project_id, 200);
  if (!project) throw new QuotaFailure('Account has no project id');
  const body = await proxyCall(account, {
    method: 'POST',
    url: 'https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary',
    header: { 'Content-Type': 'application/json', 'User-Agent': 'antigravity/cli/1.0.13 (aidev_client; os_type=darwin; arch=arm64)' },
    data: JSON.stringify({ project }),
  }, deps);
  const groups = isObj(body) ? body.groups : undefined;
  if (!Array.isArray(groups)) throw new Error('shape');
  const observedAt = new Date().toISOString();
  const quotas: CliRouterQuota[] = [];
  for (const g of groups) {
    if (!isObj(g) || !Array.isArray(g.buckets)) continue;
    for (const b of g.buckets) {
      if (!isObj(b)) continue;
      const name = boundedString(b.displayName, 80) ?? 'Unknown';
      const window = boundedString(b.window, 40);
      quotas.push({
        label: [boundedString(g.displayName, 80), window ? `${name} (${window})` : name].filter(Boolean).join(' · '),
        remainingPercent: typeof b.remainingFraction === 'number' ? percent(b.remainingFraction * 100) : null,
        resetAt: isoOrNull(b.resetTime),
        observedAt,
      });
    }
  }
  return quotas;
}

async function refreshClaudeQuota(account: UpstreamAuthFile, deps: QuotaRefreshDeps): Promise<CliRouterQuota[]> {
  const body = await proxyCall(account, {
    method: 'GET',
    url: 'https://api.anthropic.com/api/oauth/usage',
    header: { 'anthropic-beta': 'oauth-2025-04-20' },
  }, deps);
  if (!isObj(body)) throw new Error('shape');
  const observedAt = new Date().toISOString();
  const quotas: CliRouterQuota[] = [];
  // Windows are {utilization 0..100, resets_at}; other keys (extra_usage etc.) lack utilization.
  for (const [key, w] of Object.entries(body)) {
    if (!isObj(w) || !('utilization' in w) || key.length > 80) continue;
    quotas.push({
      label: key,
      remainingPercent: typeof w.utilization === 'number' ? percent(100 - w.utilization) : null,
      resetAt: isoOrNull(w.resets_at),
      observedAt,
    });
  }
  return quotas;
}

async function refreshCodexQuota(account: UpstreamAuthFile, deps: QuotaRefreshDeps): Promise<CliRouterQuota[]> {
  const header: Record<string, string> = {};
  const accountId = isObj(account.id_token) ? boundedString(account.id_token.chatgpt_account_id, 128) : null;
  if (accountId) header['Chatgpt-Account-Id'] = accountId;
  const body = await proxyCall(account, { method: 'GET', url: 'https://chatgpt.com/backend-api/wham/usage', header }, deps);
  const rateLimit = isObj(body) ? body.rate_limit : undefined;
  if (!isObj(rateLimit)) throw new Error('shape');
  const observedAt = new Date().toISOString();
  const quotas: CliRouterQuota[] = [];
  for (const [key, w] of Object.entries(rateLimit)) {
    if (!isObj(w) || !('used_percent' in w) || key.length > 80) continue;
    const resetSec = typeof w.reset_at === 'number' && Number.isFinite(w.reset_at) ? w.reset_at * 1000 : NaN;
    quotas.push({
      label: key,
      remainingPercent: typeof w.used_percent === 'number' ? percent(100 - w.used_percent) : null,
      resetAt: Number.isFinite(resetSec) && resetSec > 0 && resetSec < 8.64e15 ? new Date(resetSec).toISOString() : null,
      observedAt,
    });
  }
  return quotas;
}

const QUOTA_PROVIDERS: Record<string, (a: UpstreamAuthFile, d: QuotaRefreshDeps) => Promise<CliRouterQuota[]>> = {
  antigravity: refreshAntigravityQuota,
  claude: refreshClaudeQuota,
  codex: refreshCodexQuota,
};

async function refreshAccountQuota(account: UpstreamAuthFile, deps: QuotaRefreshDeps): Promise<QuotaCache> {
  const observedAt = new Date().toISOString();
  const name = providerOf(account).toLowerCase();
  const refresh = Object.hasOwn(QUOTA_PROVIDERS, name) ? QUOTA_PROVIDERS[name] : undefined;
  // Failures never imply zero remaining, and never echo upstream text (it can embed tokens).
  if (!refresh) {
    return { quotas: [], quotaError: 'Quota refresh is not supported for this provider', observedAt };
  }
  try {
    return { quotas: await refresh(account, deps), observedAt };
  } catch (err) {
    return { quotas: [], quotaError: err instanceof QuotaFailure ? err.message : 'Failed to refresh quota from provider', observedAt };
  }
}

// ---------------------------------------------------------------------------
// Router factory
// ---------------------------------------------------------------------------

export interface CliRouterDeps {
  fetchFn?: typeof globalThis.fetch;
  readSettingsFn?: typeof readSettings;
  writeSettingsFn?: typeof writeSettings;
}

export function cliRouterRouter(
  mutation: ReturnType<typeof mutationSafety>,
  deps: CliRouterDeps = {}
): Router {
  const router = Router();
  const doFetch = deps.fetchFn;
  const doRead = deps.readSettingsFn ?? readSettings;
  const doWrite = deps.writeSettingsFn ?? writeSettings;

  router.use((_req, res, next) => { res.setHeader('Cache-Control', 'no-store'); next(); });

  /** Read stored settings; a corrupt file is a 500 and is never overwritten or replaced by defaults. */
  const loadStored = (res: Response): { stored: CliRouterStoredSettings | null } | null => {
    try { return { stored: doRead() }; }
    catch { res.status(500).json({ error: 'config_corrupted', message: 'CLI router settings are unreadable or corrupted.' }); return null; }
  };

  // -----------------------------------------------------------------------
  // GET /api/cli-router — read-only snapshot
  // -----------------------------------------------------------------------
  router.get('/', async (_req: Request, res: Response) => {
    const loaded = loadStored(res);
    if (!loaded) return;
    const stored = loaded.stored;
    const settingsView: CliRouterSettingsView = {
      url: stored?.url ?? null,
      hasApiKey: !!stored?.apiKey,
      hasManagementKey: !!stored?.managementKey,
    };

    if (!stored?.url || !stored?.apiKey || !stored?.managementKey) {
      const snapshot: CliRouterSnapshot = {
        settings: settingsView,
        status: 'unconfigured',
        strategy: 'round-robin',
        sessionAffinity: false,
        accounts: [],
        models: [],
      };
      return res.json(snapshot);
    }

    const baseUrl = stored.url.replace(/\/$/, '');
    try {
      const [authFiles, models, routing] = await Promise.all([
        fetchAuthFiles(baseUrl, stored.managementKey, doFetch),
        fetchModels(baseUrl, stored.apiKey, doFetch),
        fetchRoutingStrategy(baseUrl, stored.managementKey, doFetch),
      ]);
      const snapshot: CliRouterSnapshot = {
        settings: settingsView,
        status: 'connected',
        strategy: routing.strategy,
        sessionAffinity: routing.sessionAffinity,
        accounts: authFiles.map(f => mapAuthFileToAccount(f, baseUrl)),
        models,
      };
      return res.json(snapshot);
    } catch {
      // Config read failed: report unavailable, never default values as if live.
      const snapshot: CliRouterSnapshot = {
        settings: settingsView,
        status: 'unavailable',
        strategy: 'round-robin',
        sessionAffinity: false,
        accounts: [],
        models: [],
      };
      return res.json(snapshot);
    }
  });

  // -----------------------------------------------------------------------
  // PUT /api/cli-router/settings — configure connection
  // -----------------------------------------------------------------------
  router.put('/settings', requireOwner, mutation('cli_router.configure'), async (req: Request, res: Response) => {
    const body = req.body;
    // Validate allowed keys
    const allowed = ['url', 'apiKey', 'managementKey'];
    if (!isObj(body) || Object.keys(body).some(k => !allowed.includes(k))) {
      return res.status(400).json({ error: 'invalid_request', message: 'Only url, apiKey, and managementKey are allowed.' });
    }
    for (const k of ['apiKey', 'managementKey']) {
      if (body[k] !== undefined && (typeof body[k] !== 'string' || (body[k] !== '' && !validSecret(body[k])))) {
        return res.status(400).json({ error: 'invalid_key', message: `${k} must be a string up to 512 characters without control characters.` });
      }
    }
    if (typeof body.url !== 'string' || !body.url) {
      return res.status(400).json({ error: 'invalid_url', message: 'URL is required.' });
    }

    let validatedUrl: URL;
    try { validatedUrl = validateRouterUrl(body.url); }
    catch (err) { return res.status(400).json({ error: 'invalid_url', message: err instanceof Error ? err.message : 'Invalid URL.' }); }

    const loaded = loadStored(res);
    if (!loaded) return;
    const existing = loaded.stored;
    const apiKey = typeof body.apiKey === 'string' && body.apiKey ? body.apiKey : existing?.apiKey;
    const managementKey = typeof body.managementKey === 'string' && body.managementKey ? body.managementKey : existing?.managementKey;

    // Initial connect requires both keys
    if (!apiKey || !managementKey) {
      return res.status(400).json({ error: 'keys_required', message: 'Initial configuration requires both apiKey and managementKey.' });
    }

    const baseUrl = validatedUrl.origin;

    // Test upstream connectivity before saving
    try {
      await fetchModels(baseUrl, apiKey, doFetch);
    } catch (err) {
      return res.status(502).json({ error: 'upstream_models_failed', message: 'Failed to communicate with the upstream models endpoint.' });
    }
    try {
      await fetchAuthFiles(baseUrl, managementKey, doFetch);
    } catch (err) {
      return res.status(502).json({ error: 'upstream_management_failed', message: 'Failed to communicate with the upstream auth-files endpoint.' });
    }

    try { doWrite({ url: baseUrl, apiKey, managementKey }); }
    catch { return res.status(500).json({ error: 'settings_write_failed', message: 'Failed to save CLI router settings.' }); }
    quotaCache.clear();
    return res.json({ success: true });
  });

  // -----------------------------------------------------------------------
  // POST /api/cli-router/routing — strategy/affinity
  // -----------------------------------------------------------------------
  router.post('/routing', requireOwner, mutation('cli_router.routing'), async (req: Request, res: Response) => {
    const body = req.body;
    const allowed = ['strategy', 'sessionAffinity'];
    if (!isObj(body) || Object.keys(body).some(k => !allowed.includes(k))) {
      return res.status(400).json({ error: 'invalid_request', message: 'Only strategy and sessionAffinity are allowed.' });
    }
    if (!(CLI_ROUTER_STRATEGIES as readonly string[]).includes(body.strategy as string)) {
      return res.status(400).json({ error: 'invalid_strategy', message: `Strategy must be one of: ${CLI_ROUTER_STRATEGIES.join(', ')}` });
    }
    if (typeof body.sessionAffinity !== 'boolean') {
      return res.status(400).json({ error: 'invalid_session_affinity', message: 'sessionAffinity must be a boolean.' });
    }
    const loaded = loadStored(res);
    if (!loaded) return;
    const stored = loaded.stored;
    if (!stored?.url || !stored?.managementKey) {
      return res.status(400).json({ error: 'unconfigured', message: 'Configure the CLI router connection first.' });
    }
    const baseUrl = stored.url.replace(/\/$/, '');
    const put = (path: string, method: string, value: unknown) => boundedUpstreamFetch({
      url: `${baseUrl}${path}`,
      method,
      headers: { 'Authorization': `Bearer ${stored.managementKey}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(value),
      fetchFn: doFetch,
    }).then(r => r.status >= 200 && r.status < 300, () => false);

    // Two upstream writes are not atomic: nothing applied if the first fails; partial if only the second does. Never replayed.
    if (!await put('/v0/management/routing/strategy', 'PUT', { value: body.strategy })) {
      return res.status(502).json({ error: 'routing_update_failed', message: 'The upstream router rejected or did not answer the routing update.' });
    }
    if (!await put('/v8/management/config/routing/session-affinity', 'PATCH', body.sessionAffinity)) {
      return res.status(502).json({
        error: 'routing_update_partial_failure',
        message: 'Strategy may have changed but session affinity did not. Refresh the snapshot to see the current state.',
      });
    }
    return res.json({ success: true });
  });

  // -----------------------------------------------------------------------
  // POST /api/cli-router/accounts/status — enable/disable
  // -----------------------------------------------------------------------
  router.post('/accounts/status', requireOwner, mutation('cli_router.account'), async (req: Request, res: Response) => {
    const body = req.body;
    const allowed = ['id', 'disabled'];
    if (!isObj(body) || Object.keys(body).some(k => !allowed.includes(k))) {
      return res.status(400).json({ error: 'invalid_request', message: 'Only id and disabled are allowed.' });
    }
    if (!boundedString(body.id)) {
      return res.status(400).json({ error: 'invalid_id', message: 'Account id is required.' });
    }
    if (typeof body.disabled !== 'boolean') {
      return res.status(400).json({ error: 'invalid_disabled', message: 'disabled must be a boolean.' });
    }
    const loaded = loadStored(res);
    if (!loaded) return;
    const stored = loaded.stored;
    if (!stored?.url || !stored?.managementKey) {
      return res.status(400).json({ error: 'unconfigured', message: 'Configure the CLI router connection first.' });
    }
    const baseUrl = stored.url.replace(/\/$/, '');

    // Resolve account name from fresh auth-files list
    let authFiles: UpstreamAuthFile[];
    try { authFiles = await fetchAuthFiles(baseUrl, stored.managementKey, doFetch); }
    catch { return res.status(502).json({ error: 'upstream_unavailable', message: 'Cannot reach upstream auth-files.' }); }

    const account = authFiles.find(f => f.id === body.id);
    if (!account) {
      return res.status(404).json({ error: 'account_not_found', message: 'Account not found.' });
    }

    try {
      const updateRes = await boundedUpstreamFetch({
        url: `${baseUrl}/v0/management/auth-files/status`,
        method: 'PATCH',
        headers: { 'Authorization': `Bearer ${stored.managementKey}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: account.name, disabled: body.disabled }),
        fetchFn: doFetch,
      });
      if (updateRes.status < 200 || updateRes.status >= 300) {
        return res.status(502).json({ error: 'account_status_failed', message: 'Upstream returned an error while updating account status.' });
      }
      return res.json({ success: true });
    } catch (err) {
      return res.status(502).json({ error: 'account_status_failed', message: 'Failed to communicate with the upstream router.' });
    }
  });

  // -----------------------------------------------------------------------
  // POST /api/cli-router/accounts/refresh — quota refresh
  // -----------------------------------------------------------------------
  router.post('/accounts/refresh', requireOwner, mutation('cli_router.refresh'), async (req: Request, res: Response) => {
    const body = req.body;
    const allowed = ['id'];
    if (!isObj(body) || Object.keys(body).some(k => !allowed.includes(k))) {
      return res.status(400).json({ error: 'invalid_request', message: 'Only id is allowed.' });
    }
    if (!boundedString(body.id)) {
      return res.status(400).json({ error: 'invalid_id', message: 'Account id is required.' });
    }
    const loaded = loadStored(res);
    if (!loaded) return;
    const stored = loaded.stored;
    if (!stored?.url || !stored?.managementKey) {
      return res.status(400).json({ error: 'unconfigured', message: 'Configure the CLI router connection first.' });
    }
    const baseUrl = stored.url.replace(/\/$/, '');

    // Fetch fresh auth files to validate the account exists
    let authFiles: UpstreamAuthFile[];
    try { authFiles = await fetchAuthFiles(baseUrl, stored.managementKey, doFetch); }
    catch { return res.status(502).json({ error: 'upstream_unavailable', message: 'Cannot reach upstream auth-files.' }); }

    const account = authFiles.find(f => f.id === body.id);
    if (!account) {
      return res.status(404).json({ error: 'account_not_found', message: 'Account not found.' });
    }

    const cache = await refreshAccountQuota(account, { baseUrl, managementKey: stored.managementKey, fetchFn: doFetch });
    quotaCache.set(cacheKey(baseUrl, account), cache);

    // Return full updated snapshot
    try {
      const [models, routing] = await Promise.all([
        fetchModels(baseUrl, stored.apiKey, doFetch),
        fetchRoutingStrategy(baseUrl, stored.managementKey, doFetch),
      ]);
      const snapshot: CliRouterSnapshot = {
        settings: { url: stored.url, hasApiKey: true, hasManagementKey: true },
        status: 'connected',
        strategy: routing.strategy,
        sessionAffinity: routing.sessionAffinity,
        accounts: authFiles.map(f => mapAuthFileToAccount(f, baseUrl)),
        models,
      };
      return res.json(snapshot);
    } catch {
      // Fall back to partial response with updated quotas
      const snapshot: CliRouterSnapshot = {
        settings: { url: stored.url, hasApiKey: true, hasManagementKey: true },
        status: 'unavailable',
        strategy: 'round-robin',
        sessionAffinity: false,
        accounts: authFiles.map(f => mapAuthFileToAccount(f, baseUrl)),
        models: [],
      };
      return res.json(snapshot);
    }
  });

  return router;
}
