/**
 * Issue #1272: repair an expired login from another device.
 *
 * LoginRepairs runs on the node that owns the credential. It starts the
 * provider's own login (or GitHub's device flow), and exposes only what a
 * phone needs: the verification link, the one-time code, fixed prompt text,
 * and the outcome. CLI output never leaves this module. A pasted code or key
 * is written to the login process or the node's credential store and is
 * never stored anywhere else.
 *
 * LoginRepairBroker runs on central. It starts repairs locally or on the
 * owning worker, and binds each one to the principal that started it plus a
 * one-time key, so a second device cannot read a code or submit a key for a
 * login it did not start.
 */
import { spawn, type ChildProcess } from 'node:child_process';
import crypto from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { repositoryCli, loginRepairMethod, type LoginRepairState, type LoginRepairView } from '@git-agent-harness/contracts';
import { nodeHeaders, type RegistryService } from './registryService.js';

const REPAIR_TTL_MS = 10 * 60_000;
/** A finished repair stays readable briefly so the device sees the outcome. */
const RESULT_TTL_MS = 5 * 60_000;
const OUTPUT_CAP = 64 * 1024;
const GITHUB = 'https://github.com';
/** gh's and opencode's public OAuth apps, as their own device flows use them. */
const GITHUB_CLIENTS = {
  gh: { clientId: '178c6fc778ccc68e1d6a', scope: 'repo read:org gist' },
  copilot: { clientId: 'Iv1.b507a08c87ecfe98', scope: 'read:user' }
} as const;
const VERIFICATION_HOSTS = {
  codex: ['auth.openai.com'],
  claude: ['claude.com', 'claude.ai', 'platform.claude.com', 'console.anthropic.com']
} as const;
const ENV_KEYS: Record<string, string> = { mistral: 'MISTRAL_API_KEY', nous: 'NOUS_API_KEY' };

export type Login = { backend: string; provider: string | null };

export class LoginRepairError extends Error {
  constructor(readonly status: 400 | 403 | 404 | 409 | 502, message: string) {
    super(message);
  }
}

const loginKey = (login: Login) => `${login.backend}|${login.provider ?? ''}`;
const terminal = (state: LoginRepairState) => ['succeeded', 'failed', 'expired', 'manual', 'install_required'].includes(state.status);
const stripAnsi = (text: string) => text.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '');

function verificationUrl(text: string, hosts: readonly string[]): string | null {
  for (const match of text.matchAll(/https:\/\/[^\s"'<>]+/g)) {
    try {
      const url = new URL(match[0]);
      if (url.protocol === 'https:' && hosts.includes(url.hostname) && match[0].length <= 2_048) return url.href;
    } catch { /* Not a URL. */ }
  }
  return null;
}

/** Pasted text: one line, bounded, printable. An API key also has no spaces. */
function validInput(text: unknown, key: boolean): text is string {
  return typeof text === 'string' && text.length > 0 && text.length <= 512
    && !/[\x00-\x1f\x7f]/.test(text) && (!key || !/\s/.test(text));
}

/** Writes a JSON or env file via a private temporary file and a rename. */
function replacePrivateFile(path: string, content: string): void {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${process.pid}.${crypto.randomBytes(4).toString('hex')}.tmp`;
  writeFileSync(temporary, content, { mode: 0o600 });
  renameSync(temporary, path);
}

interface Repair {
  id: string;
  login: Login;
  state: LoginRepairState;
  expiresAt: number;
  stop: () => void;
  input?: (text: string) => Promise<void>;
}

export interface LoginRepairDeps {
  spawn?: typeof spawn;
  fetch?: typeof fetch;
  env?: NodeJS.ProcessEnv;
  opencodeAuthPath?: string;
  providerKeysPath?: string;
  /** Re-runs this node's login check; the repair reports success after it. */
  onSuccess?: (login: Login) => Promise<unknown>;
  nodeId?: string;
  /** Poll interval floor for GitHub's device flow (tests shorten it). */
  minPollMs?: number;
  /** How long a repair may run before it is stopped (default 10 minutes). */
  ttlMs?: number;
}

export function providerKeysPath(env: NodeJS.ProcessEnv = process.env): string {
  return env.GAH_PROVIDER_KEYS_PATH ?? join(homedir(), '.config/gah/provider-keys.env');
}

/** Keys repaired from another device live in a node-private env file; the
 * server loads them at start so its checks and backends see them. */
export function loadProviderKeys(path = providerKeysPath(), env: NodeJS.ProcessEnv = process.env): void {
  if (!existsSync(path)) return;
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    const match = line.match(/^([A-Z][A-Z0-9_]*)=(\S+)$/);
    if (match && Object.values(ENV_KEYS).includes(match[1])) env[match[1]] = match[2];
  }
}

export class LoginRepairs {
  private repairs = new Map<string, Repair>();
  private readonly spawn: typeof spawn;
  private readonly fetch: typeof fetch;
  private readonly env: NodeJS.ProcessEnv;

  constructor(private readonly deps: LoginRepairDeps = {}) {
    this.spawn = deps.spawn ?? spawn;
    this.fetch = deps.fetch ?? fetch;
    this.env = deps.env ?? process.env;
  }

  start(login: Login): LoginRepairView {
    this.sweep();
    if ([...this.repairs.values()].some((repair) => loginKey(repair.login) === loginKey(login) && !terminal(repair.state))) {
      throw new LoginRepairError(409, 'A login for this is already in progress.');
    }
    const repair: Repair = {
      id: crypto.randomUUID(),
      login,
      state: { status: 'starting' },
      expiresAt: Date.now() + (this.deps.ttlMs ?? REPAIR_TTL_MS),
      stop: () => undefined
    };
    this.repairs.set(repair.id, repair);
    const expiry = setTimeout(() => {
      if (terminal(repair.state)) return;
      repair.stop();
      repair.state = { status: 'expired' };
    }, this.deps.ttlMs ?? REPAIR_TTL_MS);
    expiry.unref?.();
    if (repositoryCli(login.backend)) {
      void this.checkRepositoryTool(repair);
    } else {
      this.beginLogin(repair);
    }
    return this.view(repair.id)!;
  }

  private beginLogin(repair: Repair): void {
    const login = repair.login;
    const method = loginRepairMethod(login);
    if (method === 'device_cli') this.runCli(repair, 'codex', ['login', '--device-auth'], 'codex');
    else if (method === 'paste_code') this.runCli(repair, 'claude', ['auth', 'login', '--claudeai'], 'claude');
    else if (method === 'github_device') void this.runGithubDevice(repair, login.backend === 'gh' ? 'gh' : 'copilot');
    else if (method === 'api_key') this.awaitApiKey(repair);
    else repair.state = { status: 'manual', instructions: `Open a terminal on this machine and log in with the ${login.backend} CLI${login.backend === 'glab' ? ' (glab auth login)' : ''}.` };
  }

  /** A repository package must run before any authorization request is sent. */
  private async checkRepositoryTool(repair: Repair): Promise<void> {
    const tool = repositoryCli(repair.login.backend)!;
    try {
      const available = await new Promise<boolean>((resolve) => {
        const child = this.spawn(repair.login.backend, ['--version'], { stdio: 'ignore', env: this.env });
        const timeout = setTimeout(() => { child.kill(); resolve(false); }, 15_000);
        timeout.unref?.();
        repair.stop = () => { clearTimeout(timeout); child.kill(); resolve(false); };
        child.once('error', () => { clearTimeout(timeout); resolve(false); });
        child.once('close', code => { clearTimeout(timeout); resolve(code === 0); });
      });
      if (terminal(repair.state)) return;
      if (!available) repair.state = { status: 'install_required', install_url: tool.installUrl };
      else this.beginLogin(repair);
    } catch {
      this.fail(repair, 'The repository CLI could not be checked.');
    }
  }

  view(id: string): LoginRepairView | null {
    const repair = this.repairs.get(id);
    if (!repair) return null;
    return {
      ...repair.state,
      id: repair.id,
      node_id: this.deps.nodeId ?? '',
      backend: repair.login.backend,
      provider: repair.login.provider,
      expires_at: new Date(repair.expiresAt).toISOString()
    };
  }

  async submit(id: string, text: unknown): Promise<LoginRepairView> {
    const repair = this.repairs.get(id);
    if (!repair) throw new LoginRepairError(404, 'This login repair is no longer available.');
    if (repair.state.status !== 'awaiting_input' || !repair.input) throw new LoginRepairError(409, 'This login is not waiting for input.');
    if (!validInput(text, repair.state.secret)) throw new LoginRepairError(400, 'Paste one line of text without spaces or control characters.');
    await repair.input(text);
    return this.view(id)!;
  }

  cancel(id: string): void {
    const repair = this.repairs.get(id);
    if (!repair) return;
    repair.stop();
    if (!terminal(repair.state)) repair.state = { status: 'failed', reason: 'Cancelled.' };
  }

  private sweep(): void {
    for (const [id, repair] of this.repairs) {
      if (terminal(repair.state) && Date.now() > repair.expiresAt + RESULT_TTL_MS) this.repairs.delete(id);
    }
  }

  private async succeed(repair: Repair): Promise<void> {
    try { await this.deps.onSuccess?.(repair.login); }
    catch { /* The login worked; the next scheduled check reports it. */ }
    if (!terminal(repair.state)) repair.state = { status: 'succeeded' };
  }

  private fail(repair: Repair, reason: string): void {
    if (!terminal(repair.state)) repair.state = { status: 'failed', reason };
  }

  /** Runs the provider's own login and reads only its link and code. */
  private runCli(repair: Repair, command: 'codex' | 'claude', args: string[], hosts: keyof typeof VERIFICATION_HOSTS): void {
    let child: ChildProcess;
    try {
      child = this.spawn(command, args, { stdio: ['pipe', 'pipe', 'pipe'], env: this.env, detached: process.platform !== 'win32' });
    } catch {
      return this.fail(repair, `The ${command} CLI could not start on this machine.`);
    }
    let output = '';
    let url: string | null = null;
    repair.stop = () => {
      try { if (child.pid) process.kill(process.platform === 'win32' ? child.pid : -child.pid, 'SIGTERM'); }
      catch { child.kill('SIGTERM'); }
    };
    const read = (chunk: Buffer | string) => {
      output = stripAnsi((output + chunk).slice(-OUTPUT_CAP));
      if (terminal(repair.state) || repair.state.status === 'waiting') return;
      url ??= verificationUrl(output, VERIFICATION_HOSTS[hosts]);
      if (!url) return;
      if (command === 'claude') {
        if (repair.state.status !== 'awaiting_input') {
          repair.state = { status: 'awaiting_input', url, prompt: 'Sign in, then paste the code the page shows.', secret: false };
        }
        return;
      }
      const after = output.slice(output.indexOf('one-time code') >= 0 ? output.indexOf('one-time code') : 0);
      const code = after.match(/\b[A-Z0-9]{4,}(?:-[A-Z0-9]{3,})+\b/)?.[0] ?? null;
      repair.state = { status: 'open_url', url, code };
    };
    child.stdout?.on('data', read);
    child.stderr?.on('data', read);
    child.stdin?.on('error', () => undefined);
    repair.input = async (text) => {
      child.stdin?.write(`${text}\n`);
      repair.state = { status: 'waiting' };
    };
    child.on('error', (error: NodeJS.ErrnoException) => {
      this.fail(repair, error.code === 'ENOENT' ? `The ${command} CLI is not installed on this machine.` : `The ${command} login could not start.`);
    });
    child.on('close', (code) => {
      if (code === 0) void this.succeed(repair);
      else this.fail(repair, 'The login did not complete.');
    });
  }

  /** GitHub's device flow, then the token goes to gh or opencode's store. */
  private async runGithubDevice(repair: Repair, client: keyof typeof GITHUB_CLIENTS): Promise<void> {
    const { clientId, scope } = GITHUB_CLIENTS[client];
    let stopped = false;
    repair.stop = () => { stopped = true; };
    const post = async (path: string, body: Record<string, string>) => {
      const response = await this.fetch(`${GITHUB}${path}`, {
        method: 'POST', redirect: 'error', signal: AbortSignal.timeout(15_000),
        headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return await response.json() as Record<string, unknown>;
    };
    try {
      const device = await post('/login/device/code', { client_id: clientId, scope });
      const { device_code: deviceCode, user_code: userCode, verification_uri: uri } = device;
      if (typeof deviceCode !== 'string' || typeof userCode !== 'string' || !/^[A-Z0-9-]{4,16}$/.test(userCode)
        || typeof uri !== 'string' || !uri.startsWith(`${GITHUB}/`)) {
        return this.fail(repair, 'GitHub returned an unexpected device code.');
      }
      repair.state = { status: 'open_url', url: uri, code: userCode };
      const given = Number(device.interval);
      let interval = Math.max(Number.isFinite(given) && given >= 0 ? given * 1000 : 5_000, this.deps.minPollMs ?? 5_000);
      while (!stopped && Date.now() < repair.expiresAt) {
        await new Promise((resolve) => { const timer = setTimeout(resolve, interval); timer.unref?.(); });
        if (stopped) return;
        const token = await post('/login/oauth/access_token', {
          client_id: clientId, device_code: deviceCode, grant_type: 'urn:ietf:params:oauth:grant-type:device_code'
        });
        if (typeof token.access_token === 'string' && token.access_token) {
          repair.state = { status: 'waiting' };
          await (client === 'gh' ? this.storeGhToken(token.access_token) : this.storeOpencode('github-copilot', { type: 'oauth', refresh: token.access_token, access: '', expires: 0 }));
          return this.succeed(repair);
        }
        if (token.error === 'authorization_pending') continue;
        if (token.error === 'slow_down') { interval += 5_000; continue; }
        return this.fail(repair, token.error === 'access_denied' ? 'The sign-in was declined.' : 'GitHub ended the sign-in.');
      }
    } catch (error) {
      this.fail(repair, error instanceof StoreError ? error.message : 'GitHub could not be reached from this machine.');
    }
  }

  private storeGhToken(token: string): Promise<void> {
    return new Promise((resolve, reject) => {
      const child = this.spawn('gh', ['auth', 'login', '--hostname', 'github.com', '--with-token'], { stdio: ['pipe', 'ignore', 'ignore'], env: this.env });
      child.on('error', () => reject(new StoreError('The gh CLI is not installed on this machine.')));
      child.on('close', (code) => code === 0 ? resolve() : reject(new StoreError('gh did not accept the new login.')));
      child.stdin?.on('error', () => undefined);
      child.stdin?.end(`${token}\n`);
    });
  }

  /** Merges one provider entry into opencode's auth.json, never rewriting an
   * unreadable file. */
  private async storeOpencode(provider: string, entry: Record<string, unknown>): Promise<void> {
    const path = this.deps.opencodeAuthPath
      ?? join(this.env.XDG_DATA_HOME ?? join(homedir(), '.local/share'), 'opencode/auth.json');
    let current: Record<string, unknown> = {};
    if (existsSync(path)) {
      try { current = JSON.parse(readFileSync(path, 'utf8')) as Record<string, unknown>; }
      catch { throw new StoreError('opencode\'s auth.json is unreadable; fix it on this machine first.'); }
      if (!current || typeof current !== 'object' || Array.isArray(current)) throw new StoreError('opencode\'s auth.json is unreadable; fix it on this machine first.');
    }
    replacePrivateFile(path, `${JSON.stringify({ ...current, [provider]: entry }, null, 2)}\n`);
  }

  private storeEnvKey(variable: string, key: string): void {
    const path = this.deps.providerKeysPath ?? providerKeysPath(this.env);
    const lines = existsSync(path) ? readFileSync(path, 'utf8').split('\n').filter((line) => line && !line.startsWith(`${variable}=`)) : [];
    replacePrivateFile(path, `${[...lines, `${variable}=${key}`].join('\n')}\n`);
    this.env[variable] = key;
  }

  private awaitApiKey(repair: Repair): void {
    const { backend, provider } = repair.login;
    const label = provider ?? backend;
    repair.state = { status: 'awaiting_input', url: null, prompt: `Paste the ${label} API key. It is stored on this machine only.`, secret: true };
    repair.input = async (key) => {
      try {
        if (backend === 'opencode') await this.storeOpencode(label, { type: 'api', key });
        else this.storeEnvKey(ENV_KEYS[label], key);
      } catch (error) {
        return this.fail(repair, error instanceof StoreError ? error.message : 'The key could not be saved on this machine.');
      }
      repair.state = { status: 'waiting' };
      await this.succeed(repair);
    };
  }
}

class StoreError extends Error {}

const STATES = new Set(['starting', 'open_url', 'awaiting_input', 'waiting', 'succeeded', 'failed', 'expired', 'manual', 'install_required']);
const short = (value: unknown, max: number) => typeof value === 'string' && value.length > 0 && value.length <= max;

/** Central accepts only the documented view from a worker, with an HTTPS link. */
export function parseLoginRepairView(value: unknown): LoginRepairView | null {
  if (!value || typeof value !== 'object') return null;
  const view = value as Record<string, unknown>;
  if (!short(view.id, 64) || !short(view.backend, 64) || !(view.provider === null || short(view.provider, 64))
    || !short(view.expires_at, 64) || !STATES.has(view.status as string)) return null;
  const base = { id: view.id as string, node_id: typeof view.node_id === 'string' ? view.node_id : '', backend: view.backend as string, provider: view.provider as string | null, expires_at: view.expires_at as string };
  const https = (url: unknown) => short(url, 2_048) && (url as string).startsWith('https://');
  switch (view.status) {
    case 'install_required': {
      const tool = repositoryCli(base.backend);
      return tool && view.install_url === tool.installUrl ? { ...base, status: 'install_required', install_url: tool.installUrl } : null;
    }
    case 'open_url':
      if (!https(view.url) || !(view.code === null || (typeof view.code === 'string' && /^[A-Z0-9-]{4,32}$/.test(view.code)))) return null;
      return { ...base, status: 'open_url', url: view.url as string, code: view.code as string | null };
    case 'awaiting_input':
      if (!(view.url === null || https(view.url)) || !short(view.prompt, 200) || typeof view.secret !== 'boolean') return null;
      return { ...base, status: 'awaiting_input', url: view.url as string | null, prompt: view.prompt as string, secret: view.secret };
    case 'failed':
      return { ...base, status: 'failed', reason: short(view.reason, 200) ? view.reason as string : 'The login did not complete.' };
    case 'manual':
      return short(view.instructions, 300) ? { ...base, status: 'manual', instructions: view.instructions as string } : null;
    default:
      return { ...base, status: view.status as 'starting' | 'waiting' | 'succeeded' | 'expired' };
  }
}

export type RepairPrincipal = { kind: 'owner' } | { kind: 'device'; id: string };
const principalKey = (principal: RepairPrincipal) => principal.kind === 'owner' ? 'owner' : `device:${principal.id}`;
const digest = (key: string) => crypto.createHash('sha256').update(key).digest();

interface BrokeredRepair {
  id: string;
  keyDigest: Buffer;
  principal: string;
  nodeId: string;
  remoteId: string;
  expiresAt: number;
  reportedSuccess: boolean;
}

/** Central's side: routes each repair to the node that owns the credential
 * and lets only the device that started it read or answer it. */
export class LoginRepairBroker {
  private repairs = new Map<string, BrokeredRepair>();

  constructor(private readonly deps: {
    localNodeId: string;
    local: LoginRepairs;
    registry: RegistryService;
    /** A worker's repair succeeded; pull its fresh login report. */
    onRemoteSuccess?: (nodeId: string) => void;
    fetch?: typeof fetch;
  }) {}

  async start(principal: RepairPrincipal, nodeId: string, login: Login): Promise<{ key: string; repair: LoginRepairView }> {
    for (const [id, repair] of this.repairs) if (Date.now() > repair.expiresAt + RESULT_TTL_MS) this.repairs.delete(id);
    const view = nodeId === this.deps.localNodeId
      ? this.deps.local.start(login)
      : await this.worker(nodeId, { action: 'start', backend: login.backend, provider: login.provider });
    const key = crypto.randomBytes(32).toString('hex');
    const id = crypto.randomUUID();
    this.repairs.set(id, {
      id, keyDigest: digest(key), principal: principalKey(principal), nodeId,
      remoteId: view.id, expiresAt: Date.parse(view.expires_at) || Date.now() + REPAIR_TTL_MS, reportedSuccess: false
    });
    return { key, repair: { ...view, id, node_id: nodeId } };
  }

  async view(id: string, principal: RepairPrincipal, key: unknown): Promise<LoginRepairView> {
    const repair = this.owned(id, principal, key);
    const view = repair.nodeId === this.deps.localNodeId
      ? this.deps.local.view(repair.remoteId)
      : await this.worker(repair.nodeId, { action: 'status', id: repair.remoteId });
    if (!view) throw new LoginRepairError(404, 'This login repair is no longer available.');
    if (view.status === 'succeeded' && !repair.reportedSuccess && repair.nodeId !== this.deps.localNodeId) {
      repair.reportedSuccess = true;
      this.deps.onRemoteSuccess?.(repair.nodeId);
    }
    return { ...view, id, node_id: repair.nodeId };
  }

  async submit(id: string, principal: RepairPrincipal, key: unknown, text: unknown): Promise<LoginRepairView> {
    const repair = this.owned(id, principal, key);
    if (repair.nodeId === this.deps.localNodeId) await this.deps.local.submit(repair.remoteId, text);
    else await this.worker(repair.nodeId, { action: 'input', id: repair.remoteId, text });
    return this.view(id, principal, key);
  }

  async cancel(id: string, principal: RepairPrincipal, key: unknown): Promise<void> {
    const repair = this.owned(id, principal, key);
    if (repair.nodeId === this.deps.localNodeId) this.deps.local.cancel(repair.remoteId);
    else await this.worker(repair.nodeId, { action: 'cancel', id: repair.remoteId });
  }

  /** Same answer for "no such repair" and "not yours": a second device
   * learns nothing about a repair it did not start. */
  private owned(id: string, principal: RepairPrincipal, key: unknown): BrokeredRepair {
    const repair = this.repairs.get(id);
    const matches = !!repair && typeof key === 'string' && key.length === 64
      && crypto.timingSafeEqual(digest(key), repair.keyDigest) && repair.principal === principalKey(principal);
    if (!repair || !matches) throw new LoginRepairError(404, 'This login repair is no longer available.');
    return repair;
  }

  private async worker(nodeId: string, body: Record<string, unknown>): Promise<LoginRepairView> {
    const node = this.deps.registry.getNode(nodeId);
    if (!node) throw new LoginRepairError(404, 'That node is no longer registered.');
    const endpoint = new URL('/api/login-repair', node.advertised_url);
    if (endpoint.protocol !== 'https:' && process.env.GAH_ALLOW_INSECURE_HTTP !== '1'
      && !['localhost', '127.0.0.1', '[::1]'].includes(endpoint.hostname)) {
      throw new LoginRepairError(502, 'Login repair on this worker requires HTTPS or trusted LAN HTTP.');
    }
    let response: Response;
    try {
      response = await (this.deps.fetch ?? fetch)(endpoint, {
        method: 'POST', redirect: 'error', signal: AbortSignal.timeout(20_000),
        headers: { ...nodeHeaders(node), 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
      });
    } catch {
      throw new LoginRepairError(502, 'The worker could not be reached.');
    }
    const payload = await response.json().catch(() => null) as Record<string, unknown> | null;
    if (!response.ok) {
      const status = response.status === 400 || response.status === 404 || response.status === 409 ? response.status : 502;
      throw new LoginRepairError(status, typeof payload?.message === 'string' && payload.message.length <= 200 ? payload.message : 'The worker refused the login repair.');
    }
    if (body.action === 'cancel') return { id: '', node_id: nodeId, backend: '', provider: null, expires_at: '', status: 'failed', reason: 'Cancelled.' };
    const view = parseLoginRepairView(payload);
    if (!view) throw new LoginRepairError(502, 'The worker returned an unexpected login repair.');
    return view;
  }
}
