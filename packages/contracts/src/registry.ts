import type { ActiveClaim, AvailabilityScope, BackendInstanceSummary, RecentLedgerSummary, QuotaSnapshot } from './gah.js';
import type { ClaimLease } from './claims.js';
import coordinatorProtocol from './coordinator-protocol.json' with { type: 'json' };

export const COORDINATOR_VERSION = coordinatorProtocol.version;
export const COORDINATOR_SCHEMA_SEED = coordinatorProtocol.schema_seed;

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
