import type { ActiveClaim, AvailabilityScope, BackendInstanceSummary, RecentLedgerSummary } from './gah.js';

export interface RegisteredNode {
  node_id: string;
  display_name: string;
  advertised_url: string;
  version: string;
  schema_digest: string;
  labels?: string[];
  transport_mode: 'loopback' | 'authenticated_remote' | 'trusted_lan';
  secret_ref: string; // reference like "env:NODE_TOKEN" or "file:/path/to/token.txt"
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
