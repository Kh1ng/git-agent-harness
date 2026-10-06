import type { ActiveClaim, AvailabilityScope, BackendInstanceSummary, RecentLedgerSummary, QuotaSnapshot } from './gah.js';
import type { ClaimLease } from './claims.js';
import coordinatorProtocol from './coordinator-protocol.json' with { type: 'json' };

export const COORDINATOR_VERSION = coordinatorProtocol.version;
export const COORDINATOR_SCHEMA_SEED = coordinatorProtocol.schema_seed;
/** Issue #1416: the oldest worker version this coordinator still supports.
 * A worker below this is flagged `unsupported` in the fleet view instead of
 * failing silently; the hard major.minor compatibility rule is unchanged. */
export const COORDINATOR_MINIMUM_WORKER_VERSION = coordinatorProtocol.minimum_worker_version;

/** Numeric `major[.minor[.patch]]` comparison shared by the release feed,
 * the fleet version-skew check, and the release install path: -1 when `a`
 * sorts before `b`, 0 when equal, 1 when after. Non-numeric parts are
 * compared lexically as a final tiebreaker (edge build metadata). */
export function compareVersions(a: string, b: string): number {
  const parse = (value: string) => value
    .replace(/^v/i, '')
    .split(/[.+-]/)
    .map((part) => (/^\d+$/.test(part) ? Number(part) : part));
  const left = parse(a);
  const right = parse(b);
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const l = left[i];
    const r = right[i];
    if (l === r) continue;
    if (l === undefined) return -1;
    if (r === undefined) return 1;
    if (typeof l === 'number' && typeof r === 'number') return l < r ? -1 : 1;
    return String(l) < String(r) ? -1 : 1;
  }
  return 0;
}

/** Worker observations stay scoped to their source node; mirrored ledgers are not additive. */
export interface NodeQuotaRelay {
  nodeId: string;
  displayName: string;
  state: 'available' | 'unavailable' | 'unsupported_profile';
  quota: QuotaSnapshot | null;
  error?: string;
}

export interface FleetQuotaSnapshot {
  profile: string;
  since: string;
  nodes: NodeQuotaRelay[];
}

export interface RegisteredNode {
  node_id: string;
  display_name: string;
  advertised_url: string;
  version: string;
  schema_digest: string;
  labels?: string[];
  transport_mode: 'loopback' | 'authenticated_remote' | 'trusted_lan';
  secret_ref: string; // reference like "env:NODE_TOKEN" or "file:/path/to/token.txt"
  /** Profiles this node is declared to dispatch (issue #882: the central
   * claims API checks this before granting a lease -- a node can't claim
   * work under a profile it never declared). Unset/empty means none. */
  profiles?: string[];
  last_seen_at?: string | null;
  last_observed_state?: NodeObservationState | null;
  last_observed_at?: string | null;
  last_error_kind?: HealthCheckFailureKind | null;
  last_error_message?: string | null;
  /** Issue #1416: coordinator-driven release updates opt this node in to
   * automatic updates whenever the fleet sees it behind the coordinator. */
  auto_update?: boolean;
}

/** Issue #1416: how a registered node's version relates to this
 * coordinator. Computed coordinator-side so every client sees the same
 * verdict instead of re-deriving it from raw versions. */
export type NodeVersionStatus = 'current' | 'behind' | 'unsupported';

export interface NodeUpdateInfo {
  status: NodeVersionStatus;
  node_version: string;
  coordinator_version: string;
  minimum_worker_version: string;
}

export interface NodeSummary {
  node_id: string;
  display_name: string;
  advertised_url: string;
  version: string;
  schema_digest: string;
  labels?: string[];
  profiles?: string[];
  transport_mode: 'loopback' | 'authenticated_remote' | 'trusted_lan';
  last_seen_at?: string | null;
  last_observed_state?: NodeObservationState | null;
  last_observed_at?: string | null;
  last_error_kind?: HealthCheckFailureKind | null;
  last_error_message?: string | null;
  auto_update?: boolean;
  update?: NodeUpdateInfo | null;
}

export type NodeObservationState =
  | 'healthy'
  | 'stale'
  | 'unreachable'
  | 'auth_failed'
  | 'incompatible';

export type HealthCheckFailureKind =
  | 'DNS'
  | 'NETWORK'
  | 'TLS'
  | 'AUTH'
  | 'PROTOCOL'
  | 'VERSION'
  | 'SCHEMA';

export interface NodeResourcePressure {
  cpu_percent: number | null;
  rss_bytes: number | null;
  disk_percent: number | null;
}

export interface NodeQualifiedWorkIdentity {
  node_id: string;
  work_id: string;
  node_qualified_work_id: string;
  scope: string;
  hostname: string;
  claimed_at: string;
  age_seconds: number;
}

export interface NodeObservationError {
  kind: HealthCheckFailureKind;
  message: string;
}

export interface NodeObservationSnapshot {
  node_id: string;
  display_name: string;
  advertised_url: string;
  version: string;
  schema_digest: string;
  state: NodeObservationState;
  observed_at: string;
  last_seen_at: string | null;
  last_observed_state: NodeObservationState | null;
  last_error_kind: HealthCheckFailureKind | null;
  last_error_message: string | null;
  profile: string | null;
  profiles: string[];
  backend_configured: Record<string, boolean>;
  backend_instances: BackendInstanceSummary[];
  availability: AvailabilityScope[];
  recent_ledger: RecentLedgerSummary | null;
  active_claims: ActiveClaim[];
  active_work: NodeQualifiedWorkIdentity[];
  event_cursor: string | null;
  resource_pressure: NodeResourcePressure;
  error?: NodeObservationError | null;
  /** The worker's latest login checks (#1271); absent from older workers. */
  auth_health?: NodeAuthHealth | null;
}

export type AuthState = 'ok' | 'expired' | 'missing' | 'unknown' | 'error';

/** One login on one node: a backend, or a provider behind it. */
export interface AuthProbe {
  backend: string;
  /** Declared backend instance this login belongs to (#1352), or null for
   * the node's shared default login. */
  backend_instance?: string | null;
  provider: string | null;
  state: AuthState;
  /** Repository CLI package presence, independent of authentication. */
  installed?: boolean;
  /** A fixed explanation chosen by GAH; never provider output. */
  detail?: string;
  /** probe: `gah auth-health`; dispatch: a failed dispatch attempt; chat: a failed chat turn. */
  source: 'probe' | 'dispatch' | 'chat';
  /** When this login entered its current state, as the node saw it. */
  since?: string;
}

export interface NodeAuthHealth {
  checked_at: string;
  probes: AuthProbe[];
}

/** A login row as central reports it across the fleet. */
export interface AuthHealthRow extends AuthProbe {
  node_id: string;
  node_name: string;
}

export interface NodeHealthCheckResult {
  node_id: string;
  status: 'healthy' | 'unhealthy';
  state: NodeObservationState;
  timestamp: number;
  last_seen_at: string | null;
  snapshot?: NodeObservationSnapshot | null;
  error?: {
    kind: HealthCheckFailureKind;
    message: string;
  };
}

export interface RegistryConfig {
  nodes: RegisteredNode[];
}

/** Cached scheduler observations; absent observations have unknown health. */
export interface FleetSnapshot {
  nodes: NodeSummary[];
  observations: NodeObservationSnapshot[];
  leases: ClaimLease[];
}

// ---------------------------------------------------------------------------
// Issue #1416: coordinator-driven worker updates. The coordinator asks a
// worker to update (the worker pulls the same release artifact the central
// installs); a worker that is mid-dispatch finishes its run before the
// update restarts anything, which is why `waiting` exists between `armed`
// and `running`.
// ---------------------------------------------------------------------------

export type WorkerUpdateState =
  | 'idle'
  | 'waiting'
  | 'running'
  | 'success'
  | 'inferred_restart'
  | 'failed';

export interface WorkerUpdateStatus {
  status: WorkerUpdateState;
  current_version: string;
  target_version: string | null;
  armed_at: string | null;
  started_at: string | null;
  finished_at: string | null;
  /** Live claim count while `waiting`; null once the update is running. */
  active_dispatches: number | null;
  output: string;
}

/** One node's outcome from POST /api/registry/fleet/update-all. */
export interface FleetUpdateResult {
  node_id: string;
  display_name: string;
  started: boolean;
  status: WorkerUpdateStatus | null;
  error: string | null;
}

/** How a broken login is repaired from another device (#1272). */
export type LoginRepairMethod =
  /** The CLI prints a verification URL and a one-time code (codex). */
  | 'device_cli'
  /** GAH runs GitHub's device flow and stores the token (gh, opencode Copilot). */
  | 'github_device'
  /** The CLI prints a URL, then reads a code pasted back (claude). */
  | 'paste_code'
  /** The operator pastes an API key; the node stores it (opencode providers, Mistral, Nous). */
  | 'api_key'
  /** Only a terminal on that machine can do it. */
  | 'manual';

/** Official installation guides for the repository tools GAH runs. */
export function repositoryCli(program: string): { label: string; installUrl: string } | null {
  if (program === 'gh') return { label: 'GitHub CLI', installUrl: 'https://cli.github.com/' };
  if (program === 'glab') return { label: 'GitLab CLI', installUrl: 'https://gitlab.com/gitlab-org/cli#installation' };
  return null;
}

/** One rule for web and server: which method repairs a login. */
export function loginRepairMethod(login: { backend: string; provider: string | null }): LoginRepairMethod {
  if (login.backend === 'codex' && login.provider === null) return 'device_cli';
  if (login.backend === 'claude' && login.provider === null) return 'paste_code';
  if (login.backend === 'gh' && login.provider === 'github') return 'github_device';
  if (login.backend === 'opencode' && login.provider === 'github-copilot') return 'github_device';
  if (login.backend === 'opencode' && login.provider !== null && /^[a-z0-9][a-z0-9-]{0,63}$/.test(login.provider)) return 'api_key';
  if (login.backend === 'api' && (login.provider === 'mistral' || login.provider === 'nous')) return 'api_key';
  return 'manual';
}

/** What a device may see of a login repair: never CLI output, only the link,
 * the one-time code, fixed prompt text, and the outcome. */
export type LoginRepairState =
  | { status: 'install_required'; install_url: string }
  | { status: 'starting' }
  | { status: 'open_url'; url: string; code: string | null }
  | { status: 'awaiting_input'; url: string | null; prompt: string; secret: boolean }
  | { status: 'waiting' }
  | { status: 'succeeded' }
  | { status: 'failed'; reason: string }
  | { status: 'expired' }
  | { status: 'manual'; instructions: string };

export type LoginRepairView = LoginRepairState & {
  id: string;
  node_id: string;
  backend: string;
  provider: string | null;
  expires_at: string;
};
