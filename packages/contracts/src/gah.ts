/**
 * Typed contracts for the `gah` CLI's JSON outputs (`status --json`,
 * `report --json`, `events --json`, `ledger work --json`).
 *
 * These mirror the Rust serde structs field-for-field (src/status.rs,
 * src/report.rs, src/events.rs, src/ledger.rs) rather than being loosely
 * typed as `any`/`unknown` -- the whole point of the frontend
 * productization pass is that "unknown" and "zero" must never be
 * conflated, and that only holds if the types actually distinguish
 * `T | null | undefined` from `T`.
 *
 * If you add a field on the Rust side, add it here too. If a field is
 * `Option<T>` in Rust, it's `T | null` here (serde_json serializes `None`
 * as `null`, never omits the key unless the struct uses
 * `skip_serializing_if`, in which case it's simply absent -- marked
 * optional with `?` below where that's the case).
 */

// ---------------------------------------------------------------------------
// gah status --json (src/status.rs)
// ---------------------------------------------------------------------------

export interface ProfileIdentity {
  profile: string;
  display_name: string;
  repo_id: string;
  provider: string;
  local_path: string;
  default_target_branch: string;
  /** Resolved per-repo merge policy (inherits canonical/defaults policy
   * when the profile doesn't set its own). */
  merge_policy: string;
}

export type ObservationStatusValue = 'ok' | 'error';

export interface ObservationStatus {
  status: ObservationStatusValue;
}

export interface Observations {
  sync: ObservationStatus;
  availability: ObservationStatus;
  ledger: ObservationStatus;
}

export type AvailabilityScopeKind = 'backend_wide' | 'model_specific' | 'quota_pool';

export interface AvailabilityScope {
  backend: string;
  model: string | null;
  /** Present only when this scope is a quota-pool-level observation. */
  quota_pool?: string | null;
  eligible_now: boolean;
  reason: string | null;
  unavailable_until: string | null;
  source: string | null;
  last_error_summary: string | null;
  observed_at: string | null;
  scope: AvailabilityScopeKind | null;
}

export interface RoutingCandidateDiagnostic {
  backend: string;
  model: string | null;
  quota_pool: string | null;
  default_order: number | null;
  consideration_order: number | null;
  pace_band: string | null;
  cost_class: string | null;
  skip_reason: string | null;
  unavailable_until: string | null;
}

export interface RoutingDiagnostics {
  policy_reordered_candidates: boolean;
  selected_backend: string | null;
  selected_model: string | null;
  selected_quota_pool: string | null;
  selected_pace_band: string | null;
  selected_cost_class: string | null;
  selected_over: string[];
  candidates: RoutingCandidateDiagnostic[];
  human_summary: string | null;
}

export interface RecentLedgerSummary {
  most_recent_dispatch_timestamp: string;
  most_recent_effective_backend: string;
  most_recent_effective_model: string | null;
  most_recent_work_id: string | null;
  most_recent_mode: string;
  most_recent_validation_result: string | null;
  most_recent_failure_class: string | null;
  most_recent_failure_stage: string | null;
  most_recent_branch: string | null;
  most_recent_mr_url: string | null;
  attempts_started: number | null;
  attempts_completed: number | null;
  human_required: boolean;
  routing_diagnostics?: RoutingDiagnostics | null;
}

export interface Blocker {
  kind: string;
  reason?: string | null;
  message?: string | null;
  backend?: string | null;
  model?: string | null;
  until?: string | null;
  source_reference?: string | null;
}

export interface StatusError {
  subsystem: string;
  message: string;
  incomplete_snapshot: boolean;
}

/** #[serde(rename_all = "SCREAMING_SNAKE_CASE")] on src/sync.rs's enum. */
export type RecommendedAction =
  | 'REUSE_BRANCH'
  | 'HUMAN_MERGE_DECISION'
  | 'RUN_REVIEW'
  | 'NONE'
  | 'INSPECT_BRANCH'
  | 'INSPECT_MANUALLY';

export interface MergeRequest {
  profile?: string | null;
  branch: string;
  work_id?: string | null;
  id: string | null;
  url: string | null;
  /** Human-readable PR/MR title (TICKET-198). */
  title?: string | null;
  state: string | null;
  draft: boolean;
  merge_status: string | null;
  merged: boolean;
  /** RFC3339 merge timestamp for merged MRs (TICKET-198). */
  merged_at?: string | null;
  ci_passed: boolean;
  /** Backend/model that produced the merge, joined from the ledger (TICKET-198). */
  effective_backend?: string | null;
  effective_model?: string | null;
  /** Review verdict recorded for the merge, joined from the ledger (TICKET-198). */
  review_verdict?: string | null;
  /** Deterministic reason GAH made a reviewer result non-mergeable. */
  review_gate_reason?: string | null;
  classification: string;
  recommended_action: RecommendedAction;
}

export interface AvailableTicket {
  ticket_path: string;
  work_id: string | null;
  title: string | null;
  recommended_backend: string | null;
  recommended_model: string | null;
  prior_attempt_count: number;
  last_failure_class: string | null;
  has_active_mr: boolean;
  human_required: boolean;
}

export interface StatusSnapshot {
  schema_version: number;
  generated_at: string;
  profile: ProfileIdentity;
  observations: Observations;
  merge_requests: MergeRequest[];
  availability: AvailabilityScope[];
  recent_ledger: RecentLedgerSummary | null;
  constraints: Blocker[];
  /** Genuine profile-wide blockers (sync failure, infra unavailable, no
   * viable route) that halt ALL work. A ticket-scoped human_required entry
   * does NOT appear here -- see `blocked_work_items`. Usually empty even
   * when work is blocked; check `blocked_work_items` for that. */
  blockers: Blocker[];
  /** Work items awaiting human action, scoped to the work item(s) they
   * affect -- other eligible work stays dispatchable. This is where a
   * ticket-level human_required review verdict shows up, NOT `blockers`. */
  blocked_work_items: Blocker[];
  errors: StatusError[];
  available_tickets: AvailableTicket[];
  fix_attempt_counts: Record<string, number>;
  merge_attempt_counts: Record<string, number>;
  publishing_allow_pr: boolean;
  /** TICKET-157: per-backend "configured for this profile" signal, keyed by
   * logical backend name. Only backends with a real Rust implementation are
   * present. A `true` value means the backend is set up for the active
   * profile (explicit path or profile marker). Backends with no
   * implementation are absent and must be shown as not_implemented. */
  backend_configured: Record<string, boolean>;
}

// ---------------------------------------------------------------------------
// gah quota snapshot --json (src/quota_snapshot.rs)
// ---------------------------------------------------------------------------

export interface QuotaUsageSummary {
  entries: number;
  attempts: number;
  validation_pass: number;
  success_rate: number | null;
  total_tokens: number | null;
  requests_count: number | null;
  actual_cost_usd: number | null;
  estimated_cost_usd: number | null;
}

export interface QuotaCandidateStatus {
  modes: string[];
  backend: string;
  model: string | null;
  quota_pool?: string | null;
  configured: boolean;
  eligible_now: boolean;
  reason?: string | null;
  unavailable_until?: string | null;
  source?: string | null;
  last_error_summary?: string | null;
  observed_at?: string | null;
  usage: QuotaUsageSummary;
  quota_observations: QuotaObservation[];
}

export interface QuotaSnapshot {
  schema_version: number;
  generated_at: string;
  profile: ProfileIdentity;
  since: string;
  usage: QuotaUsageSummary;
  candidates: QuotaCandidateStatus[];
}

// ---------------------------------------------------------------------------
// gah report --json (src/report.rs)
// ---------------------------------------------------------------------------

export interface QuotaObservation {
  backend: string;
  model?: string | null;
  quota_window?: string | null;
  quota_used_percent?: number | null;
  quota_remaining_percent?: number | null;
  quota_reset_at?: string | null;
  observed_at?: string | null;
  usage_source?: string | null;
}

export interface BackendModelComparison {
  backend_or_model: string;
  is_model: boolean;
  entries: number;
  attempts: number;
  validation_pass: number;
  /** Fraction 0..1, not a percent -- multiply by 100 for display. */
  success_rate: number;
  total_cost_usd: number | null;
  actual_cost_usd: number | null;
  estimated_cost_usd: number | null;
  average_cost_usd: number | null;
  average_duration_seconds: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cache_read_tokens: number | null;
  cache_write_tokens: number | null;
  total_tokens: number | null;
  requests_count: number | null;
  quota_observations: QuotaObservation[];
  /** [verdict, count] pairs, e.g. ["APPROVE_STRONG", 3]. */
  review_verdict_distribution: [string, number][];
}

export type ReportGroupBy = 'backend' | 'model';

export interface ReportData {
  ledger_path: string;
  total_entries: number;
  since: string;
  profile: string | null;
  group_by: string;
  comparisons: BackendModelComparison[];
  trend: ReportTrendPoint[];
}

export interface ReportTrendPoint {
  date: string;
  entries: number;
  validation_pass: number;
  total_tokens: number;
  actual_cost_usd: number | null;
  estimated_cost_usd: number | null;
}

// ---------------------------------------------------------------------------
// gah report --series --bucket daily --json (Issue #142)
// Time-bucketed usage/cost/success-rate series for the trend chart.
// ---------------------------------------------------------------------------

export interface ReportSeriesPoint {
  date: string;
  total_tokens: number;
  actual_cost_usd: number | null;
  estimated_cost_usd: number | null;
  success_rate: number;
}

export interface ReportSeriesData {
  ledger_path: string;
  since: string;
  bucket: string;
  profile: string | null;
  series: ReportSeriesPoint[];
}

// ---------------------------------------------------------------------------
// gah profile list --json (src/main.rs)
// ---------------------------------------------------------------------------

export interface ProfileSummary {
  name: string;
  display_name: string;
  provider: string;
  repo: string;
  local_path: string;
  /** Human-facing repo link (github.com/... or the gitlab host), null if
   * the provider isn't recognized or a self-hosted gitlab is missing
   * provider_api_base. */
  web_url: string | null;
}

// ---------------------------------------------------------------------------
// gah events --json (src/events.rs)
// ---------------------------------------------------------------------------

export type ControllerEventType =
  | 'observation_completed'
  | 'action_decided'
  | 'dispatch_started'
  | 'dispatch_finished'
  | 'backend_marked_unavailable'
  | 'wait_selected'
  | 'human_required'
  | 'duplicate_guard_triggered'
  | 'loop_stopped';

export interface ControllerEvent {
  timestamp: string;
  event_type: string;
  profile: string | null;
  work_id: string | null;
  run_id?: string | null;
  details: string;
}

export type ControllerActivityStatus = 'running' | 'finished' | 'failed';

export interface ControllerActivity {
  run_id: string;
  profile: string | null;
  work_id: string | null;
  started_at: string;
  finished_at: string | null;
  action: string;
  status: ControllerActivityStatus;
  outcome: string | null;
}

// ---------------------------------------------------------------------------
// gah ledger work <id> --json (src/ledger.rs LedgerEntry, full shape)
// ---------------------------------------------------------------------------

export interface LedgerUsage {
  usage_source: string | null;
  observed_at?: string | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cache_read_tokens: number | null;
  cache_write_tokens: number | null;
  total_tokens: number | null;
  requests_count: number | null;
  estimated_cost_usd: number | null;
  actual_cost_usd: number | null;
  quota_window: string | null;
  quota_used_percent: number | null;
  quota_remaining_percent: number | null;
  quota_reset_at: string | null;
}

/** TICKET-101: usage for exactly this attempt (not the whole dispatch). An
 * all-null `usage` means "backend didn't report it," never "zero usage." */
export interface AttemptRecord {
  attempt_number: number;
  backend: string;
  effective_model: string | null;
  exit_code: number | null;
  validation_result: string | null;
  failure_class: string | null;
  failure_stage: string | null;
  duration_seconds: number | null;
  diff_path: string | null;
  usage: LedgerUsage;
}

export interface LedgerEntry {
  timestamp: string;
  session_id: string | null;
  profile: string;
  display_name: string;
  repo_id: string;
  repo: string;
  local_path: string;
  provider: string;
  backend: string;
  requested_backend: string;
  effective_backend: string;
  requested_model: string | null;
  effective_model: string | null;
  routing_reason: string | null;
  fallback_used: boolean;
  confidence_impact: string | null;
  human_required: boolean;
  routing_diagnostics?: RoutingDiagnostics | null;
  mode: string;
  target_summary: string | null;
  work_id?: string | null;
  source_issue_number?: string | null;
  work_title?: string | null;
  branch: string | null;
  session_dir: string | null;
  duration_seconds: number | null;
  backend_exit_code: number | null;
  validation_result: string | null;
  review_verdict?: string | null;
  review_confidence?: string | null;
  reviewer_backend?: string | null;
  reviewer_model?: string | null;
  review_gate_reason?: string | null;
  commit_attempted: boolean;
  commit_created: boolean;
  push_attempted: boolean;
  push_succeeded: boolean;
  mr_attempted: boolean;
  mr_created: boolean;
  mr_url: string | null;
  files_changed: number | null;
  insertions: number | null;
  deletions: number | null;
  error_summary: string | null;
  failure_class?: string | null;
  failure_stage?: string | null;
  /** TICKET-064: retry-loop iterations entered vs. run to completion. */
  attempts_started?: number;
  attempts_completed?: number;
  /** TICKET-101: per-attempt backend/model/duration/usage, in order. */
  attempts?: AttemptRecord[];
  /** initial | post_review_repair | review | stuck_loop_gate */
  dispatch_reason?: string | null;
  usage: LedgerUsage;
}
