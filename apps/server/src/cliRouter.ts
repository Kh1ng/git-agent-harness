import { Router } from 'express';
import type { Request, Response } from 'express';
import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync, writeFileSync, chmodSync, renameSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { homedir } from 'node:os';
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
  CLI_ROUTER_STRATEGIES,
} from '@git-agent-harness/contracts';

// ---------------------------------------------------------------------------
// Configuration constants
// ---------------------------------------------------------------------------

/** Upstream HTTP calls abort after this many ms. */
const UPSTREAM_TIMEOUT_MS = 12_000;

/** Maximum bytes we will read from an upstream JSON response. */
const MAX_RESPONSE_BYTES = 512 * 1024; // 512 KiB

/** Maximum JSON payload bytes from the client. */
const MAX_REQUEST_BODY_BYTES = 8 * 1024;

// ---------------------------------------------------------------------------
// Settings persistence (atomic, mode 0600)
// ---------------------------------------------------------------------------

function settingsPath(): string {
  return process.env.GAH_CLI_ROUTER_SETTINGS_PATH
    ?? join(homedir(), '.config', 'gah', 'cli-router.json');
}

/** Read persisted settings, returning null when unconfigured. */
export function readSettings(): CliRouterStoredSettings | null {
  try {
    const raw = readFileSync(settingsPath(), 'utf8');
    const data = JSON.parse(raw);
    if (!data || typeof data.url !== 'string' || typeof data.apiKey !== 'string' || typeof data.managementKey !== 'string') return null;
    return { url: data.url, apiKey: data.apiKey, managementKey: data.managementKey };
  } catch {
    return null;
  }
}

/** Atomic write with mode 0600. */
function writeSettings(settings: CliRouterStoredSettings): void {
  const path = settingsPath();
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const tmp = path + `.tmp.${process.pid}`;
  const fd = openSync(tmp, 'w', 0o600);
  try {
    writeFileSync(fd, JSON.stringify(settings, null, 2) + '\n');
    fsyncSync(fd);
  } finally { closeSync(fd); }
  chmodSync(tmp, 0o600);
  renameSync(tmp, path);
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

// ---------------------------------------------------------------------------
// URL validation
// ---------------------------------------------------------------------------

/** URL must be HTTPS origin only, or literal loopback HTTP. No creds, path, query, hash. */
export function validateRouterUrl(input: string): URL {
  let url: URL;
  try { url = new URL(input); }
  catch { throw new Error('Invalid URL format.'); }
  if (!['http:', 'https:'].includes(url.protocol)) throw new Error('URL must use HTTP or HTTPS.');
  if (url.username || url.password) throw new Error('URL must not contain embedded credentials.');
  if (url.pathname !== '/' || url.search || url.hash) throw new Error('URL must be an origin only — no path, query, or hash.');
  if (url.protocol === 'http:') {
    const host = url.hostname.replace(/^\[|\]$/g, '');
    const isLoopback = host === 'localhost' || host === '127.0.0.1' || host === '::1';
    if (!isLoopback) throw new Error('HTTP is only allowed for localhost/loopback addresses. Use HTTPS for remote URLs.');
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

async function fetchModels(baseUrl: string, apiKey: string, fetchFn?: typeof globalThis.fetch): Promise<CliRouterModel[]> {
  const result = await boundedUpstreamFetch({
    url: `${baseUrl}/v1/models`,
    headers: { 'Authorization': `Bearer ${apiKey}` },
    fetchFn,
  });
  if (result.status !== 200) throw new Error(`Upstream /v1/models returned ${result.status}`);
  const data = result.body as { data?: UpstreamModel[] };
  if (!data?.data || !Array.isArray(data.data)) throw new Error('Invalid /v1/models response');
  return data.data.map(m => ({ id: m.id, ownedBy: m.owned_by }));
}

async function fetchAuthFiles(baseUrl: string, managementKey: string, fetchFn?: typeof globalThis.fetch): Promise<UpstreamAuthFile[]> {
  const result = await boundedUpstreamFetch({
    url: `${baseUrl}/v0/management/auth-files`,
    headers: { 'Authorization': `Bearer ${managementKey}` },
    fetchFn,
  });
  if (result.status !== 200) throw new Error(`Upstream /v0/management/auth-files returned ${result.status}`);
  const data = result.body as { files?: UpstreamAuthFile[] };
  if (!data?.files || !Array.isArray(data.files)) throw new Error('Invalid /v0/management/auth-files response');
  return data.files;
}

function mapAuthFileToAccount(file: UpstreamAuthFile): CliRouterAccount {
  const cached = quotaCache.get(file.id);
  return {
    id: file.id,
    name: file.name,
    provider: file.provider,
    label: file.label,
    disabled: file.disabled,
    unavailable: file.unavailable,
    resetAt: file.next_retry_after,
    quotas: cached?.quotas ?? [],
    ...(cached?.quotaError ? { quotaError: cached.quotaError } : {}),
  };
}

async function fetchRoutingStrategy(baseUrl: string, managementKey: string, fetchFn?: typeof globalThis.fetch): Promise<{ strategy: CliRouterStrategy; sessionAffinity: boolean }> {
  // Fetch strategy
  const stratResult = await boundedUpstreamFetch({
    url: `${baseUrl}/v0/management/config`,
    headers: { 'Authorization': `Bearer ${managementKey}` },
    fetchFn,
  });
  let strategy: CliRouterStrategy = 'round-robin';
  let sessionAffinity = false;
  if (stratResult.status === 200 && stratResult.body && typeof stratResult.body === 'object') {
    const config = stratResult.body as Record<string, unknown>;
    const routing = config.routing as Record<string, unknown> | undefined;
    if (routing) {
      if (typeof routing.strategy === 'string' && ['round-robin', 'fill-first', 'weighted-round-robin'].includes(routing.strategy)) {
        strategy = routing.strategy as CliRouterStrategy;
      }
      if (typeof routing['session-affinity'] === 'boolean') {
        sessionAffinity = routing['session-affinity'];
      }
    }
  }
  return { strategy, sessionAffinity };
}

// ---------------------------------------------------------------------------
// Quota refresh per provider
// ---------------------------------------------------------------------------

interface QuotaRefreshDeps {
  fetchFn?: typeof globalThis.fetch;
  baseUrl: string;
  managementKey: string;
}

async function refreshAntigravityQuota(account: UpstreamAuthFile, deps: QuotaRefreshDeps): Promise<CliRouterQuota[]> {
  if (!account.project_id) throw new Error('Antigravity account missing project_id');
  // Use upstream api-call endpoint so no actual provider token leaves the proxy
  const result = await boundedUpstreamFetch({
    url: `${deps.baseUrl}/v0/management/api-call`,
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${deps.managementKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      auth_index: account.auth_index,
      method: 'POST',
      url: 'https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary',
      headers: { 'Content-Type': 'application/json', 'User-Agent': 'antigravity/cli/1.0.13 (aidev_client; os_type=darwin; arch=arm64)' },
      body: JSON.stringify({ project: account.project_id }),
    }),
    fetchFn: deps.fetchFn,
  });
  if (result.status !== 200) throw new Error(`Quota proxy returned ${result.status}`);
  const wrapper = result.body as { status_code?: number; body?: unknown };
  if (wrapper?.status_code !== 200) throw new Error(`Upstream quota returned ${wrapper?.status_code ?? 'unknown'}`);
  const body = typeof wrapper.body === 'string' ? JSON.parse(wrapper.body) : wrapper.body;
  const groups = (body as { groups?: Array<{ buckets?: Array<{ displayName?: string; window?: string; remainingFraction?: number; resetTime?: string }> }> })?.groups;
  if (!Array.isArray(groups)) return [];
  const quotas: CliRouterQuota[] = [];
  for (const g of groups) {
    if (Array.isArray(g.buckets)) {
      for (const b of g.buckets) {
        quotas.push({
          label: typeof b.displayName === "string" ? b.displayName : "Unknown",
          remainingPercent: typeof b.remainingFraction === "number" ? Math.round(b.remainingFraction * 100) : null,
          resetAt: typeof b.resetTime === "string" ? b.resetTime : null,
        });
      }
    }
  }
  return quotas;
}

async function refreshClaudeQuota(account: UpstreamAuthFile, deps: QuotaRefreshDeps): Promise<CliRouterQuota[]> {
  const result = await boundedUpstreamFetch({
    url: `${deps.baseUrl}/v0/management/api-call`,
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${deps.managementKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      auth_index: account.auth_index,
      method: 'GET',
      url: 'https://api.anthropic.com/api/oauth/usage',
      headers: { 'anthropic-beta': 'oauth-2025-04-20' },
    }),
    fetchFn: deps.fetchFn,
  });
  if (result.status !== 200) throw new Error(`Quota proxy returned ${result.status}`);
  const wrapper = result.body as { status_code?: number; body?: unknown };
  if (wrapper?.status_code !== 200) throw new Error(`Upstream quota returned ${wrapper?.status_code ?? 'unknown'}`);
  const body = typeof wrapper.body === 'string' ? JSON.parse(wrapper.body) : wrapper.body;
  if (!body || typeof body !== 'object') return [];
  const quotas: CliRouterQuota[] = [];
  for (const [key, value] of Object.entries(body as Record<string, unknown>)) {
    if (value && typeof value === 'object') {
      const window = value as { utilization?: number; resets_at?: string };
      if (typeof window.utilization === 'number') {
        quotas.push({
          label: key,
          remainingPercent: Math.round((1 - window.utilization) * 100),
          resetAt: typeof window.resets_at === 'string' ? window.resets_at : null,
        });
      }
    }
  }
  return quotas;
}

async function refreshCodexQuota(account: UpstreamAuthFile, deps: QuotaRefreshDeps): Promise<CliRouterQuota[]> {
  const result = await boundedUpstreamFetch({
    url: `${deps.baseUrl}/v0/management/api-call`,
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${deps.managementKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      auth_index: account.auth_index,
      method: 'GET',
      url: 'https://chatgpt.com/backend-api/wham/usage',
      headers: {},
    }),
    fetchFn: deps.fetchFn,
  });
  if (result.status !== 200) throw new Error(`Quota proxy returned ${result.status}`);
  const wrapper = result.body as { status_code?: number; body?: unknown };
  if (wrapper?.status_code !== 200) throw new Error(`Upstream quota returned ${wrapper?.status_code ?? 'unknown'}`);
  const body = typeof wrapper.body === 'string' ? JSON.parse(wrapper.body) : wrapper.body;
  if (!body || typeof body !== 'object') return [];
  const quotas: CliRouterQuota[] = [];
  const rateLimit = body as { rate_limit?: Record<string, unknown> };
  if (rateLimit.rate_limit) {
    for (const [key, value] of Object.entries(rateLimit.rate_limit)) {
      if (value && typeof value === 'object') {
        const window = value as { used_percent?: number; reset_at?: string };
        if (typeof window.used_percent === 'number') {
          quotas.push({
            label: key,
            remainingPercent: Math.round(100 - window.used_percent),
            resetAt: typeof window.reset_at === 'string' ? window.reset_at : null,
          });
        }
      }
    }
  }
  return quotas;
}

async function refreshAccountQuota(account: UpstreamAuthFile, deps: QuotaRefreshDeps): Promise<QuotaCache> {
  const provider = account.provider.toLowerCase();
  try {
    let quotas: CliRouterQuota[];
    if (provider.includes('antigravity') || provider.includes('google') || provider.includes('gemini')) {
      quotas = await refreshAntigravityQuota(account, deps);
    } else if (provider.includes('claude') || provider.includes('anthropic')) {
      quotas = await refreshClaudeQuota(account, deps);
    } else if (provider.includes('codex') || provider.includes('openai') || provider.includes('chatgpt')) {
      quotas = await refreshCodexQuota(account, deps);
    } else {
      return { quotas: [], quotaError: `Unsupported provider for quota refresh: ${account.provider}`, observedAt: new Date().toISOString() };
    }
    return { quotas, observedAt: new Date().toISOString() };
  } catch (err) {
    // Never imply zero remaining on failed/absent data
    return { quotas: [], quotaError: err instanceof Error ? err.message : String(err), observedAt: new Date().toISOString() };
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

  // -----------------------------------------------------------------------
  // GET /api/cli-router — read-only snapshot
  // -----------------------------------------------------------------------
  router.get('/', async (_req: Request, res: Response) => {
    const stored = doRead();
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
        accounts: authFiles.map(mapAuthFileToAccount),
        models,
      };
      return res.json(snapshot);
    } catch {
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
    if (!body || typeof body !== 'object' || Object.keys(body).some(k => !allowed.includes(k))) {
      return res.status(400).json({ error: 'invalid_request', message: 'Only url, apiKey, and managementKey are allowed.' });
    }
    if (typeof body.url !== 'string' || !body.url) {
      return res.status(400).json({ error: 'invalid_url', message: 'URL is required.' });
    }

    let validatedUrl: URL;
    try { validatedUrl = validateRouterUrl(body.url); }
    catch (err) { return res.status(400).json({ error: 'invalid_url', message: err instanceof Error ? err.message : 'Invalid URL.' }); }

    const existing = doRead();
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
      return res.status(502).json({ error: 'upstream_models_failed', message: `Cannot reach /v1/models: ${err instanceof Error ? err.message : String(err)}` });
    }
    try {
      await fetchAuthFiles(baseUrl, managementKey, doFetch);
    } catch (err) {
      return res.status(502).json({ error: 'upstream_management_failed', message: `Cannot reach /v0/management/auth-files: ${err instanceof Error ? err.message : String(err)}` });
    }

    doWrite({ url: baseUrl, apiKey, managementKey });
    return res.json({ success: true });
  });

  // -----------------------------------------------------------------------
  // POST /api/cli-router/routing — strategy/affinity
  // -----------------------------------------------------------------------
  router.post('/routing', requireOwner, mutation('cli_router.routing'), async (req: Request, res: Response) => {
    const body = req.body;
    const allowed = ['strategy', 'sessionAffinity'];
    if (!body || typeof body !== 'object' || Object.keys(body).some(k => !allowed.includes(k))) {
      return res.status(400).json({ error: 'invalid_request', message: 'Only strategy and sessionAffinity are allowed.' });
    }
    const validStrategies: CliRouterStrategy[] = ['round-robin', 'fill-first', 'weighted-round-robin'];
    if (!validStrategies.includes(body.strategy)) {
      return res.status(400).json({ error: 'invalid_strategy', message: `Strategy must be one of: ${validStrategies.join(', ')}` });
    }
    if (typeof body.sessionAffinity !== 'boolean') {
      return res.status(400).json({ error: 'invalid_session_affinity', message: 'sessionAffinity must be a boolean.' });
    }
    const stored = doRead();
    if (!stored?.url || !stored?.managementKey) {
      return res.status(400).json({ error: 'unconfigured', message: 'Configure the CLI router connection first.' });
    }
    const baseUrl = stored.url.replace(/\/$/, '');

    try {
      // Update strategy
      await boundedUpstreamFetch({
        url: `${baseUrl}/routing/strategy`,
        method: 'PUT',
        headers: { 'Authorization': `Bearer ${stored.managementKey}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ value: body.strategy }),
        fetchFn: doFetch,
      });
      // Update session affinity
      await boundedUpstreamFetch({
        url: `${baseUrl}/v8/management/config/routing/session-affinity`,
        method: 'PATCH',
        headers: { 'Authorization': `Bearer ${stored.managementKey}`, 'Content-Type': 'application/json' },
        body: JSON.stringify(body.sessionAffinity),
        fetchFn: doFetch,
      });
      return res.json({ success: true });
    } catch (err) {
      return res.status(502).json({ error: 'routing_update_failed', message: err instanceof Error ? err.message : String(err) });
    }
  });

  // -----------------------------------------------------------------------
  // POST /api/cli-router/accounts/status — enable/disable
  // -----------------------------------------------------------------------
  router.post('/accounts/status', requireOwner, mutation('cli_router.account'), async (req: Request, res: Response) => {
    const body = req.body;
    const allowed = ['id', 'disabled'];
    if (!body || typeof body !== 'object' || Object.keys(body).some(k => !allowed.includes(k))) {
      return res.status(400).json({ error: 'invalid_request', message: 'Only id and disabled are allowed.' });
    }
    if (typeof body.id !== 'string' || !body.id) {
      return res.status(400).json({ error: 'invalid_id', message: 'Account id is required.' });
    }
    if (typeof body.disabled !== 'boolean') {
      return res.status(400).json({ error: 'invalid_disabled', message: 'disabled must be a boolean.' });
    }
    const stored = doRead();
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
      return res.status(404).json({ error: 'account_not_found', message: `Account ${body.id} not found.` });
    }

    try {
      await boundedUpstreamFetch({
        url: `${baseUrl}/v0/management/auth-files/status`,
        method: 'PATCH',
        headers: { 'Authorization': `Bearer ${stored.managementKey}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: account.name, disabled: body.disabled }),
        fetchFn: doFetch,
      });
      return res.json({ success: true });
    } catch (err) {
      return res.status(502).json({ error: 'account_status_failed', message: err instanceof Error ? err.message : String(err) });
    }
  });

  // -----------------------------------------------------------------------
  // POST /api/cli-router/accounts/refresh — quota refresh
  // -----------------------------------------------------------------------
  router.post('/accounts/refresh', requireOwner, mutation('cli_router.refresh'), async (req: Request, res: Response) => {
    const body = req.body;
    const allowed = ['id'];
    if (!body || typeof body !== 'object' || Object.keys(body).some(k => !allowed.includes(k))) {
      return res.status(400).json({ error: 'invalid_request', message: 'Only id is allowed.' });
    }
    if (typeof body.id !== 'string' || !body.id) {
      return res.status(400).json({ error: 'invalid_id', message: 'Account id is required.' });
    }
    const stored = doRead();
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
      return res.status(404).json({ error: 'account_not_found', message: `Account ${body.id} not found.` });
    }

    const cache = await refreshAccountQuota(account, { baseUrl, managementKey: stored.managementKey, fetchFn: doFetch });
    quotaCache.set(body.id, cache);

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
        accounts: authFiles.map(mapAuthFileToAccount),
        models,
      };
      return res.json(snapshot);
    } catch {
      // Fall back to partial response with updated quotas
      const snapshot: CliRouterSnapshot = {
        settings: { url: stored.url, hasApiKey: true, hasManagementKey: true },
        status: 'connected',
        strategy: 'round-robin',
        sessionAffinity: false,
        accounts: authFiles.map(mapAuthFileToAccount),
        models: [],
      };
      return res.json(snapshot);
    }
  });

  return router;
}
