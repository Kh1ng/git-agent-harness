import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname, sep } from 'node:path';
import crypto from 'node:crypto';
import { spawn } from 'node:child_process';
import { hostname, networkInterfaces } from 'node:os';
import type {
  DoctorSnapshot,
  RegisteredNode,
  NodeSummary,
  NodeHealthCheckResult,
  NodeObservationSnapshot,
  NodeObservationState
} from '@git-agent-harness/contracts';
import { COORDINATOR_SCHEMA_DIGEST } from './coordinatorIdentity.js';

export function isLoopback(urlStr: string): boolean {
  try {
    const url = new URL(urlStr);
    const host = url.hostname;
    return (
      host === 'localhost' ||
      host === '127.0.0.1' ||
      host === '::1' ||
      host.startsWith('127.') ||
      host === '[::1]'
    );
  } catch {
    return false;
  }
}

export function getEndpoint(urlStr: string): string {
  try {
    const url = new URL(urlStr);
    return `${normalizeLoopbackHost(url.hostname)}:${urlPort(url)}`;
  } catch {
    return urlStr;
  }
}

function urlPort(url: URL): number {
  return Number(url.port || (url.protocol === 'https:' || url.protocol === 'wss:' ? 443 : 80));
}

function isCentralEndpoint(candidateUrl: string, advertisedCentralUrl: string, listenerPort: number): boolean {
  if (getEndpoint(candidateUrl) === getEndpoint(advertisedCentralUrl)) return true;
  try {
    const candidate = new URL(candidateUrl);
    const listenerHosts = localListenerHosts();
    listenerHosts.add(normalizeLoopbackHost(new URL(advertisedCentralUrl).hostname));
    return urlPort(candidate) === listenerPort && listenerHosts.has(normalizeLoopbackHost(candidate.hostname));
  } catch {
    return false;
  }
}

function localListenerHosts(): Set<string> {
  const hosts = new Set([normalizeLoopbackHost(hostname()), '127.0.0.1', '0.0.0.0', '::']);
  for (const addresses of Object.values(networkInterfaces())) {
    for (const address of addresses ?? []) hosts.add(normalizeLoopbackHost(address.address));
  }
  return hosts;
}

/** Normalizes every loopback spelling to one canonical host so endpoint
 * collision/self-poll comparisons don't treat `localhost:3773` and
 * `127.0.0.1:3773` as different endpoints (they resolve to the same node). */
function normalizeLoopbackHost(host: string): string {
  const lower = host.toLowerCase().replace(/^\[|\]$/g, '');
  if (lower === 'localhost' || lower === '::1' || lower.startsWith('127.')) {
    return '127.0.0.1';
  }
  return lower;
}

export function containsSecretWords(text: string): boolean {
  const secretPatterns = [/key/i, /secret/i, /password/i, /token/i, /cert/i, /credential/i, /auth/i, /private/i];
  return secretPatterns.some((pattern) => pattern.test(text));
}

export function isSchemaCompatible(schemaDigest: string): boolean {
  return schemaDigest === COORDINATOR_SCHEMA_DIGEST;
}

const NODE_OBSERVATION_TIMEOUT_MS = 5_000;
const NODE_STALE_AFTER_MS = 30 * 60 * 1000;
const NODE_POLL_CONCURRENCY = 4;

// Node liveness scheduler (issue #883): nothing drove pollNodeObservation
// periodically before this -- it only ran on-demand (dashboard fetch,
// dispatch routing), so a node with nothing currently dispatching to it
// could go dark and never get flagged. "Bad" states below all mean the
// node isn't answering health checks correctly; each is worth escalating
// the same way, so they're treated as one bucket for alerting purposes.
const LIVENESS_POLL_INTERVAL_MS = 60_000;
const LIVENESS_ALERT_AFTER_CONSECUTIVE_BAD_CHECKS = 3;
const LIVENESS_BAD_STATES: NodeObservationState[] = ['stale', 'unreachable', 'auth_failed', 'incompatible'];

function nowIso(ms: number = Date.now()): string {
  return new Date(ms).toISOString();
}

/** Pipes a single one-line message to a shell command's stdin, matching the
 * Rust side's per-profile `notify_command` shape (`docs/OPERATIONS.md`
 * section 4) so an operator configuring both doesn't learn two conventions.
 * Reads GAH_NODE_LIVENESS_NOTIFY_COMMAND fresh per call (not cached at
 * module load) so tests can vary it. A failing or missing command is
 * logged to stderr and swallowed -- it must never crash the scheduler. */
function sendLivenessAlert(message: string): void {
  const command = process.env.GAH_NODE_LIVENESS_NOTIFY_COMMAND;
  if (!command) return;
  try {
    const child = spawn('sh', ['-c', command], { stdio: ['pipe', 'ignore', 'pipe'] });
    child.on('error', (err) => console.error(`Node liveness notify command failed to start: ${err.message}`));
    child.stderr?.on('data', (chunk) => console.error(`Node liveness notify command stderr: ${chunk}`));
    child.stdin.write(`${message}\n`);
    child.stdin.end();
  } catch (err) {
    console.error(`Node liveness notify command failed: ${err instanceof Error ? err.message : String(err)}`);
  }
}

function parseIsoMillis(value: string | null | undefined): number | null {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : parsed;
}

function mapStateToResult(state: NodeObservationState): 'healthy' | 'unhealthy' {
  return state === 'healthy' ? 'healthy' : 'unhealthy';
}

function majorMinor(version: string): string | null {
  const parts = version.split('.');
  if (parts.length < 2) return null;
  return `${parts[0]}.${parts[1]}`;
}

function normalizeResourcePressure(value: unknown): NodeObservationSnapshot['resource_pressure'] {
  if (!value || typeof value !== 'object') {
    return {
      cpu_percent: null,
      rss_bytes: null,
      disk_percent: null
    };
  }
  const record = value as Record<string, unknown>;
  const toNumber = (candidate: unknown): number | null => (typeof candidate === 'number' && Number.isFinite(candidate) ? candidate : null);
  return {
    cpu_percent: toNumber(record.cpu_percent ?? record.cpuPressurePercent ?? record.cpu_utilization_percent),
    rss_bytes: toNumber(record.rss_bytes ?? record.rssBytes),
    disk_percent: toNumber(record.disk_percent ?? record.diskPressurePercent ?? record.disk_utilization_percent)
  };
}

function dedupeNodeWorkItems(nodeId: string, claims: unknown[]): NodeObservationSnapshot['active_work'] {
  const seen = new Set<string>();
  const workItems: NodeObservationSnapshot['active_work'] = [];
  for (const claim of claims) {
    if (!claim || typeof claim !== 'object') continue;
    const record = claim as Record<string, unknown>;
    const workId = typeof record.work_id === 'string' ? record.work_id : null;
    if (!workId) continue;
    const nodeQualifiedWorkId = `${nodeId}:${workId}`;
    if (seen.has(nodeQualifiedWorkId)) continue;
    seen.add(nodeQualifiedWorkId);
    workItems.push({
      node_id: nodeId,
      work_id: workId,
      node_qualified_work_id: nodeQualifiedWorkId,
      scope: typeof record.scope === 'string' ? record.scope : '',
      hostname: typeof record.hostname === 'string' ? record.hostname : '',
      claimed_at: typeof record.claimed_at === 'string' ? record.claimed_at : '',
      age_seconds: typeof record.age_seconds === 'number' && Number.isFinite(record.age_seconds) ? record.age_seconds : 0
    });
  }
  return workItems;
}

function emptyNodeObservation(
  node: RegisteredNode,
  observedAt: string,
  state: NodeObservationState,
  lastSeenAt: string | null,
  error?: { kind: string; message: string } | null
): NodeObservationSnapshot {
  return {
    node_id: node.node_id,
    display_name: node.display_name,
    advertised_url: node.advertised_url,
    version: node.version,
    schema_digest: node.schema_digest,
    state,
    observed_at: observedAt,
    last_seen_at: lastSeenAt ?? node.last_seen_at ?? null,
    last_observed_state: node.last_observed_state ?? null,
    last_error_kind: node.last_error_kind ?? null,
    last_error_message: node.last_error_message ?? null,
    profile: null,
    profiles: [],
    backend_configured: {},
    backend_instances: [],
    availability: [],
    recent_ledger: null,
    active_claims: [],
    active_work: [],
    event_cursor: null,
    resource_pressure: {
      cpu_percent: null,
      rss_bytes: null,
      disk_percent: null
    },
    error: error ? (error as NodeObservationSnapshot['error']) : null
  };
}

/** The deadline covers response-body reads too; redirects cannot change the registered target. */
async function fetchWithTimeout(url: string, headers: Record<string, string>, timeoutMs: number): Promise<Response> {
  return fetch(url, { headers, signal: AbortSignal.timeout(timeoutMs), redirect: 'error' });
}

/** All registered-node requests use the same transport and secret-reference rules. */
export function nodeHeaders(node: RegisteredNode): Record<string, string> {
  const headers: Record<string, string> = { Accept: 'application/json', 'User-Agent': 'GAH-Coordinator/0.1.0' };
  if (node.transport_mode === 'authenticated_remote' || (node.transport_mode === 'trusted_lan' && !isLoopback(node.advertised_url))) {
    headers.Authorization = `Bearer ${resolveSecret(node.secret_ref)}`;
  }
  return headers;
}

export class NodeDoctorError extends Error {
  constructor(public readonly status: number, message: string) { super(message); }
}

/** Validate the worker boundary before its output becomes dashboard readiness. */
function isDoctorSnapshot(value: unknown, profile: string): value is DoctorSnapshot {
  if (!value || typeof value !== 'object') return false;
  const snapshot = value as Partial<DoctorSnapshot>;
  return snapshot.schema_version === 1 && typeof snapshot.generated_at === 'string'
    && Number.isFinite(Date.parse(snapshot.generated_at))
    && ['ok', 'warn', 'fail'].includes(snapshot.overall_status ?? '')
    && Array.isArray(snapshot.checks) && snapshot.checks.length > 0
    && snapshot.checks.every((check) => check && typeof check.name === 'string' && typeof check.detail === 'string'
      && ['ok', 'warn', 'fail'].includes(check.status) && (check.profile == null || check.profile === profile));
}

async function mapWithConcurrency<T, U>(
  items: T[],
  concurrency: number,
  fn: (item: T, index: number) => Promise<U>
): Promise<U[]> {
  if (items.length === 0) return [];
  const results = new Array<U>(items.length);
  let index = 0;
  const workers = Array.from({ length: Math.max(1, Math.min(concurrency, items.length)) }, async () => {
    while (true) {
      const current = index++;
      if (current >= items.length) return;
      results[current] = await fn(items[current], current);
    }
  });
  await Promise.all(workers);
  return results;
}

/** Node health checks fetch an operator-supplied `advertised_url`, so a `file:`
 * secret ref must never resolve outside a known directory -- otherwise a
 * registrant could point advertised_url at a server they control and use the
 * coordinator as an oracle to read (and exfiltrate, via the Authorization
 * header it sends) any file readable by the server process. */
const DEFAULT_NODE_SECRETS_ROOT = '/etc/gah/node-secrets';

export function nodeSecretsRoot(): string {
  return resolve(process.env.GAH_NODE_SECRETS_ROOT || DEFAULT_NODE_SECRETS_ROOT);
}

export function resolveSecret(secretRef: string): string {
  if (!secretRef) {
    throw new Error('Secret reference is empty');
  }
  if (secretRef.startsWith('env:')) {
    const envVar = secretRef.slice(4);
    const val = process.env[envVar];
    if (val === undefined) {
      throw new Error(`Environment variable ${envVar} is not set`);
    }
    return val;
  }
  if (secretRef.startsWith('file:')) {
    const root = nodeSecretsRoot();
    const filePath = resolve(secretRef.slice(5));
    if (filePath !== root && !filePath.startsWith(root + sep)) {
      throw new Error(`Secret file path must be inside ${root}`);
    }
    try {
      return readFileSync(filePath, 'utf8').trim();
    } catch (e: any) {
      throw new Error(`Failed to read secret file ${filePath}: ${e.message}`);
    }
  }
  throw new Error(`Unsupported secret reference format (must start with 'env:' or 'file:')`);
}

export class RegistryService {
  private observationRequests = new Map<string, number>();
  private observations = new Map<string, NodeObservationSnapshot>();
  private listeners = new Set<() => void>();
  private configPath: string | null;
  private nodes: Map<string, RegisteredNode> = new Map();
  private livenessTimer: ReturnType<typeof setInterval> | null = null;
  private consecutiveBadChecks: Map<string, number> = new Map();
  private alreadyAlerted: Set<string> = new Set();
  /** Normalized endpoint of the central node itself (issue #944): a worker
   * must never register a node whose advertised_url is the central node's
   * own endpoint -- the liveness scheduler would poll the central's own
   * /api/status and recurse (observed live: every fleet/status call 502s). */
  private selfUrl: string | null;
  private listenerPort: number | null;

  constructor(configPath?: string | null, selfEndpoint?: string, listenerPort?: number) {
    // A worker uses local session transport without reading or writing a central registry.
    this.configPath = configPath === null ? null : configPath || process.env.GAH_REGISTRY_CONFIG_PATH || resolve(process.cwd(), 'config/registry-config.json');
    this.selfUrl = selfEndpoint ?? null;
    this.listenerPort = listenerPort ?? (selfEndpoint ? urlPort(new URL(selfEndpoint)) : null);
    this.load();
  }

  private load() {
    if (this.configPath && existsSync(this.configPath)) {
      try {
        const data = JSON.parse(readFileSync(this.configPath, 'utf8'));
        if (Array.isArray(data.nodes)) {
          for (const node of data.nodes) {
            this.nodes.set(node.node_id, node);
          }
        }
      } catch (e) {
        console.error('Failed to load registry config:', e);
      }
    }
  }

  private save() {
    if (!this.configPath) throw new Error('Workers cannot persist a central registry.');
    try {
      const dir = dirname(this.configPath);
      if (!existsSync(dir)) {
        mkdirSync(dir, { recursive: true });
      }
      const data = {
        nodes: Array.from(this.nodes.values())
      };
      writeFileSync(this.configPath, JSON.stringify(data, null, 2));
    } catch (e) {
      console.error('Failed to save registry config:', e);
      throw e;
    }
  }

  getNodes(): RegisteredNode[] {
    return Array.from(this.nodes.values());
  }

  getNode(nodeId: string): RegisteredNode | undefined {
    return this.nodes.get(nodeId);
  }

  getNodesSummary(): NodeSummary[] {
    return this.getNodes().map(({ secret_ref, ...summary }) => summary);
  }

  /** Notify dashboards to refetch authenticated data, without publishing node details. */
  onChange(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  private changed(): void {
    for (const listener of this.listeners) listener();
  }

  /** Reads observations already collected by liveness checks; never contacts a node. */
  getCachedObservations(): NodeObservationSnapshot[] {
    return [...this.observations.values()];
  }

  async getNodeObservations(profile?: string): Promise<NodeObservationSnapshot[]> {
    const nodes = this.getNodes();
    const observations = await mapWithConcurrency(nodes, NODE_POLL_CONCURRENCY, async (node) => {
      const result = await this.pollNodeObservation(node, profile);
      return result.snapshot ?? emptyNodeObservation(node, nowIso(result.timestamp), result.state, result.last_seen_at ?? null, result.error ?? null);
    });
    if (!profile) this.changed();
    return observations;
  }

  private persistObservation(
    node: RegisteredNode,
    observedAt: string,
    state: NodeObservationState,
    lastSeenAt: string | null,
    error?: { kind: string; message: string } | null
  ): void {
    if (this.nodes.get(node.node_id) !== node) return;
    node.last_observed_at = observedAt;
    node.last_observed_state = state;
    node.last_seen_at = lastSeenAt ?? node.last_seen_at ?? null;
    node.last_error_kind = error?.kind as RegisteredNode['last_error_kind'];
    node.last_error_message = error?.message ?? null;
    this.save();
  }

  private async pollNodeObservation(node: RegisteredNode, profile?: string): Promise<NodeHealthCheckResult> {
    const sequence = (this.observationRequests.get(node.node_id) ?? 0) + 1;
    this.observationRequests.set(node.node_id, sequence);
    const result = await this.fetchNodeObservation(node, profile);
    // Publish only the newest request for this registration. A slow poll must
    // not undo a newer health check or restore a revoked/repointed node.
    if (this.nodes.get(node.node_id) === node && this.observationRequests.get(node.node_id) === sequence) {
      this.persistObservation(node, nowIso(result.timestamp), result.state, result.last_seen_at, result.error);
      // A scoped dispatch must not replace the fleet-wide observation.
      if (!profile) this.observations.set(node.node_id, result.snapshot ?? emptyNodeObservation(
        node, nowIso(result.timestamp), result.state, result.last_seen_at, result.error
      ));
    }
    return result;
  }

  private async fetchNodeObservation(node: RegisteredNode, profile?: string): Promise<NodeHealthCheckResult> {
    const start = Date.now();
    const observedAt = nowIso(start);
    const snapshotUrl = new URL('/api/status', node.advertised_url);
    // Workers may not have a profile named "gah" (the status endpoint's default).
    const observedProfile = profile ?? node.profiles?.[0];
    if (observedProfile) {
      snapshotUrl.searchParams.set('profile', observedProfile);
    }

    let headers: Record<string, string>;
    try {
      headers = nodeHeaders(node);
    } catch (e: any) {
      return {
        node_id: node.node_id, status: 'unhealthy', state: 'auth_failed', timestamp: start,
        last_seen_at: node.last_seen_at ?? null,
        error: { kind: 'AUTH', message: `Failed to resolve secret reference: ${e.message}` }
      };
    }

    let response: Response;
    try {
      response = await fetchWithTimeout(snapshotUrl.toString(), headers, NODE_OBSERVATION_TIMEOUT_MS);
    } catch (err: any) {
      const errorMessage = err?.cause?.message || err?.message || String(err);
      const errorCode = err?.cause?.code || err?.code || '';
      let state: NodeObservationState = 'unreachable';
      let kind: NonNullable<NodeHealthCheckResult['error']>['kind'] = 'NETWORK';
      if (errorCode === 'ENOTFOUND' || errorCode === 'EAI_AGAIN' || errorMessage.includes('ENOTFOUND') || errorMessage.includes('EAI_AGAIN')) {
        kind = 'DNS';
      } else if (errorMessage.toLowerCase().includes('ssl') || errorMessage.toLowerCase().includes('certificate') || errorMessage.toLowerCase().includes('tls')) {
        kind = 'TLS';
      }
      const error = { kind, message: `Node observation failed: ${errorMessage}` };
      return {
        node_id: node.node_id,
        status: 'unhealthy',
        state,
        timestamp: start,
        last_seen_at: node.last_seen_at ?? null,
        error
      };
    }

    if (response.status === 401 || response.status === 403) {
      const error: NonNullable<NodeHealthCheckResult['error']> = {
        kind: 'AUTH',
        message: `Node returned HTTP ${response.status} (Unauthorized)`
      };
      return {
        node_id: node.node_id,
        status: 'unhealthy',
        state: 'auth_failed',
        timestamp: start,
        last_seen_at: node.last_seen_at ?? null,
        error
      };
    }

    if (!response.ok) {
      const error: NonNullable<NodeHealthCheckResult['error']> = {
        kind: 'PROTOCOL',
        message: `Node returned HTTP status ${response.status}`
      };
      return {
        node_id: node.node_id,
        status: 'unhealthy',
        state: 'unreachable',
        timestamp: start,
        last_seen_at: node.last_seen_at ?? null,
        error
      };
    }

    const contentType = response.headers.get('content-type') || '';
    if (!contentType.includes('application/json')) {
      const error: NonNullable<NodeHealthCheckResult['error']> = {
        kind: 'PROTOCOL',
        message: `Node returned non-JSON content-type: ${contentType}`
      };
      return {
        node_id: node.node_id,
        status: 'unhealthy',
        state: 'incompatible',
        timestamp: start,
        last_seen_at: node.last_seen_at ?? null,
        error
      };
    }

    let payload: any;
    try {
      payload = await response.json();
    } catch (err: any) {
      const error: NonNullable<NodeHealthCheckResult['error']> = {
        kind: 'PROTOCOL',
        message: `Failed to parse JSON response: ${err?.message || String(err)}`
      };
      return {
        node_id: node.node_id,
        status: 'unhealthy',
        state: 'incompatible',
        timestamp: start,
        last_seen_at: node.last_seen_at ?? null,
        error
      };
    }

    if (!payload || typeof payload !== 'object') {
      const error: NonNullable<NodeHealthCheckResult['error']> = {
        kind: 'PROTOCOL',
        message: 'Node status response is not an object'
      };
      return {
        node_id: node.node_id,
        status: 'unhealthy',
        state: 'incompatible',
        timestamp: start,
        last_seen_at: node.last_seen_at ?? null,
        error
      };
    }

    const payloadVersion = typeof payload.version === 'string' ? payload.version : node.version;
    const payloadSchemaDigest = typeof payload.schema_digest === 'string'
      ? payload.schema_digest
      : typeof payload.identity?.schema_digest === 'string'
        ? payload.identity.schema_digest
        : node.schema_digest;
    const expectedVersion = majorMinor(node.version);
    const observedVersion = majorMinor(payloadVersion);
    if (expectedVersion === null || observedVersion === null || observedVersion !== expectedVersion || payloadSchemaDigest !== node.schema_digest) {
      const error = {
        kind: observedVersion !== expectedVersion ? ('VERSION' as const) : ('SCHEMA' as const),
        message: observedVersion !== expectedVersion
          ? `Incompatible node version: ${payloadVersion}. Expected ${expectedVersion ?? node.version}`
          : `Schema digest mismatch. Registered: ${node.schema_digest}, node reported: ${payloadSchemaDigest}`
      };
      const lastSeenAt = typeof payload.generated_at === 'string' ? payload.generated_at : observedAt;
      return {
        node_id: node.node_id,
        status: 'unhealthy',
        state: 'incompatible',
        timestamp: start,
        last_seen_at: lastSeenAt,
        error
      };
    }

    const generatedAt = typeof payload.generated_at === 'string' ? payload.generated_at : observedAt;
    const generatedMillis = parseIsoMillis(generatedAt);
    const state: NodeObservationState =
      generatedMillis !== null && start - generatedMillis > NODE_STALE_AFTER_MS ? 'stale' : 'healthy';
    const observedSnapshot: NodeObservationSnapshot = {
      node_id: node.node_id,
      display_name: typeof payload.profile?.display_name === 'string' ? payload.profile.display_name : node.display_name,
      advertised_url: node.advertised_url,
      version: payloadVersion,
      schema_digest: payloadSchemaDigest,
      state,
      observed_at: generatedAt,
      last_seen_at: generatedAt,
      last_observed_state: state,
      last_error_kind: null,
      last_error_message: null,
      profile: typeof payload.profile?.profile === 'string' ? payload.profile.profile : (typeof payload.profile === 'string' ? payload.profile : null),
      profiles: typeof payload.profile?.profile === 'string'
        ? [payload.profile.profile]
        : typeof payload.profile === 'string'
          ? [payload.profile]
          : [],
      backend_configured: payload.backend_configured && typeof payload.backend_configured === 'object'
        ? payload.backend_configured
        : {},
      backend_instances: Array.isArray(payload.backend_instances) ? payload.backend_instances : [],
      availability: Array.isArray(payload.availability) ? payload.availability : [],
      recent_ledger: payload.recent_ledger ?? null,
      active_claims: Array.isArray(payload.active_claims) ? payload.active_claims : [],
      active_work: dedupeNodeWorkItems(node.node_id, Array.isArray(payload.active_claims) ? payload.active_claims : []),
      event_cursor: typeof payload.event_cursor === 'string'
        ? payload.event_cursor
        : typeof payload.recent_ledger?.most_recent_dispatch_timestamp === 'string'
          ? payload.recent_ledger.most_recent_dispatch_timestamp
          : null,
      resource_pressure: normalizeResourcePressure(payload.resource_pressure),
      error: null
    };

    return {
      node_id: node.node_id,
      status: mapStateToResult(state),
      state,
      timestamp: start,
      last_seen_at: generatedAt,
      snapshot: observedSnapshot
    };
  }

  registerNode(node: RegisteredNode): { warnings: string[]; created: boolean } {
    const warnings: string[] = [];
    const existing = this.nodes.get(node.node_id);

    // 1. Basic validation
    if (!node.node_id || typeof node.node_id !== 'string') {
      throw new Error('Invalid or missing node_id');
    }
    if (!node.display_name || typeof node.display_name !== 'string') {
      throw new Error('Invalid or missing display_name');
    }
    if (!node.advertised_url || typeof node.advertised_url !== 'string') {
      throw new Error('Invalid or missing advertised_url');
    }
    if (!node.version || typeof node.version !== 'string') {
      throw new Error('Invalid or missing version');
    }
    if (!node.schema_digest || typeof node.schema_digest !== 'string') {
      throw new Error('Invalid or missing schema_digest');
    }
    const validTransportModes: RegisteredNode['transport_mode'][] = [
      'loopback',
      'authenticated_remote',
      'trusted_lan'
    ];
    if (!validTransportModes.includes(node.transport_mode)) {
      // Fail closed: an unrecognized value must never silently skip the
      // transport/TLS enforcement below.
      throw new Error(
        `Invalid transport_mode '${node.transport_mode}': must be one of ${validTransportModes.join(', ')}`
      );
    }

    // 2. Reject the central node registering itself (issue #944). The
    // liveness scheduler polls every registered node's advertised_url/api/status;
    // a node advertising the central's own endpoint makes the central poll
    // itself and recurse until timeout (observed live: /api/status and
    // /api/registry/fleet both 502). Loopback spellings are normalized so
    // "localhost:3773" and "127.0.0.1:3773" both trip this.
    if (this.selfUrl && this.listenerPort !== null && isCentralEndpoint(node.advertised_url, this.selfUrl, this.listenerPort)) {
      throw new Error(
        `Refusing to register: advertised_url '${node.advertised_url}' is this central node's own endpoint (would make the central poll itself). A worker must advertise its own reachable URL.`
      );
    }

    // 3. Reject endpoint collisions
    const newEndpoint = getEndpoint(node.advertised_url);
    for (const existingNode of this.nodes.values()) {
      if (existingNode.node_id === node.node_id) continue;
      if (getEndpoint(existingNode.advertised_url) === newEndpoint) {
        throw new Error(`Endpoint collision: ${node.advertised_url} collides with registered node ${existingNode.node_id}`);
      }
    }

    // 4. Reject incompatible schema
    if (!isSchemaCompatible(node.schema_digest)) {
      throw new Error(`Incompatible schema digest: ${node.schema_digest}`);
    }

    // 5. Reject secret-looking labels
    if (containsSecretWords(node.display_name)) {
      throw new Error(`Display name '${node.display_name}' contains secret-looking words`);
    }
    if (node.labels) {
      for (const label of node.labels) {
        if (containsSecretWords(label)) {
          throw new Error(`Label '${label}' contains secret-looking words`);
        }
      }
    }

    // 6. Registry config supports certificate/token secret references, not raw credentials
    if (!node.secret_ref || (!node.secret_ref.startsWith('env:') && !node.secret_ref.startsWith('file:'))) {
      throw new Error('Secret reference must use references (starting with env: or file:), not raw credentials');
    }

    // 7. Non-loopback endpoints require TLS plus authenticated node/client identity; localhost development remains explicit
    const loopback = isLoopback(node.advertised_url);
    if (!loopback) {
      if (node.transport_mode === 'loopback') {
        throw new Error('Non-loopback advertised URL cannot use loopback transport mode');
      }
      if (node.transport_mode === 'authenticated_remote') {
        const url = node.advertised_url.toLowerCase();
        if (!url.startsWith('https://') && !url.startsWith('wss://')) {
          throw new Error('Non-loopback authenticated remote endpoints must use TLS (https:// or wss://)');
        }
      } else if (node.transport_mode === 'trusted_lan') {
        // Issue #944: self-hosted tailnet/LAN workers legitimately advertise
        // a plain-HTTP URL (e.g. http://100.118.97.79). Mirror the Rust side's
        // GAH_COORDINATOR_INSECURE_TLS=1 opt-in: allow it only when the central
        // operator explicitly opts in, so the fail-closed default is unchanged.
        if (process.env.GAH_ALLOW_INSECURE_HTTP !== '1') {
          throw new Error(
            'Non-loopback trusted_lan endpoints require GAH_ALLOW_INSECURE_HTTP=1 on the central node (fail-closed default; use authenticated_remote over TLS otherwise)'
          );
        }
        warnings.push('Non-loopback trusted_lan endpoint accepted over plain HTTP (GAH_ALLOW_INSECURE_HTTP=1)');
      }
    } else {
      if (node.transport_mode === 'authenticated_remote') {
        const url = node.advertised_url.toLowerCase();
        if (!url.startsWith('https://') && !url.startsWith('wss://')) {
          warnings.push('Loopback authenticated remote endpoint does not terminate TLS locally');
        }
      }
    }

    // SECURITY (issue #951 review): UPDATING an existing node_id repoints
    // where the central polls (advertised_url) and how it authenticates
    // (secret_ref). authMiddleware alone cannot gate that -- a reverse-proxied
    // LAN peer appears loopback to Express and skips auth entirely -- so the
    // ownership check for existing nodes lives in the POST /api/registry/nodes
    // ROUTE (apps/server/src/server.ts), which requires the coordinator token
    // before registerNode is called for an existing node_id. This method has
    // exactly one caller today; if a second caller appears, it MUST preserve
    // that gate, or any caller that can reach registerNode with a known
    // node_id can hijack the node.
    //
    // By the time we're here a duplicate node_id is either a legitimate,
    // authenticated reconciliation or a brand-new node. Store the incoming
    // payload as-is: validation above requires every security-relevant field,
    // and a full replace (rather than a merge) means a re-registration can
    // never inherit stale profiles or secret_ref from an older partial
    // payload.
    this.nodes.set(node.node_id, node);
    this.observations.delete(node.node_id);
    this.save();
    this.changed();
    return { warnings, created: !existing };
  }

  revokeNode(nodeId: string): boolean {
    const deleted = this.nodes.delete(nodeId);
    if (deleted) {
      this.observations.delete(nodeId);
      this.observationRequests.delete(nodeId);
      this.save();
      this.changed();
    }
    return deleted;
  }

  rotateSecret(nodeId: string, secretRef: string): void {
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`Node ${nodeId} not found`);
    }
    if (!secretRef || (!secretRef.startsWith('env:') && !secretRef.startsWith('file:'))) {
      throw new Error('Secret reference must use references (starting with env: or file:), not raw credentials');
    }
    node.secret_ref = secretRef;
    this.save();
  }

  /** Run readiness on one declared worker profile, without changing fleet health or routing eligibility. */
  async checkNodeDoctor(nodeId: string, profile: string): Promise<DoctorSnapshot> {
    const node = this.nodes.get(nodeId);
    if (!node) throw new NodeDoctorError(404, 'Node is not registered.');
    if (!profile || !node.profiles?.includes(profile)) throw new NodeDoctorError(400, 'Select a profile declared by this worker. Configure or import a worker profile and register it first.');
    let headers: Record<string, string>;
    try { headers = nodeHeaders(node); }
    catch { throw new NodeDoctorError(502, 'AUTH: Cannot resolve the worker credential on central.'); }
    const url = new URL('/api/doctor', node.advertised_url);
    url.searchParams.set('profile', profile);
    try {
      const response = await fetchWithTimeout(url.toString(), headers, 30_000);
      if (response.status === 401 || response.status === 403) throw new NodeDoctorError(502, 'AUTH: Worker rejected its configured credential.');
      if (!response.ok) throw new NodeDoctorError(502, `UPSTREAM: Worker doctor returned HTTP ${response.status}.`);
      const snapshot: unknown = await response.json();
      if (!isDoctorSnapshot(snapshot, profile)) throw new NodeDoctorError(502, 'PROTOCOL: Worker returned an invalid or mismatched doctor result.');
      if (this.nodes.get(nodeId) !== node) throw new NodeDoctorError(409, 'Node registration changed during the check. Run readiness again.');
      return snapshot;
    } catch (error) {
      if (error instanceof NodeDoctorError) throw error;
      if (error instanceof Error && ['TimeoutError', 'AbortError'].includes(error.name)) throw new NodeDoctorError(504, 'TIMEOUT: Worker readiness did not finish within 30 seconds.');
      if (error instanceof SyntaxError) throw new NodeDoctorError(502, 'PROTOCOL: Worker doctor response was not JSON.');
      throw new NodeDoctorError(502, 'NETWORK: Cannot reach the registered worker doctor endpoint.');
    }
  }

  async checkNodeHealth(nodeId: string, profile?: string): Promise<NodeHealthCheckResult> {
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`Node ${nodeId} not found`);
    }
    const result = await this.pollNodeObservation(node, profile);
    if (!profile) this.changed();
    return result;
  }

  /** Starts the periodic liveness poll (issue #883). Idempotent -- calling
   * this while already running just restarts the interval with the new
   * value. `unref()` so a bare scheduler doesn't keep the process alive on
   * its own (the HTTP server's own listeners already do that). */
  startLivenessScheduler(intervalMs: number = LIVENESS_POLL_INTERVAL_MS): void {
    this.stopLivenessScheduler();
    this.livenessTimer = setInterval(() => {
      this.runLivenessCheck().catch((err) => console.error(`Node liveness check failed: ${err instanceof Error ? err.message : String(err)}`));
    }, intervalMs);
    this.livenessTimer.unref?.();
  }

  stopLivenessScheduler(): void {
    if (this.livenessTimer) {
      clearInterval(this.livenessTimer);
      this.livenessTimer = null;
    }
  }

  /** One liveness poll cycle: check every registered node, track
   * consecutive bad-state counts, and alert once per node the first time
   * it crosses the threshold (not on every subsequent bad check --
   * silenced again once it recovers, so a flapping node doesn't spam).
   * Exposed directly (not just via the interval) so tests can drive a
   * cycle deterministically instead of racing real timers. */
  async runLivenessCheck(): Promise<void> {
    const observations = await this.getNodeObservations();
    for (const obs of observations) {
      const isBad = LIVENESS_BAD_STATES.includes(obs.state);
      if (!isBad) {
        this.consecutiveBadChecks.delete(obs.node_id);
        this.alreadyAlerted.delete(obs.node_id);
        continue;
      }
      const count = (this.consecutiveBadChecks.get(obs.node_id) ?? 0) + 1;
      this.consecutiveBadChecks.set(obs.node_id, count);
      if (count >= LIVENESS_ALERT_AFTER_CONSECUTIVE_BAD_CHECKS && !this.alreadyAlerted.has(obs.node_id)) {
        this.alreadyAlerted.add(obs.node_id);
        sendLivenessAlert(
          `Node "${obs.display_name}" (${obs.node_id}) has been ${obs.state} for ${count} consecutive checks (last seen: ${obs.last_seen_at ?? 'never'}).`
        );
      }
    }
  }
}
