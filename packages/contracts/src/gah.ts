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

import type { NodeObservationSnapshot, NodeResourcePressure } from './registry.js';

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

/** Issue #230: health of the automatic post-attempt telemetry export
 * pipeline. Distinguishes never-run, healthy, stale (no recent success),
 * retrying (a recent failure within its own retry budget), and failed
 * (retries exhausted, needs operator attention). */
export type ExportHealthStatusKind = 'never_run' | 'healthy' | 'stale' | 'retrying' | 'failed';

export interface ExportHealth {
  status: ExportHealthStatusKind;
  schema_version: number;
  last_attempt_at: string | null;
  last_success_at: string | null;
  last_error: string | null;
  last_error_class: string | null;
  /** Latest ledger entry timestamp reflected in a successful export. */
  exported_watermark: string | null;
  /** Cumulative count of telemetry records ever exported. */
  record_count: number;
  /** True when the most recent export attempt failed and a retry is owed. */
  retry_pending: boolean;
}

// Issue #966 (#863 gap 2): per-backend-instance skill inventory -- what GAH
// intends bound vs. what a backend instance last self-reported having.

export interface SkillDrift {
  /** Bound in GAH but missing from the backend's self-reported set. */
  bound_not_observed: string[];
  /** Present on the backend but not bound in GAH -- the #863 hand-edit
   * scenario (an operator reconfigured a backend's skills out-of-band). */
  observed_not_bound: string[];
}

export interface SkillInventoryView {
  backend: string;
  backend_instance?: string;
  /** Absent means GAH could not resolve what's bound for this instance
   * (store/registry failure), never an empty list -- an instance that was
   * actually resolved and genuinely has nothing bound serializes as `[]`. */
  bound_skill_ids?: string[];
  /** Absent means the backend could not self-report (unknown), never an
   * empty list -- a backend that was actually asked and confirmed zero
   * skills serializes as `[]`. */
  observed_skill_ids?: string[];
  observed_at?: string;
  observation_age_seconds?: number;
  /** `true` means the dashboard must not present this observation as live
   * truth (same #741 discipline as cached quota data). Absent when there is
   * no observation yet to judge freshness of. */
  observation_stale?: boolean;
  drift?: SkillDrift;
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
  /** Deterministic operator remediation plan for this blocked work item. */
  remediation_plan?: RemediationPlan;
}

export type RemediationAuthority =
  | 'operator'
  | 'paid_route_approver'
  | 'human_reviewer'
  | 'merge_approver'
  | 'profile_maintainer';

export type RemediationActionKind =
  | 'command'
  | 'api_action'
  | 'manual_review'
  | 'manual_merge'
  | 'config_change'
  | 'inspect';

export interface RemediationAction {
  kind: RemediationActionKind;
  summary: string;
  command?: string | null;
  api_action?: string | null;
}

export type RemediationPlan =
  | {
      result: 'plan';
      profile: string;
      work_id?: string | null;
      reference?: string | null;
      reason_code: HumanRequiredReasonCode;
      required_authority: RemediationAuthority;
      safe_actions: RemediationAction[];
    }
  | {
      result: 'no_automatic_remediation';
      profile: string;
      work_id?: string | null;
      reference?: string | null;
      reason_code: HumanRequiredReasonCode;
      required_authority: RemediationAuthority;
      safe_actions: RemediationAction[];
      reason: string;
    };

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
  /** Immutable PR/MR head commit used for review identity. */
  source_sha?: string | null;
  /** Provider-created commit that landed a merged PR/MR. */
  merge_commit_sha?: string | null;
  review_contract_version: number;
  review_generation?: string | null;
  review_generation_status?: string | null;
  classification: string;
  recommended_action: RecommendedAction;
}

export interface AvailableTicket {
  ticket_path: string;
  work_id: string | null;
  normalized_work_identity: string;
  source: CandidateSource;
  execution_policy: CandidateExecutionPolicy;
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

export type CandidateSource = 'legacy_ticket' | 'github_issue' | 'gitlab_issue';

export interface CandidateExecutionPolicy {
  intake_mode: string;
  explicit_autonomy_required: boolean;
  autonomous_metadata_present: boolean;
  dispatchable_now: boolean;
  exclusion_reason_code: string | null;
  exclusion_reason: string | null;
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
  /** "body" (canonical `Blocked by:` line) | "github_sub_issue" | "gitlab_blocks_link" */
  provenance: string | null;
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
  /** Per-node observations aggregated by the coordinator. Optional for
   * standalone CLI callers that only want the local node snapshot. */
  nodes?: NodeObservationSnapshot[];
  /** Local node resource pressure (best-effort; nulls preserve unknowns). */
  resource_pressure?: NodeResourcePressure | null;
  /** Event replay cursor for the local node's controller event stream. */
  event_cursor?: string | null;
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
  /** Effective normalized instance identities. Optional while schema-v1
   * clients may still be connected to an older CLI. */
  backend_instances?: BackendInstanceSummary[];
  /** Issue #230: automatic post-attempt telemetry export health. Optional
   * while schema-v1 clients may still be connected to an older CLI. */
  export_health?: ExportHealth;
  /** Issue #966 (#863 gap 2): per-backend-instance skill inventory. Optional
   * while schema-v1 clients may still be connected to an older CLI. */
  skill_inventory?: SkillInventoryView[];
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
  quota_observations?: QuotaObservation[];
}

export interface QuotaCheck {
  backend: string;
  checked_at: string;
  status: 'data' | 'no_data' | 'failed';
  error?: string | null;
}

export interface QuotaSnapshot {
  schema_version: number;
  generated_at: string;
  freshness: {
    ledger_observed_at?: string | null;
    availability_observed_at?: string | null;
    quota_checked_at?: string | null;
    quota_observed_at?: string | null;
  };
  quota_checks: QuotaCheck[];
  profile: ProfileIdentity;
  since: string;
  usage: QuotaUsageSummary;
  candidates: QuotaCandidateStatus[];
}

// ---------------------------------------------------------------------------
// gah doctor --json (src/doctor.rs)
// ---------------------------------------------------------------------------

export type DoctorCheckStatus = 'ok' | 'warn' | 'fail';

export interface DoctorCheck {
  profile?: string | null;
  name: string;
  status: DoctorCheckStatus;
  detail: string;
}

export interface DoctorSnapshot {
  schema_version: number;
  generated_at: string;
  overall_status: DoctorCheckStatus;
  checks: DoctorCheck[];
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
  memory_gateway_capture_l0_recorded: number | null;
  requests_count: number | null;
  tokens_per_success: number | null;
  requests_per_success: number | null;
  quota_observations: QuotaObservation[];
  /** [verdict, count] pairs, e.g. ["APPROVE_STRONG", 3]. */
  review_verdict_distribution: [string, number][];
}

export type ReportGroupBy = 'backend' | 'model';

/**
 * Actual token/cost burn observed by GAH itself, aggregated from the
 * manager-chat session logs (#940): every assistant message the backends
 * reported usage for, grouped by backend + model + UTC day. Unlike the
 * dispatch-ledger comparison above, this covers manager-chat turns --
 * where nearly all work happens now -- so subscription-burn questions
 * ("how much of the codex plan did yesterday cost?") have a real answer.
 */
export interface UsageRollupRow {
  backend: string;
  /** null = the backend's default model for that turn. */
  model: string | null;
  /** UTC day the turn's assistant message was logged, e.g. 2026-08-30. */
  day: string;
  turns: number;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  /** null when any included turn did not report cost. */
  estimated_cost_usd: number | null;
}

export interface UsageRollupSummary {
  profile: string;
  /** Inclusive window start (ms epoch) the rollup covers. */
  since: number;
  /** ms epoch of the aggregation. */
  generated_at: number;
  rows: UsageRollupRow[];
  /** Turns whose assistant message reported no usage at all -- counted so
   * silent gaps are visible instead of disappearing the burn. */
  unattributed_turns: number;
  /** Same burn grouped per ticket: issue chats roll up under their issue
   * number, other sessions under their branch, the profile default
   * conversation under a shared bucket. Sorted by tokens desc. */
  tickets: UsageRollupTicketRow[];
}

export interface UsageRollupTicketRow {
  ticket: string;
  /** The session's title, when one was set. */
  title: string | null;
  turns: number;
  total_tokens: number;
  /** null when any included turn did not report cost. */
  estimated_cost_usd: number | null;
  /** tokens per backend, e.g. { codex: 1500 }. */
  backends: Record<string, number>;
}

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
  /** Stable repo identity used for GAH-owned branch/worktree naming
   * (`gah/<repo_id>-<ts>`, `gah-chat-<repo_id>-<session>`). Clients that
   * materialize worktrees MUST use this rather than deriving from `repo`,
   * or prune's prefix matching silently misses their worktrees. */
  repo_id: string;
  /** Effective worktree root (defaults.worktree_base). Chat sessions and
   * other clients that materialize worktrees create theirs here so
   * `gah prune` governs them under one base and naming convention. */
  worktree_base: string;
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
  /** Delivery mode for work results ('pr' | 'handoff'). Defaults to 'pr' if omitted. */
  delivery_mode?: 'pr' | 'handoff';
  /** Effective validation command timeout in seconds for this profile (defaults
   * to 300). If unset in TOML, this is computed and returned as the effective
   * timeout. */
  validation_timeout_seconds: number;
  /** Effective idle window before daily chat maintenance archives a live
   * session. Defaults to 14 when omitted from profile TOML. */
  chat_session_idle_days?: number;
}

export interface ProjectImportData {
  gitUrl: string;
  /** Replace an existing managed checkout after verifying it is clean. */
  reclone?: boolean;
}

export interface ProjectImportResult {
  project: ProfileSummary;
  checkoutPath: string;
  checkoutStatus: 'cloned' | 'verified' | 'recloned';
  detectedLanguages: string[];
  validationCommands: string[];
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
  instance: string | null;
  model: string | null;
  quota_pool: string | null;
  priority: number;
  included_in_quota: boolean;
  marginal_cost_usd: number | null;
  quota_usage_percent: number | null;
  quota_days_remaining: number | null;
  requires_approval: boolean;
}

export interface BackendInstanceSummary {
  backend_instance: string;
  runner_kind: string;
  logical_backend: string;
  account_label: string | null;
  auth_source_label: string | null;
  quota_pool: string | null;
  supported_models: string[];
  executable_configured: boolean;
  isolated_state_configured: boolean;
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
  /** Effective delivery behavior for completed work. */
  delivery_mode: 'pr' | 'handoff';
  merge_policy: string;
  max_fix_attempts_per_mr: number;
  max_implementation_failures_per_ticket: number;
  max_review_cycles_per_ticket: number;
  max_paid_reviews_per_ticket: number;
  backend_instances: BackendInstanceSummary[];
  pm_candidates: RoutingCandidateSummary[];
  improve_candidates: RoutingCandidateSummary[];
  review_candidates: RoutingCandidateSummary[];
  task_routing_rules: TaskRoutingRuleSummary[];
  routine_reviewer: RoutingCandidateSummary | null;
  escalatory_reviewers: RoutingCandidateSummary[];
  context: ConfigProfileContextSummary;
  notifications: NotificationSummary;
}

/** Browser-safe notification projection returned by the control-plane
 * Settings API. Environment values are reduced to presence booleans before
 * serialization so command contents and credentials cannot reach the DOM. */
export type SettingsNotificationSummary = Omit<NotificationSummary, 'env_file' | 'env_file_prod'> & {
  env_file_configured: boolean;
  env_file_prod_configured: boolean;
};

/** GET /api/config/effective response. The CLI's raw environment-source
 * fields are deliberately replaced at the server boundary. */
export type SettingsConfigProfileSummary = Omit<ConfigProfileSummary, 'notifications'> & {
  notifications: SettingsNotificationSummary;
};

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
  remediation_plan?: RemediationPlan | null;
}

// TICKET-505: HumanRequired reason codes
export type HumanRequiredReasonCode =
  | 'policy_approval'
  | 'retry_budget_exhausted'
  | 'review_evidence_gate'
  | 'review_output_invalid_exhausted'
  | 'review_ceiling_exhausted'
  | 'merge_policy'
  | 'publishing_restriction'
  | 'configuration_infra'
  | 'fix_retry_cap_exceeded'
  | 'merge_retry_cap_exceeded'
  | 'stuck_loop_gate'
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
  auth_source_label?: string | null;
  quota_pool?: string | null;
  provider_attribution_source?: 'backend_reported' | 'config_declared' | 'inferred' | 'unknown' | 'mixed' | 'mixed_or_unknown' | null;
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

/** Secret-safe canonical route identity persisted for a single attempt.
 * Executable paths and credential values are deliberately excluded. */
export interface ExecutionIdentity {
  runner_kind: string;
  requested_backend: string;
  logical_backend: string;
  backend_instance: string;
  account_label: string | null;
  auth_source_label: string | null;
  quota_pool: string | null;
  requested_model: string | null;
  effective_model: string | null;
}

export interface AttemptRoutingRecord {
  attempt_number: number;
  backend_instance: string;
  effective_model: string | null;
  identity?: ExecutionIdentity | null;
  routing_diagnostics?: RoutingDiagnostics | null;
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
  /** Canonical route identity for each attempted launch. */
  attempt_routing?: AttemptRoutingRecord[];
  /** initial | post_review_repair | review | stuck_loop_gate */
  dispatch_reason?: string | null;
  usage: LedgerUsage;
}

// ---------------------------------------------------------------------------
// `gah ledger summary --json` / `gah availability --json` (Issue #861: HTTP
// endpoints for CLI subcommands that had no route yet). `gah sync --json`
// reuses MergeRequest above -- same `sync_mr_to_json` producer as
// StatusSnapshot.merge_requests.
// ---------------------------------------------------------------------------

export interface LedgerSummary {
  ledger_path: string;
  entries: number;
  success: number;
  failed: number;
  by_mode: Record<string, number>;
  by_requested_backend: Record<string, number>;
  by_backend: Record<string, number>;
  by_model: Record<string, number>;
  by_failure_class: Record<string, number>;
  fallback_count: number;
  validation_pass: number;
  push_success: number;
  mr_count: number;
  human_required_count: number;
  attempts_started: number;
  attempts_completed: number;
  attempts_started_unknown: number;
  attempts_completed_unknown: number;
  average_duration_seconds: number | null;
  usage_input_tokens: number;
  usage_output_tokens: number;
  usage_reasoning_tokens: number;
  usage_cache_read_tokens: number;
  usage_cache_write_tokens: number;
  usage_total_tokens: number;
  usage_requests_count: number;
  estimated_cost_usd: number | null;
  actual_cost_usd: number | null;
  last_run: string | null;
}

/** One row of the raw, unnormalized `gah availability --json` state dump --
 * distinct from StatusSnapshot's `AvailabilityScope[]`, which is a richer
 * view built via `availability_for_identity`. */
export interface AvailabilityRecord {
  backend: string;
  backend_instance?: string;
  model?: string;
  quota_pool?: string;
  eligible: boolean;
  reason?: string;
  unavailable_until?: string;
  remaining_cooldown?: string;
  source?: string;
  last_error_summary?: string;
}

// ---------------------------------------------------------------------------
// Manager chat backend selection (apps/server-only preference, separate
// from ConfigSummary.current_manager -- that field drives the autonomous
// manager-wake notification path, this drives which backend answers the
// interactive Manager Chat page).
// ---------------------------------------------------------------------------

export interface ManagerBackendInfo {
  id: string;
  displayName: string;
  /** False for backends listed but not wired up yet (see #820/#821). */
  implemented: boolean;
}

export interface ManagerChatSettingsSummary {
  defaultBackend: string;
  profileOverrides: Record<string, string>;
  availableBackends: ManagerBackendInfo[];
}

/** Secret-safe GET /api/settings/gateway summary. Credential bytes are
 * available only from the explicit bootstrap-command reveal endpoint. */
export interface GatewaySettingsSummary {
  url: string;
  apiKeyConfigured: boolean;
  enabled: boolean;
  disabledProfiles: string[];
  contextPolicy: MemoryContextPolicy;
  contextPolicies: Record<string, MemoryContextPolicy>;
  degraded: GatewayHealthSummary;
  /** This host's own Tailscale IPv4, best-effort detected -- null if the
   * `tailscale` binary is missing, not logged in, or detection otherwise
   * failed. Used to build an "Add a Node" command a genuinely different
   * machine can reach; the internal `url` above is typically
   * loopback-optimized for this server's own calls, not reachable by
   * anyone else. */
  tailscaleIPv4: string | null;
}

export interface MemoryContextPolicy {
  /** Maximum recalled characters injected per turn. Unset = unlimited. */
  budgetChars?: number;
  /** Eligible memory tiers. Unset/empty = all tiers. */
  tiers?: string[];
}

export interface GatewayHealthSummary {
  degraded: boolean;
  lastError: string | null;
  lastFailedAt: number | null;
  lastOkAt: number | null;
}

/** Sensitive response returned only after an explicit authenticated reveal. */
export interface GatewayBootstrapCommand {
  command: string;
}

export interface GatewaySettingsUpdate {
  url?: string | null;
  apiKey?: string | null;
  enabled?: boolean;
  disabledProfiles?: string[];
  contextPolicy?: MemoryContextPolicy;
  contextPolicies?: Record<string, MemoryContextPolicy>;
}

/** Payload for POST /api/manager-chat/settings. Omitted fields are left
 * unchanged server-side. */
export interface ManagerChatSettingsUpdate {
  defaultBackend?: string;
  profileOverrides?: Record<string, string>;
}

/** A real slash command from the active backend's own command registry
 * (e.g. Hermes's live ACP available-commands list) -- not something GAH
 * invents itself. Powers the "/" palette in Manager Chat. */
export interface ManagerCommandInfo {
  name: string;
  description: string;
  argsHint?: string;
}

/** A real selectable model from the active backend's own ACP session config
 * options -- not a list GAH maintains. An empty list means no model picker
 * for that backend. */
export interface ManagerModelInfo {
  id: string;
  name: string;
  description?: string;
}

/** A reasoning-effort choice advertised by the active backend through ACP's
 * `thought_level` session config category. Values are provider-owned. */
export interface ManagerReasoningEffortInfo {
  id: string;
  name: string;
  description?: string;
}

export interface ManagerModelsSummary {
  models: ManagerModelInfo[];
  currentModelId: string | null;
  reasoningEfforts: ManagerReasoningEffortInfo[];
  currentReasoningEffortId: string | null;
  /** Context-window occupancy from ACP's usage_update notification
   * (issue #865) -- only Hermes emits this today. Null for any backend
   * that doesn't, so the UI hides the indicator rather than showing a
   * fake 0/0. */
  contextUsage: { size: number; used: number } | null;
}

// ---------------------------------------------------------------------------
// Skill bank (issue #963/#964): the central node's versioned skill store.
// A skill is authored/installed once here and bound to any backend/instance
// on any node without touching that provider's own configuration. #964 ships
// the store + /api/skills; #965 resolves bindings per backend instance.
// ---------------------------------------------------------------------------

/** One versioned skill record in the central bank. Multiple versions of the
 * same id coexist; unversioned reads resolve to the newest. */
export interface Skill {
  /** Stable id, e.g. 'gah-manager'. */
  id: string;
  /** Semver-ish version string, e.g. '1.0.0'. */
  version: string;
  displayName: string;
  description: string;
  /** The skill content (markdown text, a prompt file, etc.). */
  content: string;
  /** Declared backend compatibility (e.g. ['hermes', 'codex']). Empty = all
   * backends. Used by #965 to decide which bindings are applicable. */
  backends: string[];
  /** Provenance: where the skill came from (e.g. 'docs/gah-manager-skill.md'). */
  source: string;
  createdAt: number;
  updatedAt: number;
}

/** The durable bank file's shape: a flat list of versioned records plus the
 * (still-resolving, #965) binding registry used to refuse deletion of a
 * skill that is in use. */
export interface SkillBankFile {
  skills: Skill[];
  /** skillId -> human-readable binding labels (e.g. 'hermes' or
   * 'hermes:gah'). A skill with bindings cannot be deleted. */
  bindings: Record<string, string[]>;
  /** Binding labels that declare a set explicitly. This preserves empty
   * project overrides and distinguishes absent canonical instance sets. */
  bindingOverrides: string[];
}

export interface SkillSummary {
  id: string;
  version: string;
  displayName: string;
  description: string;
  backends: string[];
  source: string;
  /** True when this skill has live bindings (deletion would be refused). */
  bound: boolean;
}

export interface SkillBindingSummary {
  profile: string;
  backend: string;
  instance: string | null;
  source: 'canonical' | 'profile';
  supported: boolean;
  selectedIds: string[];
  observedSkills: { id: string; version: string }[] | null;
  skills: SkillSummary[];
}

export interface SkillBindingUpdate {
  profile: string;
  backend: string;
  instance?: string | null;
  skillIds: string[];
}

export interface SkillResolution {
  profile: string;
  backend: string;
  instance: string | null;
  source: 'canonical' | 'profile';
  skills: Skill[];
}

// ---------------------------------------------------------------------------
// In-app update (issue #989): `gah update --role central --restart-server`
// driven from the dashboard instead of SSH. `inferred_restart` exists
// because the update's own final step restarts this server, which kills the
// updater before it can record its exit code -- see apps/server/src/adminUpdate.ts.
// ---------------------------------------------------------------------------

export interface AdminUpdateCommitInfo {
  hash: string;
  short: string;
  subject: string;
}

export interface AdminUpdatePendingInfo {
  current: AdminUpdateCommitInfo | null;
  latest: AdminUpdateCommitInfo | null;
  commitsBehind: number;
  upToDate: boolean;
}

export type AdminUpdateStatus = 'idle' | 'running' | 'success' | 'failed' | 'inferred_restart';

export interface AdminUpdateState {
  status: AdminUpdateStatus;
  startedAt: string | null;
  finishedAt: string | null;
  exitCode: number | null;
  pid: number | null;
  output: string;
}
