use crate::config::Profile;
use crate::routing::RoutingRuntimeState;
use serde::{Deserialize, Serialize};
use std::path::Path;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Coarse attribution for why a dispatch failed. Deliberately not
/// exhaustively wired everywhere yet (TICKET-063): only the least ambiguous
/// boundaries in dispatch.rs set this. Everything else stays `None` rather
/// than guess. Persisted as a plain lowercase string, matching this
/// codebase's existing convention for enum-like ledger fields (e.g.
/// `validation_result`) rather than a serde-tagged enum, so the wire format
/// never breaks if variants are renamed internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // unwired variants are the schema for future tickets, not unused code
pub enum FailureClass {
    HarnessError,
    EnvironmentError,
    BackendError,
    AgentNoProgress,
    AgentFailure,
    /// The reviewer process completed, but its structured payload could not
    /// safely drive a repair. Kept separate from backend and agent failures so
    /// routing can request a second opinion without blaming the code change.
    ReviewOutputInvalid,
    ContextLimitExceeded,
    ValidationFailure,
    /// TICKET-073: the dispatch *gate* itself (a profile's
    /// `validation_commands`) failed self-verification against a fresh
    /// worktree. Deliberately distinct from `ValidationFailure`, which means
    /// the dispatched ticket's work failed the gate -- conflating the two
    /// would make a broken config look like the ticket's fault.
    ValidationGate,
    /// Issue #584: a backend returned a structured disposition reporting that
    /// the source issue's requirements are already satisfied in the target
    /// branch, with grounded file/test evidence and *no* repository diff. This
    /// is deliberately distinct from `AgentNoProgress` (the agent tried and
    /// could not make progress) so GAH never forces an agent to manufacture a
    /// change just to close an already-completed task.
    AlreadySatisfied,
    HumanBlocked,
    Unknown,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HarnessError => "harness_error",
            Self::EnvironmentError => "environment_error",
            Self::BackendError => "backend_error",
            Self::AgentNoProgress => "agent_no_progress",
            Self::AgentFailure => "agent_failure",
            Self::ReviewOutputInvalid => "review_output_invalid",
            Self::ContextLimitExceeded => "context_limit_exceeded",
            Self::ValidationFailure => "validation_failure",
            Self::ValidationGate => "validation_gate",
            Self::AlreadySatisfied => "already_satisfied",
            Self::HumanBlocked => "human_blocked",
            Self::Unknown => "unknown",
        }
    }
}

/// Where in the dispatch pipeline a failure occurred. See `FailureClass` for
/// the "not exhaustively wired yet" caveat — same applies here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // unwired variants are the schema for future tickets, not unused code
pub enum FailureStage {
    /// Top-level fallback for an error that escaped a more precise pipeline
    /// boundary. This keeps operational alerts actionable instead of
    /// emitting an unclassified `unknown` stage.
    Dispatch,
    Preflight,
    BaselineValidation,
    Route,
    BackendLaunch,
    AgentRun,
    PostValidation,
    Commit,
    Push,
    MrCreate,
    Review,
    Sync,
}

impl FailureStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::Preflight => "preflight",
            Self::BaselineValidation => "baseline_validation",
            Self::Route => "route",
            Self::BackendLaunch => "backend_launch",
            Self::AgentRun => "agent_run",
            Self::PostValidation => "post_validation",
            Self::Commit => "commit",
            Self::Push => "push",
            Self::MrCreate => "mr_create",
            Self::Review => "review",
            Self::Sync => "sync",
        }
    }
}

/// TICKET-064: one record per retry-loop attempt within a single dispatch.
/// Embedded in LedgerEntry (not a separate append-only stream) — a
/// deliberate scope reduction from the ticket's stated preference, chosen
/// for simplicity (one file, one read path). The tradeoff: if the process
/// crashes mid-retry, in-progress attempts are lost along with the rest of
/// the not-yet-appended ledger line, same as every other field on this
/// struct today.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct AttemptRecord {
    pub attempt_number: u32,
    pub backend: String,
    pub effective_model: Option<String>,
    pub exit_code: Option<i32>,
    pub validation_result: Option<String>,
    pub failure_class: Option<String>,
    pub failure_stage: Option<String>,
    pub duration_seconds: Option<f64>,
    pub diff_path: Option<String>,
    /// TICKET-242: AGY CLI version for this attempt (e.g. "1.0.16"), captured
    /// via `agy --version` when an AGY backend ran. `None` for non-AGY
    /// backends and for runs where version detection failed. `#[serde(default)]`
    /// keeps historical ledger entries (written before this field existed)
    /// deserializing.
    #[serde(default)]
    pub cli_version: Option<String>,
    /// TICKET-101: provider-reported usage for exactly this attempt, not
    /// the whole dispatch. Same "unknown stays unknown, never zero"
    /// discipline as `LedgerEntry.usage` -- an empty `LedgerUsage` (all
    /// `None`) means "the backend didn't report it," not "zero usage."
    /// `#[serde(default)]` so historical ledger entries without this field
    /// still deserialize.
    #[serde(default)]
    pub usage: LedgerUsage,
}

/// Route selected for one launched attempt inside a dispatch. Unlike the
/// top-level routing fields, this list preserves earlier route decisions and
/// their skip diagnostics when a later retry changes backend.
/// Issue #119: provenance/quality of a normalized per-attempt behavior metric.
///
/// Distinguishes how a behavior count (tool calls, shell calls, file edits,
/// test runs) was obtained so that "unavailable" is never silently treated as
/// a real zero.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorMetricQuality {
    /// Backend/session reported the count directly (provider API/usage).
    ProviderReported,
    /// Count derived from documented structured backend events (e.g. the
    /// `gah.behavior_summary` event), never from arbitrary prose or logs.
    StructuredEventDerived,
    /// Count is a coarse estimate (e.g. bounded from a known command budget).
    Estimated,
    /// Backend does not expose this metric at all.
    Unavailable,
}

/// Issue #119: a per-attempt behavior metric captured with explicit provenance.
///
/// `count` is `None` when the backend did not report it (unknown). Unknown is
/// distinct from zero and must never be coerced to `0`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct BehaviorMetric {
    /// Known count (`None` = unknown / not reported).
    #[serde(default)]
    pub count: Option<u64>,
    /// How this count was obtained.
    pub quality: BehaviorMetricQuality,
    /// Why the count is unknown when `count` is `None` and quality is
    /// `Unavailable`. Kept separate so consumers never conflate missing
    /// telemetry with a real zero.
    #[serde(default)]
    pub unknown_reason: Option<String>,
}

impl BehaviorMetric {
    /// Unknown metric with the given reason; never serialized as zero.
    pub fn unavailable(reason: &str) -> Self {
        BehaviorMetric {
            count: None,
            quality: BehaviorMetricQuality::Unavailable,
            unknown_reason: Some(reason.to_string()),
        }
    }
}

/// Issue #119: normalized per-attempt behavior metrics with provenance.
///
/// Optional at the usage level so pre-tracking ledger lines (written before
/// this field existed) deserialize as `None` (unknown) rather than zeros.
#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct AttemptBehaviorMetrics {
    #[serde(default)]
    pub tool_calls: Option<BehaviorMetric>,
    #[serde(default)]
    pub shell_calls: Option<BehaviorMetric>,
    #[serde(default)]
    pub file_edits: Option<BehaviorMetric>,
    #[serde(default)]
    pub test_runs: Option<BehaviorMetric>,
}

impl AttemptBehaviorMetrics {
    /// Whether every metric is unknown (no known count anywhere). Used to keep
    /// failed/retried/timeout/cancelled/fallback attempts preserved
    /// independently even when no behavior data is available.
    pub fn is_fully_unknown(&self) -> bool {
        let known =
            |m: &Option<BehaviorMetric>| m.as_ref().map(|b| b.count.is_some()).unwrap_or(false);
        !known(&self.tool_calls)
            && !known(&self.shell_calls)
            && !known(&self.file_edits)
            && !known(&self.test_runs)
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct AttemptRoutingRecord {
    pub attempt_number: u32,
    pub backend_instance: String,
    pub effective_model: Option<String>,
    #[serde(default)]
    pub routing_diagnostics: Option<RoutingDiagnostics>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct LedgerUsage {
    pub usage_source: Option<String>,
    /// Normalized accounting class. This is explicit even when the backend
    /// cannot identify it; `unknown` is never treated as zero-cost.
    #[serde(default)]
    pub usage_classification: Option<String>,
    /// Safe logical instance/account labels; never secrets or raw API keys.
    #[serde(default)]
    pub backend_instance: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    /// Model reported by the backend/session, distinct from the route's
    /// requested/effective model when an alias or fallback substituted it.
    #[serde(default)]
    pub actual_model: Option<String>,
    /// Why `actual_model` is unknown. Kept separate from the requested and
    /// effective route model so consumers never have to infer whether a null
    /// means missing telemetry, an alias, or a mixed-attempt aggregate.
    #[serde(default)]
    pub actual_model_unknown_reason: Option<String>,
    /// Why the model provider is unknown when `provider` is absent.
    #[serde(default)]
    pub provider_unknown_reason: Option<String>,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub pricing_source: Option<String>,
    #[serde(default)]
    pub pricing_version: Option<String>,
    #[serde(default)]
    pub cost_unknown_reason: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub requests_count: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub actual_cost_usd: Option<f64>,
    pub quota_window: Option<String>,
    pub quota_used_percent: Option<f64>,
    pub quota_remaining_percent: Option<f64>,
    pub quota_reset_at: Option<String>,
    /// Exact token counters were not exposed for this execution. Distinct
    /// from zero tokens, which must only be recorded when the backend says 0.
    #[serde(default)]
    pub token_usage_unknown_reason: Option<String>,
    /// Quota state was unavailable for a quota-backed execution.
    #[serde(default)]
    pub quota_unknown_reason: Option<String>,
    /// Issue #119: provenance-aware per-attempt behavior metrics (tool calls,
    /// shell calls, file edits, test runs). Optional so historical ledger
    /// lines without this key deserialize as `None` (unknown), never zero.
    #[serde(default)]
    pub behavior_metrics: Option<AttemptBehaviorMetrics>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct RoutingDiagnostics {
    #[serde(default)]
    pub policy_reordered_candidates: bool,
    #[serde(default)]
    pub selected_backend: Option<String>,
    #[serde(default)]
    pub selected_model: Option<String>,
    #[serde(default)]
    pub selected_quota_pool: Option<String>,
    #[serde(default)]
    pub selected_pace_band: Option<String>,
    #[serde(default)]
    pub selected_cost_class: Option<String>,
    #[serde(default)]
    pub selected_over: Vec<String>,
    #[serde(default)]
    pub candidates: Vec<RoutingCandidateDiagnostic>,
    #[serde(default)]
    pub human_summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct RoutingCandidateDiagnostic {
    pub backend: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quota_pool: Option<String>,
    #[serde(default)]
    pub default_order: Option<usize>,
    #[serde(default)]
    pub consideration_order: Option<usize>,
    #[serde(default)]
    pub pace_band: Option<String>,
    #[serde(default)]
    pub cost_class: Option<String>,
    #[serde(default)]
    pub skip_reason: Option<String>,
    #[serde(default)]
    pub unavailable_until: Option<String>,
}

/// Ledger wire schema version. Bumped whenever a field's semantics change
/// in a way that affects aggregation or round-tripping.
///
/// - `1` (default for entries written before this field existed): the legacy
///   shape. Attempt counters `attempts_started`/`attempts_completed` were
///   plain `u32` with a serde default, so pre-attempt-tracking entries
///   deserialized as literal `0` and were indistinguishable from real zeros
///   (issue #240).
/// - `2`: attempt counters became `Option<u32>` so unknown stays unknown
///   (never coerced to `0`), honoring the standing "unknown remains unknown"
///   usage rule.
/// - `3`: usage distinguishes reasoning tokens and records explicit reasons
///   when exact token or quota telemetry is unavailable.
// v4 adds `review_metadata_fingerprint`. The field has a serde default so v1-v3
// history remains readable; its absence deliberately makes old reviews stale.
// v5 adds typed, machine-validated actionable review findings. The field also
// defaults empty so historical review records remain readable but cannot
// silently become repair instructions.
// v6 adds an independently-versioned review contract and source/metadata
// generation. Ledger schema and review policy deliberately advance separately.
pub const LEDGER_SCHEMA_VERSION: u32 = 7;

/// Version of the machine-enforced review output and lifecycle policy. Bump
/// this when an older review opinion, retry budget, or derived human gate must
/// not remain authoritative under the new contract. This is intentionally not
/// the ledger wire-schema version.
pub const CURRENT_REVIEW_CONTRACT_VERSION: u32 = 1;
/// Short compatibility name used by the generation-aware lifecycle modules.
/// Both names identify the same independently-versioned review contract.
pub const REVIEW_CONTRACT_VERSION: u32 = CURRENT_REVIEW_CONTRACT_VERSION;

pub fn review_generation(
    source_sha: Option<&str>,
    metadata_fingerprint: Option<&str>,
) -> Option<String> {
    let source_sha = source_sha.filter(|value| !value.trim().is_empty())?;
    let metadata_fingerprint = metadata_fingerprint.filter(|value| !value.trim().is_empty())?;
    Some(format!(
        "review-v{CURRENT_REVIEW_CONTRACT_VERSION}:{source_sha}:{metadata_fingerprint}"
    ))
}

fn default_ledger_schema_version() -> u32 {
    1
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LedgerEntry {
    #[serde(default = "default_ledger_schema_version")]
    pub schema_version: u32,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub profile: String,
    pub display_name: String,
    pub repo_id: String,
    pub repo: String,
    pub local_path: String,
    pub provider: String,
    pub backend: String,
    pub requested_backend: String,
    pub effective_backend: String,
    pub requested_model: Option<String>,
    pub effective_model: Option<String>,
    pub routing_reason: Option<String>,
    pub fallback_used: bool,
    pub confidence_impact: Option<String>,
    pub human_required: bool,
    /// Work-item-specific reason for `human_required` when the hold is known.
    /// This intentionally remains optional so historical and opportunistically
    /// produced entries do not infer a reason they never observed.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub human_required_reason_code: Option<String>,
    #[serde(default)]
    pub routing_diagnostics: Option<RoutingDiagnostics>,
    pub mode: String,
    pub target_summary: Option<String>,
    #[serde(default)]
    pub work_id: Option<String>,
    #[serde(default)]
    pub source_issue_number: Option<String>,
    #[serde(default)]
    pub work_title: Option<String>,
    pub branch: Option<String>,
    /// TICKET-118 (classification): task class parsed from ticket metadata,
    /// e.g. `improve`, `fix`, `pm`, `review`, `experiment`. `#[serde(default)]`
    /// so pre-existing JSONL ledger lines without this key still deserialize.
    #[serde(default)]
    pub task_class: Option<String>,
    /// TICKET-118 (classification): difficulty parsed from ticket metadata,
    /// e.g. `easy`, `medium`, `hard`. `#[serde(default)]` for pre-existing
    /// JSONL ledger lines.
    #[serde(default)]
    pub difficulty: Option<String>,
    pub session_dir: Option<String>,
    pub duration_seconds: Option<f64>,
    pub backend_exit_code: Option<i32>,
    pub validation_result: Option<String>,
    /// TICKET-125: review verdict/confidence/reviewer identity for associating
    /// review entries back to implementation entries. `#[serde(default)]` for
    /// pre-existing ledger lines.
    #[serde(default)]
    pub review_verdict: Option<String>,
    #[serde(default)]
    pub review_confidence: Option<String>,
    #[serde(default)]
    pub reviewer_backend: Option<String>,
    #[serde(default)]
    pub reviewer_model: Option<String>,
    /// Issue #214: reviewer authority tier ("strong"/"escalatory"/"standard"/
    /// "weak"), derived from routing config via `derive_reviewer_tier` -- never
    /// parsed from the model's JSON. Persisted on `LedgerEntry` (the transient
    /// `ReviewVerdict.reviewer_tier` in models.rs is the source of truth that
    /// populates this) so per-tier cost/outcome breakdowns can be reconstructed
    /// from ledger history instead of sniffing verdict text.
    #[serde(default)]
    pub reviewer_tier: Option<String>,
    /// Immutable source commit reviewed by this ledger record. Missing on
    /// historical records, which must never be considered duplicates.
    #[serde(default)]
    pub review_source_sha: Option<String>,
    /// Versioned digest of the provider title/body/draft state and source SHA
    /// inspected by this review. Historical entries omit it and are therefore
    /// intentionally re-reviewed before their verdict can drive a repair.
    #[serde(default)]
    pub review_metadata_fingerprint: Option<String>,
    /// Independent review-policy/output contract version. `None` means the
    /// record predates versioned review authority and is telemetry only.
    #[serde(default)]
    pub review_contract_version: Option<u32>,
    /// Exact review lifecycle generation: contract + source SHA + provider
    /// metadata fingerprint. Review-derived gates and budgets are authoritative
    /// only while this matches the active MR generation.
    #[serde(default)]
    pub review_generation: Option<String>,
    /// Authority class (strong/standard/weak/escalatory) of the reviewer.
    #[serde(default)]
    pub reviewer_class: Option<String>,
    /// Deterministic reason why GAH made a reviewer output non-mergeable.
    #[serde(default)]
    pub review_gate_reason: Option<String>,
    /// Structured reviewer output retained for deterministic repair prompts.
    /// These fields intentionally remain separate from the rendered provider
    /// comment so FixMr never has to scrape human-formatted Markdown.
    #[serde(default)]
    pub review_blocking_findings: Vec<String>,
    #[serde(default)]
    pub review_actionable_findings: Vec<crate::models::ActionableReviewFinding>,
    #[serde(default)]
    pub review_non_blocking_findings: Vec<String>,
    #[serde(default)]
    pub review_risk_notes: Vec<String>,
    #[serde(default)]
    pub review_evidence: Vec<String>,
    #[serde(default)]
    pub review_compatibility_evidence: Vec<String>,
    /// TICKET-540: review supervision classification. `Some("idle")` when the
    /// reviewer was killed after the idle budget with no progress; `Some("hard")`
    /// when it hit the optional wall-clock hard ceiling while still making
    /// progress. `None` for any other outcome. Distinct so a healthy reviewer
    /// exceeding the old flat clock is never conflated with a backend failure.
    #[serde(default)]
    pub review_timeout_class: Option<String>,
    /// TICKET-540: configured review idle supervision budget (seconds), surfaced
    /// for telemetry/status even on a successful review.
    #[serde(default)]
    pub review_idle_timeout_seconds: Option<u64>,
    /// TICKET-540: configured review hard safety ceiling (seconds), if any.
    #[serde(default)]
    pub review_hard_timeout_seconds: Option<u64>,
    /// TICKET-540: elapsed seconds since dispatch start at last observed review
    /// progress (stdout/stderr activity, worktree update, or child process
    /// CPU/I/O). `None` when the reviewer never produced progress.
    #[serde(default)]
    pub review_last_progress_secs: Option<f64>,
    pub commit_attempted: bool,
    pub commit_created: bool,
    pub push_attempted: bool,
    pub push_succeeded: bool,
    pub mr_attempted: bool,
    pub mr_created: bool,
    pub mr_url: Option<String>,
    /// Generic provider-write telemetry used by operations that are not merge
    /// requests (for example PM child issue publication). Values are
    /// secret-safe enums/URLs and remain optional for historical entries.
    #[serde(default)]
    pub provider_mutation_kind: Option<String>,
    #[serde(default)]
    pub provider_mutation_status: Option<String>,
    #[serde(default)]
    pub provider_mutation_url: Option<String>,
    pub files_changed: Option<u32>,
    pub insertions: Option<u32>,
    pub deletions: Option<u32>,
    pub error_summary: Option<String>,
    /// TICKET-063: coarse failure attribution, populated at only the
    /// clearest boundaries so far. `#[serde(default)]` so pre-existing
    /// JSONL ledger lines without these keys still deserialize.
    #[serde(default)]
    pub failure_class: Option<String>,
    #[serde(default)]
    pub failure_stage: Option<String>,
    /// TICKET-064: how many retry-loop iterations were entered vs. ran
    /// their backend to completion (launched and exited, regardless of
    /// whether validation then passed). `#[serde(default)]` for pre-existing
    /// lines. **Issue #240:** now `Option<u32>` instead of `u32` — legacy
    /// entries whose JSONL line lacks these keys (written before attempt
    /// tracking existed) deserialize as `None` (unknown) rather than `0`, so
    /// historical telemetry aggregates can no longer silently mix unknown with
    /// real zeros. New entries always write `Some(n)`.
    #[serde(default)]
    pub attempts_started: Option<u32>,
    #[serde(default)]
    pub attempts_completed: Option<u32>,
    #[serde(default)]
    pub attempts: Vec<AttemptRecord>,
    #[serde(default)]
    pub attempt_routing: Vec<AttemptRoutingRecord>,
    /// Live routing state for the current dispatch only.
    #[serde(skip, default)]
    pub routing_runtime: RoutingRuntimeState,
    /// Distinguishes the *kind* of dispatch that produced this ledger entry:
    /// `initial` (first DispatchTicket), `post_review_repair` (FixMr after a
    /// NEEDS_FIX review), `review` (ReviewMr), or `stuck_loop_gate` (a
    /// synthetic human-required gate written by the stuck-loop detector).
    /// The retry cap (`count_fix_attempts_per_branch`) counts ONLY
    /// `post_review_repair` entries — internal OpenHands retries within a
    /// single dispatch (attempts_started) do NOT consume retry budget.
    #[serde(default)]
    pub dispatch_reason: Option<String>,
    /// Context budgeting metadata for the assembled phase prompt.
    #[serde(default)]
    pub context_phase: Option<String>,
    #[serde(default)]
    pub context_estimated_tokens_before: Option<u64>,
    #[serde(default)]
    pub context_estimated_tokens_after: Option<u64>,
    #[serde(default)]
    pub context_compacted: bool,
    pub usage: LedgerUsage,
}

impl LedgerEntry {
    pub fn new(
        profile_name: &str,
        profile: &Profile,
        backend: &str,
        mode: &str,
        target: &str,
        session_id: Option<String>,
        session_dir: Option<&Path>,
    ) -> Self {
        Self {
            timestamp: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string()),
            schema_version: LEDGER_SCHEMA_VERSION,
            session_id,
            profile: profile_name.to_string(),
            display_name: profile.display_name.clone(),
            repo_id: profile.repo_id.clone(),
            repo: profile.repo.clone(),
            local_path: profile.local_path.clone(),
            provider: profile.provider.clone(),
            backend: backend.to_string(),
            requested_backend: backend.to_string(),
            effective_backend: backend.to_string(),
            requested_model: None,
            effective_model: None,
            routing_reason: None,
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            human_required_reason_code: None,
            routing_diagnostics: None,
            mode: mode.to_string(),
            target_summary: summarize_target(target),
            work_id: None,
            source_issue_number: None,
            work_title: None,
            task_class: None,
            difficulty: None,
            branch: None,
            session_dir: session_dir.map(|p| p.display().to_string()),
            duration_seconds: None,
            backend_exit_code: None,
            validation_result: None,
            review_verdict: None,
            review_confidence: None,
            reviewer_backend: None,
            reviewer_model: None,
            reviewer_tier: None,
            review_source_sha: None,
            review_metadata_fingerprint: None,
            review_contract_version: None,
            review_generation: None,
            reviewer_class: None,
            review_gate_reason: None,
            review_blocking_findings: Vec::new(),
            review_actionable_findings: Vec::new(),
            review_non_blocking_findings: Vec::new(),
            review_risk_notes: Vec::new(),
            review_evidence: Vec::new(),
            review_compatibility_evidence: Vec::new(),
            review_timeout_class: None,
            review_idle_timeout_seconds: None,
            review_last_progress_secs: None,
            review_hard_timeout_seconds: None,
            commit_attempted: false,
            commit_created: false,
            push_attempted: false,
            push_succeeded: false,
            mr_attempted: false,
            mr_created: false,
            mr_url: None,
            provider_mutation_kind: None,
            provider_mutation_status: None,
            provider_mutation_url: None,
            files_changed: None,
            insertions: None,
            deletions: None,
            error_summary: None,
            failure_class: None,
            failure_stage: None,
            attempts_started: Some(0),
            attempts_completed: Some(0),
            attempts: Vec::new(),
            attempt_routing: Vec::new(),
            routing_runtime: RoutingRuntimeState::default(),
            dispatch_reason: None,
            context_phase: None,
            context_estimated_tokens_before: None,
            context_estimated_tokens_after: None,
            context_compacted: false,
            usage: LedgerUsage::default(),
        }
    }

    /// Set precise failure attribution at the specific error site whenever
    /// possible. The dispatch boundary supplies a broad harness/dispatch
    /// fallback only when a path reaches it without either field populated.
    pub fn set_failure(&mut self, class: FailureClass, stage: FailureStage) {
        self.failure_class = Some(class.as_str().to_string());
        self.failure_stage = Some(stage.as_str().to_string());
    }

    /// Issue #95: create a tombstone entry for `gah clear-attempts`. The
    /// tombstone marks all prior ledger entries for the given `work_id` as
    /// stale. It does NOT rewrite history -- the original entries remain in
    /// the JSONL file, but `ledger_lookup_for_ticket` will reset its running
    /// counters when it encounters a `clear_attempts` entry.
    pub fn new_clear_attempts(profile_name: &str, profile: &Profile, work_id: &str) -> Self {
        Self {
            timestamp: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string()),
            schema_version: LEDGER_SCHEMA_VERSION,
            session_id: None,
            profile: profile_name.to_string(),
            display_name: profile.display_name.clone(),
            repo_id: profile.repo_id.clone(),
            repo: profile.repo.clone(),
            local_path: profile.local_path.clone(),
            provider: profile.provider.clone(),
            backend: String::new(),
            requested_backend: String::new(),
            effective_backend: String::new(),
            requested_model: None,
            effective_model: None,
            routing_reason: None,
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            human_required_reason_code: None,
            routing_diagnostics: None,
            mode: "clear_attempts".to_string(),
            target_summary: None,
            work_id: Some(work_id.to_string()),
            source_issue_number: None,
            work_title: None,
            task_class: None,
            difficulty: None,
            branch: None,
            session_dir: None,
            duration_seconds: None,
            backend_exit_code: None,
            validation_result: None,
            review_verdict: None,
            review_confidence: None,
            reviewer_backend: None,
            reviewer_model: None,
            reviewer_tier: None,
            review_source_sha: None,
            review_metadata_fingerprint: None,
            review_contract_version: None,
            review_generation: None,
            reviewer_class: None,
            review_gate_reason: None,
            review_blocking_findings: Vec::new(),
            review_actionable_findings: Vec::new(),
            review_non_blocking_findings: Vec::new(),
            review_risk_notes: Vec::new(),
            review_evidence: Vec::new(),
            review_compatibility_evidence: Vec::new(),
            review_timeout_class: None,
            review_idle_timeout_seconds: None,
            review_last_progress_secs: None,
            review_hard_timeout_seconds: None,
            commit_attempted: false,
            commit_created: false,
            push_attempted: false,
            push_succeeded: false,
            mr_attempted: false,
            mr_created: false,
            mr_url: None,
            provider_mutation_kind: None,
            provider_mutation_status: None,
            provider_mutation_url: None,
            files_changed: None,
            insertions: None,
            deletions: None,
            error_summary: None,
            failure_class: None,
            failure_stage: None,
            attempts_started: Some(0),
            attempts_completed: Some(0),
            attempts: Vec::new(),
            attempt_routing: Vec::new(),
            routing_runtime: RoutingRuntimeState::default(),
            dispatch_reason: None,
            context_phase: None,
            context_estimated_tokens_before: None,
            context_estimated_tokens_after: None,
            context_compacted: false,
            usage: LedgerUsage::default(),
        }
    }

    /// Parallel-worker safety: a "claim" entry written the MOMENT a
    /// dispatch begins (before any backend work runs), not just when it
    /// finishes. `run()` only appended a ledger entry at the very end
    /// (success or failure) -- with multiple `gah loop` workers running
    /// for one profile, a ticket that's mid-flight for
    /// minutes-to-hours had NO ledger trace yet, so a second worker's
    /// `check_duplicate_work` couldn't see it was already being worked and
    /// could dispatch the same ticket twice. This entry exists so a
    /// concurrent worker sees "claimed" immediately. Superseded by the
    /// real completion entry once the dispatch finishes; `is_claim_stale`
    /// (dispatch.rs) governs when an orphaned claim (worker crashed/killed
    /// mid-flight) stops blocking retries.
    pub fn new_claim(profile_name: &str, profile: &Profile, work_id: &str) -> Self {
        Self {
            mode: "claim".to_string(),
            ..Self::new_clear_attempts(profile_name, profile, work_id)
        }
    }

    /// A manager session (human or supervising Claude/Codex/Hermes,
    /// explicitly NOT gah's own automated loop) is reviewing this work_id
    /// out of band and wants `decide_next_action` to hold off auto-merging
    /// it until the review is done. Mirrors `new_claim`'s shape (a
    /// `LedgerEntry` with a distinct `mode`, superseded by a later entry)
    /// but is a separate mechanism -- a claim guards dispatch-time duplicate
    /// work, this guards merge-time timing against an out-of-band review.
    pub fn new_review_hold(
        profile_name: &str,
        profile: &Profile,
        work_id: &str,
        reason: Option<String>,
    ) -> Self {
        Self {
            mode: "review_hold".to_string(),
            target_summary: reason,
            ..Self::new_clear_attempts(profile_name, profile, work_id)
        }
    }

    /// Releases a prior `new_review_hold` for `work_id`. The hold and its
    /// release are both plain ledger entries -- `active_review_hold_work_ids`
    /// determines "active" by finding whichever of the two is most recent.
    pub fn new_review_hold_release(profile_name: &str, profile: &Profile, work_id: &str) -> Self {
        Self {
            mode: "review_hold_release".to_string(),
            ..Self::new_clear_attempts(profile_name, profile, work_id)
        }
    }

    /// Persist an operator's work-item-scoped permission to use one exact
    /// paid backend/model route. The identity is intentionally safe metadata,
    /// never a key, account token, or other credential.
    pub fn new_paid_route_approval(
        profile_name: &str,
        profile: &Profile,
        work_id: &str,
        backend: &str,
        model: Option<&str>,
        granted: bool,
    ) -> Self {
        Self {
            mode: if granted {
                "paid_route_approval_grant".to_string()
            } else {
                "paid_route_approval_revoke".to_string()
            },
            backend: backend.to_string(),
            requested_backend: backend.to_string(),
            effective_backend: backend.to_string(),
            requested_model: model.map(str::to_string),
            effective_model: model.map(str::to_string),
            // A grant is a controller resume signal, not a failed execution.
            // `ledger_lookup_for_ticket` derives the escalation signal from
            // this control mode without polluting failure telemetry.
            ..Self::new_clear_attempts(profile_name, profile, work_id)
        }
    }
}

fn summarize_target(target: &str) -> Option<String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return None;
    }
    let single_line = trimmed.lines().next().unwrap_or(trimmed).trim();
    let mut summary = single_line.to_string();
    if summary.len() > 240 {
        summary.truncate(240);
        summary.push_str("...");
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Profile, RoutingPolicy};
    use std::collections::HashMap;

    fn profile() -> Profile {
        Profile {
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
            prune_older_than_days: None,
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: "/tmp/repo".into(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: None,
            provider_project_id: None,
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            vibe_args: vec![],
            vibe_path: None,
            opencode_args: vec![],
            opencode_path: None,
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds_by_model: HashMap::new(),
            max_concurrent_per_model: HashMap::new(),
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
            max_open_managed_mrs: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            auto_fix_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            review_hard_timeout_seconds: None,
            validation_timeout_seconds: None,
            notify_command: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
            publishing: Default::default(),
        }
    }

    #[test]
    fn target_summary_is_trimmed_to_first_line() {
        let entry = LedgerEntry::new(
            "test",
            &profile(),
            "claude",
            "pm",
            "first line\nsecond line",
            Some("123".into()),
            None,
        );
        assert_eq!(entry.target_summary.as_deref(), Some("first line"));
    }

    #[test]
    fn routing_diagnostics_round_trip_through_json() {
        let mut entry = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        entry.routing_diagnostics = Some(RoutingDiagnostics {
            policy_reordered_candidates: true,
            selected_backend: Some("codex".into()),
            selected_model: Some("gpt-5.4".into()),
            selected_quota_pool: Some("codex-main".into()),
            selected_pace_band: Some("aggressive_burn".into()),
            selected_cost_class: Some("included_quota".into()),
            selected_over: vec!["openhands/gpt-5.4 (paid $0.2500)".into()],
            candidates: vec![RoutingCandidateDiagnostic {
                backend: "codex".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: Some("codex-main".into()),
                default_order: Some(1),
                consideration_order: Some(0),
                pace_band: Some("aggressive_burn".into()),
                cost_class: Some("included_quota".into()),
                skip_reason: None,
                unavailable_until: None,
            }],
            human_summary: Some("selected codex/gpt-5.4".into()),
        });
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed
                .routing_diagnostics
                .as_ref()
                .unwrap()
                .selected_backend
                .as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn new_entry_has_no_failure_attribution_by_default() {
        let entry = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        assert_eq!(entry.failure_class, None);
        assert_eq!(entry.failure_stage, None);
        assert_eq!(entry.work_id, None);
        assert_eq!(entry.work_title, None);
    }

    #[test]
    fn set_failure_populates_both_fields_as_lowercase_strings() {
        let mut entry = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        entry.set_failure(FailureClass::BackendError, FailureStage::AgentRun);
        assert_eq!(entry.failure_class.as_deref(), Some("backend_error"));
        assert_eq!(entry.failure_stage.as_deref(), Some("agent_run"));
    }

    #[test]
    fn failure_attribution_round_trips_through_json() {
        let mut entry = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        entry.set_failure(FailureClass::AgentNoProgress, FailureStage::PostValidation);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"failure_class\":\"agent_no_progress\""));
        assert!(json.contains("\"failure_stage\":\"post_validation\""));
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.failure_class.as_deref(), Some("agent_no_progress"));
        assert_eq!(parsed.failure_stage.as_deref(), Some("post_validation"));
    }

    /// TICKET-063 requirement: existing historical JSONL entries — written
    /// before failure_class/failure_stage existed — must still deserialize.
    /// This is the exact fixture line used in
    /// tests/gah_cli.rs::ledger_summary_reports_recent_counts, which has no
    /// failure_class/failure_stage keys at all.
    #[test]
    fn pre_existing_ledger_line_without_failure_fields_still_deserializes() {
        let old_line = "{\"timestamp\":\"2099-01-01T00:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"claude\",\"requested_backend\":\"claude\",\"effective_backend\":\"claude\",\"requested_model\":null,\"effective_model\":null,\"routing_reason\":\"explicit\",\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"mode\":\"pm\",\"target_summary\":\"x\",\"branch\":null,\"session_dir\":null,\"duration_seconds\":1.0,\"backend_exit_code\":0,\"validation_result\":\"not_run\",\"commit_attempted\":false,\"commit_created\":false,\"push_attempted\":false,\"push_succeeded\":false,\"mr_attempted\":false,\"mr_created\":false,\"mr_url\":null,\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"usage_source\":null}}";
        let parsed: LedgerEntry = serde_json::from_str(old_line).unwrap();
        assert_eq!(parsed.failure_class, None);
        assert_eq!(parsed.failure_stage, None);
        assert_eq!(parsed.routing_diagnostics, None);
        assert_eq!(parsed.work_id, None);
        assert_eq!(parsed.work_title, None);
        assert!(parsed.review_blocking_findings.is_empty());
        assert!(parsed.review_non_blocking_findings.is_empty());
        assert!(parsed.review_risk_notes.is_empty());
        assert!(parsed.review_evidence.is_empty());
        assert!(parsed.review_compatibility_evidence.is_empty());
        assert_eq!(parsed.profile, "real");
    }

    #[test]
    fn legacy_ledger_line_deserializes_attempts_as_unknown() {
        let raw = r#"{
            "timestamp": "2026-07-01T00:00:00Z",
            "profile": "test",
            "display_name": "Repo",
            "repo_id": "repo",
            "repo": "owner/repo",
            "local_path": "/tmp/repo",
            "provider": "github",
            "backend": "codex",
            "requested_backend": "codex",
            "effective_backend": "codex",
            "mode": "fix",
            "commit_attempted": false,
            "commit_created": false,
            "push_attempted": false,
            "push_succeeded": false,
            "mr_attempted": false,
            "mr_created": false,
            "fallback_used": false,
            "human_required": false,
            "attempts": [],
            "usage": {}
        }"#;
        let entry: LedgerEntry = serde_json::from_str(raw).unwrap();
        assert_eq!(
            entry.schema_version, 1,
            "legacy entries default to schema v1"
        );
        assert_eq!(
            entry.attempts_started, None,
            "pre-tracking attempts_started must be unknown, not 0"
        );
        assert_eq!(entry.attempts_completed, None);
    }

    /// Issue #240: a v1 fixture that explicitly carried `0` attempt counters
    /// (i.e. the field was present) must round-trip as a *known* zero —
    /// distinct from the unknown (`None`) case above. This guards against the
    /// "convert unknown to zero" mistake.
    #[test]
    fn v1_ledger_line_with_zero_attempts_stays_known_zero() {
        let raw = r#"{
            "schema_version": 1,
            "timestamp": "2026-07-01T00:00:00Z",
            "profile": "test",
            "display_name": "Repo",
            "repo_id": "repo",
            "repo": "owner/repo",
            "local_path": "/tmp/repo",
            "provider": "github",
            "backend": "codex",
            "requested_backend": "codex",
            "effective_backend": "codex",
            "mode": "fix",
            "commit_attempted": false,
            "commit_created": false,
            "push_attempted": false,
            "push_succeeded": false,
            "mr_attempted": false,
            "mr_created": false,
            "fallback_used": false,
            "human_required": false,
            "attempts_started": 0,
            "attempts_completed": 0,
            "attempts": [],
            "usage": {}
        }"#;
        let entry: LedgerEntry = serde_json::from_str(raw).unwrap();
        assert_eq!(entry.schema_version, 1);
        assert_eq!(
            entry.attempts_started,
            Some(0),
            "explicit 0 is a known zero"
        );
        assert_eq!(entry.attempts_completed, Some(0));
    }

    /// Issue #240 acceptance #1: every newly constructed entry carries the
    /// current `LEDGER_SCHEMA_VERSION`.
    #[test]
    fn new_entries_carry_current_schema_version() {
        let prof = profile();
        let entry = LedgerEntry::new("test", &prof, "codex", "improve", "t", None, None);
        assert_eq!(entry.schema_version, super::LEDGER_SCHEMA_VERSION);
        assert_eq!(entry.attempts_started, Some(0));
        assert_eq!(entry.attempts_completed, Some(0));

        let claim = LedgerEntry::new_claim("test", &prof, "TICKET-1");
        assert_eq!(claim.schema_version, super::LEDGER_SCHEMA_VERSION);
        let hold = LedgerEntry::new_review_hold("test", &prof, "TICKET-1", None);
        assert_eq!(hold.schema_version, super::LEDGER_SCHEMA_VERSION);
        let clear = LedgerEntry::new_clear_attempts("test", &prof, "TICKET-1");
        assert_eq!(clear.schema_version, super::LEDGER_SCHEMA_VERSION);
    }

    // Issue #95: new_clear_attempts creates a valid tombstone entry
    #[test]
    fn new_clear_attempts_creates_valid_tombstone() {
        let prof = profile();
        let entry = LedgerEntry::new_clear_attempts("test-profile", &prof, "TICKET-99");
        assert_eq!(entry.mode, "clear_attempts");
        assert_eq!(entry.work_id.as_deref(), Some("TICKET-99"));
        assert_eq!(entry.profile, "test-profile");
        assert_eq!(entry.repo_id, prof.repo_id);
        assert!(!entry.human_required);
        assert_eq!(entry.failure_class, None);
        assert_eq!(entry.attempts_started, Some(0));
        // Must serialize/deserialize cleanly (JSONL round-trip)
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.mode, "clear_attempts");
        assert_eq!(parsed.work_id.as_deref(), Some("TICKET-99"));
    }

    #[test]
    fn new_claim_creates_valid_claim_entry() {
        let prof = profile();
        let entry = LedgerEntry::new_claim("test-profile", &prof, "TICKET-500");
        assert_eq!(entry.mode, "claim");
        assert_eq!(entry.work_id.as_deref(), Some("TICKET-500"));
        assert_eq!(entry.profile, "test-profile");
        assert!(!entry.human_required);
        assert_eq!(entry.branch, None);
        assert_eq!(entry.mr_url, None);
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.mode, "claim");
        assert_eq!(parsed.work_id.as_deref(), Some("TICKET-500"));
    }

    #[test]
    fn new_review_hold_creates_valid_hold_entry() {
        let prof = profile();
        let entry = LedgerEntry::new_review_hold(
            "test-profile",
            &prof,
            "TICKET-600",
            Some("reviewing PR #204".into()),
        );
        assert_eq!(entry.mode, "review_hold");
        assert_eq!(entry.work_id.as_deref(), Some("TICKET-600"));
        assert_eq!(entry.target_summary.as_deref(), Some("reviewing PR #204"));
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.mode, "review_hold");
    }
}
