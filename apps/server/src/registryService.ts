import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname, sep } from 'node:path';
import crypto from 'node:crypto';
import type {
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
    const port = url.port || (url.protocol === 'https:' || url.protocol === 'wss:' ? '443' : '80');
    return `${url.hostname}:${port}`;
  } catch {
    return urlStr;
  }
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

function nowIso(ms: number = Date.now()): string {
  return new Date(ms).toISOString();
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

async function fetchWithTimeout(url: string, headers: Record<string, string>, timeoutMs: number): Promise<Response> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, {
      headers,
      signal: controller.signal
    });
  } finally {
    clearTimeout(timeout);
  }
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
  private configPath: string;
  private nodes: Map<string, RegisteredNode> = new Map();

  constructor(configPath?: string) {
    this.configPath = configPath || process.env.GAH_REGISTRY_CONFIG_PATH || resolve(process.cwd(), 'config/registry-config.json');
    this.load();
  }

  private load() {
    if (existsSync(this.configPath)) {
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

  async getNodeObservations(profile?: string): Promise<NodeObservationSnapshot[]> {
    const nodes = this.getNodes();
    return mapWithConcurrency(nodes, NODE_POLL_CONCURRENCY, async (node) => {
      const result = await this.pollNodeObservation(node, profile);
      return result.snapshot ?? emptyNodeObservation(node, nowIso(result.timestamp), result.state, result.last_seen_at ?? null, result.error ?? null);
    });
  }

  private persistObservation(
    nodeId: string,
    observedAt: string,
    state: NodeObservationState,
    lastSeenAt: string | null,
    error?: { kind: string; message: string } | null
  ): void {
    const node = this.nodes.get(nodeId);
    if (!node) return;
    node.last_observed_at = observedAt;
    node.last_observed_state = state;
    node.last_seen_at = lastSeenAt ?? node.last_seen_at ?? null;
    node.last_error_kind = error?.kind as RegisteredNode['last_error_kind'];
    node.last_error_message = error?.message ?? null;
    this.save();
  }

  private async pollNodeObservation(node: RegisteredNode, profile?: string): Promise<NodeHealthCheckResult> {
    const start = Date.now();
    const observedAt = nowIso(start);
    const snapshotUrl = new URL('/api/status', node.advertised_url);
    if (profile) {
      snapshotUrl.searchParams.set('profile', profile);
    }

    const headers: Record<string, string> = {
      Accept: 'application/json',
      'User-Agent': 'GAH-Coordinator/0.1.0'
    };

    if (node.transport_mode === 'authenticated_remote') {
      let token = '';
      try {
        token = resolveSecret(node.secret_ref);
      } catch (e: any) {
        const error: NonNullable<NodeHealthCheckResult['error']> = {
          kind: 'AUTH',
          message: `Failed to resolve secret reference: ${e.message}`
        };
        this.persistObservation(node.node_id, observedAt, 'auth_failed', null, error);
        return {
          node_id: node.node_id,
          status: 'unhealthy',
          state: 'auth_failed',
          timestamp: start,
          last_seen_at: node.last_seen_at ?? null,
          error
        };
      }
      headers.Authorization = `Bearer ${token}`;
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
      this.persistObservation(node.node_id, observedAt, state, node.last_seen_at ?? null, error);
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
      this.persistObservation(node.node_id, observedAt, 'auth_failed', null, error);
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
      this.persistObservation(node.node_id, observedAt, 'unreachable', null, error);
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
      this.persistObservation(node.node_id, observedAt, 'incompatible', null, error);
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
      this.persistObservation(node.node_id, observedAt, 'incompatible', null, error);
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
      this.persistObservation(node.node_id, observedAt, 'incompatible', null, error);
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
      this.persistObservation(node.node_id, observedAt, 'incompatible', lastSeenAt, error);
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

    this.persistObservation(node.node_id, observedAt, state, generatedAt, null);
    return {
      node_id: node.node_id,
      status: mapStateToResult(state),
      state,
      timestamp: start,
      last_seen_at: generatedAt,
      snapshot: observedSnapshot
    };
  }

  registerNode(node: RegisteredNode): { warnings: string[] } {
    const warnings: string[] = [];

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

    // 2. Reject duplicate IDs
    if (this.nodes.has(node.node_id)) {
      throw new Error(`Duplicate node ID: ${node.node_id} is already registered`);
    }

    // 3. Reject endpoint collisions
    const newEndpoint = getEndpoint(node.advertised_url);
    for (const existingNode of this.nodes.values()) {
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
        throw new Error('Non-loopback advertised URL cannot use trusted_lan transport mode; use authenticated_remote with TLS');
      }
    } else {
      if (node.transport_mode === 'authenticated_remote') {
        const url = node.advertised_url.toLowerCase();
        if (!url.startsWith('https://') && !url.startsWith('wss://')) {
          warnings.push('Loopback authenticated remote endpoint does not terminate TLS locally');
        }
      }
    }

    this.nodes.set(node.node_id, node);
    this.save();
    return { warnings };
  }

  revokeNode(nodeId: string): boolean {
    const deleted = this.nodes.delete(nodeId);
    if (deleted) {
      this.save();
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

  async checkNodeHealth(nodeId: string, profile?: string): Promise<NodeHealthCheckResult> {
    const node = this.nodes.get(nodeId);
    if (!node) {
      throw new Error(`Node ${nodeId} not found`);
    }
    return this.pollNodeObservation(node, profile);
  }
}
