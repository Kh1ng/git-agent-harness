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
  max_fix_attempts_per_mr: number;
  max_implementation_failures_per_ticket: number;
  max_open_managed_mrs: number;
  /** Resolved per-repo merge policy (inherits canonical/defaults policy
   * when the profile doesn't set its own). */
  merge_policy: string;
  /** Effective issue intake policy for this profile. */
  issue_intake_policy: IssueIntakePolicy;
}

export interface IssueIntakePolicy {
  mode: string;
  canonical_autonomous_label: string;
  trusted_human_authors: string[];
  trusted_bot_authors: string[];
  github_issue_author_allowlist: string[];
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
  review_timeout_class: string | null;
  review_idle_timeout_seconds: number | null;
  review_hard_timeout_seconds: number | null;
  review_last_progress_secs: number | null;
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
  /** TICKET-505: stable reason code for HumanRequired blockers. */
  reason_code?: string | null;
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
  ci_pending: boolean;
  /** Backend/model that produced the merge, joined from the ledger (TICKET-198). */
  effective_backend?: string | null;
  effective_model?: string | null;
  /** Review verdict recorded for the merge, joined from the ledger (TICKET-198). */
  review_verdict?: string | null;
  /** Deterministic reason GAH made a reviewer result non-mergeable. */
  review_gate_reason?: string | null;
  source_sha?: string | null;
  review_contract_version: number;
  review_generation?: string | null;
  review_generation_status?: string | null;
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
  genuine_agent_failure_count: number;
  last_failure_class: string | null;
  has_active_mr: boolean;
  has_active_claim: boolean;
  human_required: boolean;
  human_required_reason_code?: string | null;
}

export interface IssueIntakeRejection {
  ticket_path: string;
  work_id: string | null;
  title: string | null;
  provider: string;
  author_login: string | null;
  author_kind: string | null;
  reason_code: string;
  reason: string;
  labels: string[];
}

export interface DependencyObservation {
  identity: string;
  provider: string;
  provider_state: string | null;
  normalized_state: 'open' | 'closed' | 'unknown' | 'missing' | 'inaccessible';
}

export interface DependencyBlocker {
  ticket_path: string;
  work_id: string;
  title: string;
  reason_code: string;
  reason: string;
  dependencies: DependencyObservation[];
}

export interface ActiveClaim {
  work_id: string;
  pid: number;
  scope: string;
  hostname: string;
  claimed_at: string;
  age_seconds: number;
}

export interface PmParentStatus {
  work_id: string;
  source_issue_number: string;
  plan_fingerprint: string;
  child_issue_numbers: string[];
  open_child_count: number;
  completed: boolean;
  reconciled: boolean;
}

export interface StatusSnapshot {
  schema_version: number;
  review_contract_version: number;
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
  /** Issue intake rejections observed during recurring discovery. */
  issue_intake_rejections: IssueIntakeRejection[];
  /** Native issues excluded by unresolved canonical prerequisites. Optional
   * while status schema v1 clients may still be connected to an older CLI. */
  dependency_blockers?: DependencyBlocker[];
  errors: StatusError[];
  available_tickets: AvailableTicket[];
  active_claims: ActiveClaim[];
  /** Published PM parents and the current provider-native state of their
   * exact child issue identities. */
  pm_parent_states: PmParentStatus[];
  /** Failed PM planning/publication attempts, keyed by native work ID. */
  pm_decomposition_attempt_counts: Record<string, number>;
  /** Effective bounded retry ceiling for PM decomposition. */
  pm_max_attempts: number;
  fix_attempt_counts: Record<string, number>;
  merge_attempt_counts: Record<string, number>;
  /** Work IDs currently under an out-of-band manager review hold. These
   * remain blocked from automatic review/merge until explicitly released. */
  review_held_work_ids: string[];
  publishing_allow_pr: boolean;
  /** Effective profile policy used to reject newly tracked generated files
   * before commit/push. */
  generated_artifact_deny_patterns: string[];
  max_parallel_workers: number;
  open_managed_mr_count: number;
  inflight_implementation_count: number;
  implementation_intake_paused: boolean;
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
  reasoning_tokens?: number | null;
  cache_read_tokens: number | null;
  cache_write_tokens: number | null;
  total_tokens: number | null;
  requests_count: number | null;
  tokens_per_success: number | null;
  requests_per_success: number | null;
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

/** Values accepted for a profile's `manager_wake_autonomy`. Mirrors
 * `WakeAutonomy` in src/config.rs (serde snake_case). */
export type WakeAutonomyValue = 'off' | 'review_only' | 'full';

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
  /** Max concurrent tickets `gah loop` may run for this profile (null =
   * unset, which the harness treats as 1). */
  max_parallel_workers: number | null;
  /** Effective maximum open managed PRs/MRs for the profile. */
  max_open_managed_mrs: number;
  /** Manager-wake autonomy for this profile (null = unset -> off). */
  manager_wake_autonomy: WakeAutonomyValue | null;
  /** Effective validation command timeout in seconds for this profile (defaults
   * to 300). If unset in TOML, this is computed and returned as the effective
   * timeout. */
  validation_timeout_seconds: number;
}

// ---------------------------------------------------------------------------
// gah config show --json (src/main.rs) -- global defaults
// ---------------------------------------------------------------------------

export interface ConfigSummary {
  /** Which agent CLI is currently acting as the operator's manager across
   * all profiles/projects (null = unset, so no manager wake happens). */
  current_manager: string | null;
}

export interface RoutingCandidateSummary {
  backend: string;
  model: string | null;
  quota_pool: string | null;
  priority: number;
  included_in_quota: boolean;
  marginal_cost_usd: number | null;
  quota_usage_percent: number | null;
  quota_days_remaining: number | null;
  requires_approval: boolean;
}

export interface ContextOverrideBudgetSummary {
  enabled?: boolean | null;
  soft_limit_tokens?: number | null;
  hard_limit_tokens?: number | null;
  compact_after_tool_calls?: number | null;
  fresh_context_on_review?: boolean | null;
  fresh_context_on_fix?: boolean | null;
  include_full_git_history?: boolean | null;
  include_full_worker_transcript_in_review?: boolean | null;
  recent_history_tokens?: number | null;
}

export interface ContextBudgetSummary {
  enabled: boolean;
  soft_limit_tokens: number;
  hard_limit_tokens: number;
  compact_after_tool_calls: number;
  fresh_context_on_review: boolean;
  fresh_context_on_fix: boolean;
  include_full_git_history: boolean;
  include_full_worker_transcript_in_review: boolean;
  recent_history_tokens: number;
}

export interface ConfigBackendContextSummary {
  backend: string;
  effective: ContextBudgetSummary;
  backend_override: ContextOverrideBudgetSummary | null;
}

export interface ConfigProfileContextSummary {
  global: ContextBudgetSummary;
  profile_override: ContextOverrideBudgetSummary | null;
  /** Effective context budget for every backend this profile actually
   * routes to (pm/improve/review candidates, routine reviewer, escalatory
   * reviewers). `context.backends.<name>` overrides are merged in
   * per-backend, so different routed backends for the same profile can have
   * different effective budgets -- this is what dispatch actually applies. */
  effective_by_backend: ConfigBackendContextSummary[];
}

export interface TaskRoutingRuleSummary {
  modes: string[];
  task_classes: string[];
  difficulties: string[];
  risks: string[];
  candidates: RoutingCandidateSummary[];
}

export interface NotificationSummary {
  configured: boolean;
  /** Secret-safe transport classification; the command itself is never sent. */
  transport: 'telegram' | 'custom_command' | null;
  manager_wake_autonomy: 'off' | 'review_only' | 'full';
  /** Paths are configuration metadata only; file contents are never sent. */
  env_file: string | null;
  env_file_prod: string | null;
}

/** Effective read-only profile configuration for Settings’ "effective config"
 * view. Values reflect inheritance through defaults + canonical + repo config
 * for the requested profile. */
export interface ConfigProfileSummary {
  profile: string;
  merge_policy: string;
  max_fix_attempts_per_mr: number;
  max_implementation_failures_per_ticket: number;
  max_review_cycles_per_ticket: number;
  max_paid_reviews_per_ticket: number;
  pm_candidates: RoutingCandidateSummary[];
  improve_candidates: RoutingCandidateSummary[];
  review_candidates: RoutingCandidateSummary[];
  task_routing_rules: TaskRoutingRuleSummary[];
  routine_reviewer: RoutingCandidateSummary | null;
  escalatory_reviewers: RoutingCandidateSummary[];
  context: ConfigProfileContextSummary;
  notifications: NotificationSummary;
}

/** Versioned allowlisted response from `gah config show --json --full`. */
export interface ConfigShowFull {
  schema_version: number;
  config_path: string;
  current_manager: string | null;
  profiles: Record<string, ConfigProfileSummary>;
}

/** Payload for `gah config set` (POST /api/config). `current_manager: null`
 * clears the field. */
export interface ConfigSetData {
  current_manager?: string | null;
  /** Field names to clear (e.g. "current_manager"). */
  clear?: string[];
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
  reason_code?: string | null;
  details: string;
}

// TICKET-505: HumanRequired reason codes
export type HumanRequiredReasonCode =
  | 'policy_approval'
  | 'retry_budget_exhausted'
  | 'review_evidence_gate'
  | 'merge_policy'
  | 'publishing_restriction'
  | 'configuration_infra'
  | 'fix_retry_cap_exceeded'
  | 'merge_retry_cap_exceeded'
  | 'unknown';

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
  usage_classification?: 'quota_backed' | 'api_key_backed' | 'local_unmetered' | 'unknown' | 'mixed' | 'mixed_or_unknown' | null;
  /** Safe logical execution instance, optionally qualified by quota pool. */
  backend_instance?: string | null;
  /** Model provider; distinct from LedgerEntry.provider (GitHub/GitLab). */
  provider?: string | null;
  actual_model?: string | null;
  actual_model_unknown_reason?: string | null;
  provider_unknown_reason?: string | null;
  account_label?: string | null;
  pricing_source?: string | null;
  pricing_version?: string | null;
  cost_unknown_reason?: string | null;
  observed_at?: string | null;
  input_tokens: number | null;
  output_tokens: number | null;
  reasoning_tokens?: number | null;
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
  token_usage_unknown_reason?: string | null;
  quota_unknown_reason?: string | null;
  /**
   * Issue #119: provenance-aware per-attempt behavior metrics (tool calls,
   * shell calls, file edits, test runs). `null`/`undefined` means the backend
   * did not report it (unknown) — never a real zero.
   */
  behavior_metrics?: AttemptBehaviorMetrics | null;
}

/** Issue #119: how a behavior metric count was obtained. */
export type BehaviorMetricQuality =
  | 'provider_reported'
  | 'structured_event_derived'
  | 'estimated'
  | 'unavailable';

/** Issue #119: a single per-attempt behavior metric with explicit provenance. */
export interface BehaviorMetric {
  /** Known count (`null` = unknown / not reported). */
  count: number | null;
  /** How this count was obtained. */
  quality: BehaviorMetricQuality;
  /** Why the count is unknown when `count` is `null` and quality is `unavailable`. */
  unknown_reason?: string | null;
}

/** Issue #119: normalized per-attempt behavior metrics with provenance. */
export interface AttemptBehaviorMetrics {
  tool_calls?: BehaviorMetric | null;
  shell_calls?: BehaviorMetric | null;
  file_edits?: BehaviorMetric | null;
  test_runs?: BehaviorMetric | null;
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
  human_required_reason_code?: string | null;
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
  review_contract_version?: number | null;
  review_generation?: string | null;
  review_timeout_class?: string | null;
  review_idle_timeout_seconds?: number | null;
  review_hard_timeout_seconds?: number | null;
  review_last_progress_secs?: number | null;
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
