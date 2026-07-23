use crate::availability;
use crate::config::{GahConfig, Profile};
use crate::controller::{
    plan_remediation, HumanRequiredReason, RemediationContext, RemediationPlan,
};
use crate::ledger::{self, LedgerEntry, RoutingDiagnostics};
use crate::sync;
use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

mod pm;
pub use self::pm::PmParentStatus;

mod gates;
mod intake;

fn effective_issue_intake_policy(profile: &Profile) -> crate::models::IssueIntakePolicy {
    let trusted_human_authors = profile
        .publishing
        .trusted_issue_human_authors
        .clone()
        .or_else(|| {
            profile
                .provider
                .eq_ignore_ascii_case("github")
                .then(|| profile.publishing.github_issue_author_allowlist.clone())
                .flatten()
        })
        .unwrap_or_else(|| {
            if profile.provider.eq_ignore_ascii_case("github") {
                profile
                    .repo
                    .split_once('/')
                    .map(|(owner, _)| vec![owner.to_string()])
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        });
    let trusted_bot_authors = profile
        .publishing
        .trusted_issue_bot_authors
        .clone()
        .unwrap_or_default();
    crate::models::IssueIntakePolicy {
        mode: profile.publishing.issue_intake_mode.as_str().to_string(),
        canonical_autonomous_label: profile.publishing.canonical_autonomous_label.clone(),
        trusted_human_authors,
        trusted_bot_authors,
        github_issue_author_allowlist: profile
            .publishing
            .github_issue_author_allowlist
            .clone()
            .unwrap_or_default(),
    }
}

#[derive(Serialize, Clone)]
pub struct StatusSnapshot {
    /// Status schema v1 permits additive fields; consumers must ignore fields
    /// they do not recognize. Increment this only for an incompatible removal,
    /// rename, or semantic/type change.
    pub schema_version: u32,
    #[serde(default = "default_review_contract_version")]
    pub review_contract_version: u32,
    pub generated_at: String,
    pub profile: ProfileIdentity,
    pub observations: Observations,
    pub merge_requests: Vec<sync::SyncMrJson>,
    pub availability: Vec<ScopeStatusJson>,
    pub recent_ledger: Option<RecentLedgerSummary>,
    pub constraints: Vec<Blocker>,
    /// Genuine profile-wide blockers (sync failure, infra unavailable, auth
    /// failure with no viable route, etc.) that halt ALL work. A
    /// ticket-scoped `human_required` ledger entry must NOT appear here --
    /// it is tracked per work item in `blocked_work_items` instead.
    pub blockers: Vec<Blocker>,
    /// Work items awaiting human action (e.g. a ticket/MR review verdict
    /// with `human_required`). Scoped to the work item(s) it affects; other
    /// eligible work remains dispatchable. Separated from `blockers` so
    /// `gah status` and the controller can distinguish a global freeze from
    /// a single blocked ticket.
    pub blocked_work_items: Vec<Blocker>,
    /// Issue intake rejections observed during recurring discovery. These are
    /// surfaced alongside the accepted tickets so a triage operator can see
    /// why a particular issue was not deemed dispatchable.
    pub issue_intake_rejections: Vec<crate::models::IssueIntakeRejection>,
    /// Native issues excluded because their canonical same-project
    /// prerequisites are unresolved or invalid.
    pub dependency_blockers: Vec<crate::models::DependencyBlocker>,
    pub errors: Vec<StatusError>,
    /// TICKET-078: dispatch candidates from `docs/tickets/`, feeding
    /// `decide_next_action`'s DispatchTicket/Retry/Escalate rules.
    pub available_tickets: Vec<crate::models::AvailableTicket>,
    /// Active durable claims keyed by canonical profile+repo scope.
    pub active_claims: Vec<ActiveClaimSnapshot>,
    /// Bounded PM orchestration history and provider-native child state.
    pub pm_parent_states: Vec<PmParentStatus>,
    pub pm_decomposition_attempt_counts: std::collections::HashMap<String, usize>,
    pub pm_max_attempts: u32,
    /// TICKET-118: fix attempt counts per branch for retry cap.
    pub fix_attempt_counts: std::collections::HashMap<String, usize>,
    /// TICKET-127: merge attempt counts per branch for the auto-merge
    /// retry cap.
    pub merge_attempt_counts: std::collections::HashMap<String, usize>,
    /// work_ids currently under an active `gah hold set` -- a manager
    /// session (human or supervising Claude/Codex/Hermes) reviewing the
    /// work_id's PR out of band. The controller must not auto-merge these
    /// out from under an in-progress review; see `decide_next_action`'s
    /// READY_FOR_HUMAN arm.
    pub review_held_work_ids: std::collections::HashSet<String>,
    /// TICKET-128: per-profile publishing policy. When PR/MR creation is
    /// disabled, the controller must never enter the auto-merge path even
    /// when a strong reviewer has approved and CI is green. This is an
    /// independent axis from reviewer routing and merge policy.
    pub publishing_allow_pr: bool,
    /// Effective pre-publication generated-artifact deny patterns for this
    /// profile. Exposed so CLI/API/dashboard clients can explain a blocked
    /// commit without reimplementing config defaults.
    pub generated_artifact_deny_patterns: Vec<String>,
    /// How many `gah loop` workers may run concurrently for this profile
    /// (see `Profile::max_parallel_workers`). Read by `gah-supervisor.sh`
    /// to decide how many worker loops to launch when not given explicitly
    /// on its own command line.
    pub max_parallel_workers: u32,
    /// Provider-neutral implementation intake backpressure. Open managed MRs
    /// and in-flight implementation claims consume this limit; lifecycle work
    /// continues while intake is paused.
    pub open_managed_mr_count: u32,
    pub inflight_implementation_count: u32,
    pub implementation_intake_paused: bool,
    /// TICKET-157: per-backend "configured for this profile" signal. Keyed
    /// by logical backend name. `true` means the backend has a real Rust
    /// implementation AND is set up for the active profile (an explicit
    /// path or profile marker is configured). This lets Settings distinguish
    /// "implemented but not set up for this profile" from "implemented and
    /// ready" rather than conflating "not explicitly marked unavailable" with
    /// "available". Backends with no implementation (e.g. grok/cursor) are
    /// simply absent from this map and should be reported as not_implemented
    /// by the frontend.
    pub backend_configured: std::collections::HashMap<String, bool>,
    /// Effective provider-neutral instance identities. Runtime paths and
    /// credential values are deliberately excluded from this projection.
    pub backend_instances: Vec<crate::config_show::BackendInstanceSummary>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProfileIdentity {
    pub profile: String,
    pub display_name: String,
    pub repo_id: String,
    pub provider: String,
    pub local_path: String,
    pub default_target_branch: String,
    /// Resolved per-repo merge policy (Issue #124 / TICKET-127). Inherits the
    /// canonical/defaults policy when the profile doesn't set its own.
    pub merge_policy: crate::config::MergePolicy,
    /// Effective cap for automatic post-review repair runs. Kept in the
    /// snapshot so controller decisions and dashboard blockers agree.
    pub max_fix_attempts_per_mr: u32,
    /// Genuine implementation failures allowed while walking the configured
    /// backend/model ladder. Separate from post-review repairs.
    pub max_implementation_failures_per_ticket: u32,
    /// Effective open managed PR/MR limit for this profile.
    pub max_open_managed_mrs: u32,
    /// Effective issue intake policy for this profile.
    pub issue_intake_policy: crate::models::IssueIntakePolicy,
}

#[derive(Serialize, Clone)]
pub struct Observations {
    pub sync: ObservationStatus,
    pub availability: ObservationStatus,
    pub ledger: ObservationStatus,
}

#[derive(Serialize, Clone)]
pub struct ObservationStatus {
    pub status: &'static str,
}

#[derive(Serialize, Clone)]
pub struct ScopeStatusJson {
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    pub eligible_now: bool,
    pub reason: Option<String>,
    pub unavailable_until: Option<String>,
    pub source: Option<String>,
    pub last_error_summary: Option<String>,
    pub observed_at: Option<String>,
    pub scope: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct RecentLedgerSummary {
    pub most_recent_dispatch_timestamp: String,
    pub most_recent_effective_backend: String,
    pub most_recent_effective_model: Option<String>,
    pub most_recent_work_id: Option<String>,
    pub most_recent_mode: String,
    pub most_recent_validation_result: Option<String>,
    pub most_recent_failure_class: Option<String>,
    pub most_recent_failure_stage: Option<String>,
    pub most_recent_branch: Option<String>,
    pub most_recent_mr_url: Option<String>,
    pub attempts_started: Option<u32>,
    pub attempts_completed: Option<u32>,
    pub human_required: bool,
    pub review_timeout_class: Option<String>,
    pub review_idle_timeout_seconds: Option<u64>,
    pub review_hard_timeout_seconds: Option<u64>,
    pub review_last_progress_secs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_diagnostics: Option<RoutingDiagnostics>,
}

#[derive(Serialize, Clone)]
pub struct ActiveClaimSnapshot {
    pub work_id: String,
    pub pid: u32,
    pub scope: String,
    pub hostname: String,
    pub claimed_at: String,
    pub age_seconds: u64,
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct Blocker {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_reference: Option<String>,
    /// TICKET-505: stable reason code for why autonomy stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation_plan: Option<RemediationPlan>,
}

fn remediation_plan_for_blocker(
    profile_name: &str,
    profile: &Profile,
    blocker_kind: &str,
    source_reference: Option<&str>,
    reason_code: Option<&str>,
    backend: Option<&str>,
    model: Option<&str>,
) -> Option<RemediationPlan> {
    let plan = match blocker_kind {
        "human_required" => plan_remediation(RemediationContext {
            profile_name,
            profile,
            work_id: source_reference,
            reference: source_reference,
            reason_code: reason_code
                .map(HumanRequiredReason::from_code)
                .unwrap_or(HumanRequiredReason::Unknown),
            blocker_kind: Some(blocker_kind),
            backend,
            model,
        }),
        "backend_unavailable" => plan_remediation(RemediationContext {
            profile_name,
            profile,
            work_id: source_reference,
            reference: source_reference,
            reason_code: HumanRequiredReason::ConfigurationInfra,
            blocker_kind: Some(blocker_kind),
            backend,
            model,
        }),
        _ => return None,
    };
    Some(plan)
}

#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
pub struct StatusError {
    pub subsystem: String,
    pub message: String,
    pub incomplete_snapshot: bool,
}

pub fn build_snapshot(
    cfg: &GahConfig,
    profile_name: &str,
    now: OffsetDateTime,
) -> Result<StatusSnapshot> {
    match ledger::read_entries(cfg) {
        Ok(entries) => build_snapshot_inner(cfg, profile_name, now, &entries, None),
        Err(error) => build_snapshot_inner(cfg, profile_name, now, &[], Some(format!("{error:#}"))),
    }
}

pub fn build_snapshot_from_entries(
    cfg: &GahConfig,
    profile_name: &str,
    now: OffsetDateTime,
    entries: &[LedgerEntry],
) -> Result<StatusSnapshot> {
    build_snapshot_inner(cfg, profile_name, now, entries, None)
}

fn build_snapshot_inner(
    cfg: &GahConfig,
    profile_name: &str,
    now: OffsetDateTime,
    entries: &[LedgerEntry],
    ledger_error: Option<String>,
) -> Result<StatusSnapshot> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let generated_at = now.format(&Rfc3339).unwrap_or_default();

    let effective_routing = profile.effective_routing(&cfg.defaults);
    let resolved_merge_policy = effective_routing.merge_policy.unwrap_or_default();
    let profile_identity = ProfileIdentity {
        profile: profile_name.to_string(),
        display_name: profile.display_name.clone(),
        repo_id: profile.repo_id.clone(),
        provider: profile.provider.clone(),
        local_path: profile.local_path.clone(),
        default_target_branch: profile.default_target_branch.clone(),
        merge_policy: resolved_merge_policy,
        max_fix_attempts_per_mr: effective_routing.max_fix_attempts_per_mr(),
        max_implementation_failures_per_ticket: effective_routing
            .max_implementation_failures_per_ticket(),
        max_open_managed_mrs: profile.max_open_managed_mrs(),
        issue_intake_policy: effective_issue_intake_policy(profile),
    };

    let mut errors = Vec::new();
    let (pm_parent_states, pm_decomposition_attempt_counts, pm_error) =
        pm::project(profile, profile_name, entries);
    if let Some(message) = pm_error {
        errors.push(StatusError {
            subsystem: "pm_reconciliation".into(),
            message,
            incomplete_snapshot: false,
        });
    }

    // TICKET-127: Count merge attempts per branch for the auto-merge retry cap
    let merge_attempt_counts =
        sync::count_merge_attempts_per_branch_for_scope(entries, profile_name, &profile.repo_id);
    // review-hold: work_ids a manager session is actively reviewing via
    // `gah hold set`, out of band from gah's own auto-merge loop.
    let review_held_work_ids =
        ledger::active_review_hold_work_ids_from_entries(entries, profile_name);

    // 1. Sync State
    let mut merge_requests = Vec::new();
    let mut raw_mrs: Vec<sync::SyncMr> = Vec::new();
    let mut sync_obs = ObservationStatus { status: "ok" };

    // Ledger read is hoisted above the sync step so recently-merged MRs can be
    // enriched with their backend/model and review verdict (TICKET-198).
    let ledger_entries_by_work_id = ledger::index_entries_by_work_id(entries);
    match sync::fetch_active_mrs(profile) {
        Ok(mrs) => {
            merge_requests = mrs
                .iter()
                .map(|mr| sync::sync_mr_to_json(mr, None, &ledger_entries_by_work_id))
                .collect();
            raw_mrs = mrs;
        }
        Err(e) => {
            sync_obs.status = "error";
            errors.push(StatusError {
                subsystem: "sync".into(),
                message: format!("{:#}", e),
                incomplete_snapshot: true,
            });
        }
    }
    // Review-derived repair budgets are scoped to the exact active contract,
    // source SHA, and metadata generation. Historical attempts remain in the
    // ledger for telemetry but cannot exhaust a fresh generation's budget.
    let fix_attempt_counts = sync::count_current_fix_attempts_for_mrs(
        entries,
        profile_name,
        &profile.repo_id,
        &merge_requests,
    );

    // 2. Availability State
    let state_path = availability::resolve_state_path();
    let mut availability_list = Vec::new();
    let mut avail_obs = ObservationStatus { status: "ok" };
    match availability::list_scopes(&state_path, now) {
        Ok(scopes) => {
            availability_list = scopes
                .into_iter()
                .map(|s| ScopeStatusJson {
                    backend: s.backend,
                    backend_instance: s.backend_instance,
                    model: s.model,
                    quota_pool: s.quota_pool,
                    eligible_now: s.eligible,
                    reason: s.reason.map(|r| r.as_str().to_string()),
                    unavailable_until: s.unavailable_until,
                    source: s.source.map(|r| r.as_str().to_string()),
                    last_error_summary: s.last_error_summary,
                    observed_at: s.observed_at,
                    scope: s.scope.map(|s| match s {
                        availability::BlockScope::BackendWide => "backend_wide".into(),
                        availability::BlockScope::ModelSpecific => "model_specific".into(),
                        availability::BlockScope::QuotaPool => "quota_pool".into(),
                    }),
                })
                .collect();
        }
        Err(e) => {
            avail_obs.status = "error";
            errors.push(StatusError {
                subsystem: "availability".into(),
                message: format!("{:#}", e),
                incomplete_snapshot: true,
            });
        }
    }

    // 3. Ledger State
    let mut recent_ledger = None;
    let mut ledger_obs = ObservationStatus { status: "ok" };
    if let Some(message) = ledger_error {
        ledger_obs.status = "error";
        errors.push(StatusError {
            subsystem: "ledger".into(),
            message,
            incomplete_snapshot: true,
        });
    }
    {
        let mut latest: Option<&LedgerEntry> = None;
        let mut max_ts: Option<OffsetDateTime> = None;

        for entry in entries.iter().filter(|e| e.profile == profile_name) {
            if let Ok(ts) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
                match max_ts {
                    Some(m) if ts > m => {
                        max_ts = Some(ts);
                        latest = Some(entry);
                    }
                    None => {
                        max_ts = Some(ts);
                        latest = Some(entry);
                    }
                    _ => {}
                }
            } else if latest.is_none() {
                latest = Some(entry);
            }
        }

        if let Some(entry) = latest {
            recent_ledger = Some(RecentLedgerSummary {
                most_recent_dispatch_timestamp: entry.timestamp.clone(),
                most_recent_effective_backend: entry.effective_backend.clone(),
                most_recent_effective_model: entry.effective_model.clone(),
                most_recent_work_id: entry.work_id.clone(),
                most_recent_mode: entry.mode.clone(),
                most_recent_validation_result: entry.validation_result.clone(),
                most_recent_failure_class: entry.failure_class.clone(),
                most_recent_failure_stage: entry.failure_stage.clone(),
                most_recent_branch: entry.branch.clone(),
                most_recent_mr_url: entry.mr_url.clone(),
                attempts_started: entry.attempts_started,
                attempts_completed: entry.attempts_completed,
                human_required: entry.human_required,
                review_timeout_class: entry.review_timeout_class.clone(),
                review_idle_timeout_seconds: entry.review_idle_timeout_seconds,
                review_hard_timeout_seconds: entry.review_hard_timeout_seconds,
                review_last_progress_secs: entry.review_last_progress_secs,
                routing_diagnostics: entry.routing_diagnostics.clone(),
            });
        }
    }

    // 4. Blockers and Constraints
    let mut constraints = Vec::new();
    let blockers = Vec::new();

    for avail in &availability_list {
        if !avail.eligible_now {
            constraints.push(Blocker {
                kind: "backend_unavailable".into(),
                reason: avail.reason.clone(),
                message: None,
                backend: Some(avail.backend.clone()),
                model: avail.model.clone(),
                until: avail.unavailable_until.clone(),
                source_reference: None,
                reason_code: None,
                remediation_plan: remediation_plan_for_blocker(
                    profile_name,
                    profile,
                    "backend_unavailable",
                    None,
                    None,
                    Some(avail.backend.as_str()),
                    avail.model.as_deref(),
                ),
            });
        }
    }

    // Removed all_backends_unavailable blocker check. Status has no routing context (mode),
    // so it correctly falls back to emitting individual availability constraints only.

    // TICKET-human-required-scoping: a ticket/MR review verdict with
    // `human_required = true` is WORK-ITEM SCOPED, never profile-wide. A
    // single blocked ticket must not freeze unrelated work. Derive the
    // effective `human_required` for each available ticket from its own
    // ledger history (canonical helper `ledger_lookup_for_ticket`) and
    // record it against that work item only. The newest ledger entry no
    // longer implicitly defines a profile-wide `human_required` blocker.
    // Genuine profile-wide blockers (sync failure, infra unavailable, auth
    // failure with no viable route) are still emitted above via
    // `availability`-derived constraints, NOT here.
    let mut blocked_work_items: Vec<Blocker> = Vec::new();

    // 5. Available tickets (TICKET-078): reuses the already-fetched `raw_mrs`
    // rather than calling sync::fetch_mrs a second time.
    let ticket_scan = crate::dispatch::scan_available_tickets_with_dependencies(
        profile,
        &raw_mrs,
        &ledger_entries_by_work_id,
    );
    let mut available_tickets = ticket_scan.available_tickets;
    let dependency_blockers = ticket_scan.dependency_blockers;
    let issue_intake_rejections = ticket_scan.issue_intake_rejections;
    let published_pm_work_ids = pm_parent_states
        .iter()
        .map(|parent| parent.work_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for issue in issue_intake_rejections
        .iter()
        .filter(|issue| issue.reason_code == "planning")
    {
        let Some(work_id) = issue.work_id.as_deref() else {
            continue;
        };
        let attempts = pm_decomposition_attempt_counts
            .get(work_id)
            .copied()
            .unwrap_or_default();
        if attempts >= profile.publishing.pm_max_attempts()
            && !published_pm_work_ids.contains(work_id)
        {
            blocked_work_items.push(Blocker {
                kind: "human_required".into(),
                reason: Some("retry_budget_exhausted".into()),
                message: Some(format!(
                    "{work_id} exhausted {attempts} bounded PM decomposition attempt(s)"
                )),
                backend: None,
                model: None,
                until: None,
                source_reference: Some(work_id.to_string()),
                reason_code: Some("retry_budget_exhausted".into()),
                remediation_plan: remediation_plan_for_blocker(
                    profile_name,
                    profile,
                    "human_required",
                    Some(work_id),
                    Some("retry_budget_exhausted"),
                    None,
                    None,
                ),
            });
        }
    }
    if let Some(error) = ticket_scan.provider_error {
        errors.push(StatusError {
            subsystem: "issue_intake".into(),
            message: error,
            // Ticket intake is fail-closed below, but MR review/repair must
            // remain processable from the independently successful sync.
            incomplete_snapshot: false,
        });
    }
    // A paid-route approval request is conditional, not permanent. If a
    // subscription/local route has recovered since the failed dispatch,
    // release the derived ticket hold without rewriting ledger history.
    for ticket in &mut available_tickets {
        let Some(work_id) = ticket.work_id.as_deref() else {
            continue;
        };
        let Some(gate) = ledger::effective_human_gate_from_entries(
            entries,
            profile_name,
            &profile.repo_id,
            work_id,
        ) else {
            continue;
        };
        if !gates::policy_approval_still_required(
            cfg,
            profile_name,
            profile,
            entries,
            work_id,
            &gate,
        ) {
            ticket.human_required = false;
            ticket.human_required_reason_code = None;
        }
    }
    // Load durable claim state once and apply active-claim flags directly from
    // the canonical `<profile>@<repo_id>` scope to avoid stale CLI/runtime
    // mismatch and per-ticket file reads.
    let claim_scope = crate::work_claim::canonical_claim_scope(profile_name, &profile.repo_id);
    let mut active_claim_work_ids = std::collections::HashSet::new();
    let mut active_claims = Vec::new();
    match crate::work_claim::claim_details_for_profile(&claim_scope) {
        Ok(claims) => {
            let now = Utc::now();
            for claim in claims {
                if claim.is_stale {
                    continue;
                }
                let age_seconds = now
                    .signed_duration_since(claim.claimed_at)
                    .num_seconds()
                    .max(0) as u64;
                active_claim_work_ids
                    .insert(crate::work_claim::normalize_work_identity(&claim.work_id));
                active_claims.push(ActiveClaimSnapshot {
                    work_id: claim.work_id,
                    pid: claim.pid,
                    scope: claim_scope.clone(),
                    hostname: claim.hostname,
                    claimed_at: claim.claimed_at.to_rfc3339(),
                    age_seconds,
                });
            }
        }
        Err(error) => errors.push(StatusError {
            subsystem: "claims".into(),
            message: format!("{:#}", error),
            incomplete_snapshot: true,
        }),
    }
    for ticket in &mut available_tickets {
        if let Some(work_id) = ticket.work_id.as_deref() {
            if active_claim_work_ids.contains(&crate::work_claim::normalize_work_identity(work_id))
            {
                ticket.has_active_claim = true;
            }
        }
    }

    // TICKET-human-required-scoping: after the per-ticket human_required is
    // derived (in scan_available_tickets via ledger_lookup_for_ticket), record
    // each blocked work item in `blocked_work_items` so it stays visible in
    // status output without freezing the whole profile. Tickets whose
    // human_required state has since cleared are no longer shown as blocked.
    for ticket in &available_tickets {
        if ticket.human_required {
            let reason_code = ticket.human_required_reason_code.clone();
            blocked_work_items.push(Blocker {
                kind: "human_required".into(),
                reason: reason_code.clone().or(Some("ledger_human_required".into())),
                message: Some("Ledger indicates human intervention required".into()),
                backend: None,
                model: None,
                until: None,
                source_reference: ticket.work_id.clone(),
                reason_code,
                remediation_plan: remediation_plan_for_blocker(
                    profile_name,
                    profile,
                    "human_required",
                    ticket.work_id.as_deref(),
                    ticket.human_required_reason_code.as_deref(),
                    None,
                    None,
                ),
            });
        }
    }

    // Project durable ledger gates directly onto every active PR/MR. This is
    // deliberately independent of available_tickets: dependency filtering or
    // issue-intake policy may hide the source issue while its already-open MR
    // still needs repair/review. Losing the gate in that state caused the
    // controller to redispatch and notify the same blocked MR every tick.
    gates::project_effective_mr_gates(
        cfg,
        profile_name,
        profile,
        entries,
        &merge_requests,
        &mut blocked_work_items,
    );
    for dependency in &dependency_blockers {
        blocked_work_items.push(Blocker {
            kind: "dependency".into(),
            reason: Some(dependency.reason_code.clone()),
            message: Some(dependency.reason.clone()),
            backend: None,
            model: None,
            until: None,
            source_reference: Some(dependency.work_id.clone()),
            reason_code: Some(dependency.reason_code.clone()),
            remediation_plan: None,
        });
    }

    // Project retry-cap-blocked MRs into blocked_work_items. An MR classified
    // NEEDS_FIX whose fix_attempt_counts reach the effective repair cap will be returned as
    // HumanRequired by decide_next_action, but this is a controller decision
    // not a ledger human_required flag — without this projection, gah status
    // shows no blockers while the supervisor pings human_required every cycle.
    let fix_retry_cap = profile_identity.max_fix_attempts_per_mr as usize;
    for mr in &merge_requests {
        if matches!(mr.classification.as_str(), "CI_FAILED" | "NEEDS_FIX") {
            let attempts = fix_attempt_counts.get(&mr.branch).copied().unwrap_or(0);
            if attempts >= fix_retry_cap {
                blocked_work_items.push(Blocker {
                    kind: "human_required".into(),
                    reason: Some("fix_retry_cap_exceeded".into()),
                    message: Some(format!(
                        "MR on branch '{}' classified {} but fix retry cap ({}) exceeded",
                        mr.branch, mr.classification, fix_retry_cap
                    )),
                    backend: None,
                    model: None,
                    until: None,
                    source_reference: Some(mr.branch.clone()),
                    reason_code: Some(HumanRequiredReason::FixRetryCapExceeded.as_str().into()),
                    remediation_plan: remediation_plan_for_blocker(
                        profile_name,
                        profile,
                        "human_required",
                        mr.work_id.as_deref().or(Some(mr.branch.as_str())),
                        Some(HumanRequiredReason::FixRetryCapExceeded.as_str()),
                        None,
                        None,
                    ),
                });
            }
        }
    }

    // Same projection as above, for the merge side: an approved, non-draft,
    // CI-passing MR whose merge_attempt_counts reach AUTO_RETRY_CAP is
    // returned as HumanRequired(MergeRetryCapExceeded) by decide_next_action
    // (decision.rs), but under a non-StopForHuman merge policy that decision
    // never reaches the surfaced HumanRequired branch -- it silently no-ops
    // every tick instead. Without this projection an already-approved,
    // green-CI MR sits permanently unmerged and invisible in `gah status`.
    for mr in &merge_requests {
        if mr.classification == "READY_FOR_HUMAN" && !mr.draft && mr.ci_passed {
            let attempts = merge_attempt_counts.get(&mr.branch).copied().unwrap_or(0);
            if attempts >= crate::controller::AUTO_RETRY_CAP {
                blocked_work_items.push(Blocker {
                    kind: "human_required".into(),
                    reason: Some("merge_retry_cap_exceeded".into()),
                    message: Some(format!(
                        "MR on branch '{}' classified {} but merge retry cap ({}) exceeded",
                        mr.branch,
                        mr.classification,
                        crate::controller::AUTO_RETRY_CAP
                    )),
                    backend: None,
                    model: None,
                    until: None,
                    source_reference: Some(mr.branch.clone()),
                    reason_code: Some(HumanRequiredReason::MergeRetryCapExceeded.as_str().into()),
                    remediation_plan: remediation_plan_for_blocker(
                        profile_name,
                        profile,
                        "human_required",
                        mr.work_id.as_deref().or(Some(mr.branch.as_str())),
                        Some(HumanRequiredReason::MergeRetryCapExceeded.as_str()),
                        None,
                        None,
                    ),
                });
            }
        }
    }

    // TICKET-157: build the per-backend "configured for this profile" signal.
    // Only backends with a real Rust implementation are listed. `configured`
    // is true when the profile sets up this backend (an explicit executable
    // path or profile marker). `configured_path` echoes the configured marker
    // for display. Backends with no implementation (grok/cursor) are omitted
    // entirely so the frontend can show them as not_implemented.
    let implemented_backends = [
        "codex",
        "claude",
        "agy",
        "agy-main",
        "agy-second",
        "vibe",
        "opencode",
        "openhands",
    ];
    let mut backend_configured: std::collections::HashMap<String, bool> =
        std::collections::HashMap::new();
    for backend in implemented_backends {
        let configured = profile.is_backend_configured_with_defaults(&cfg.defaults, backend);
        backend_configured.insert(backend.to_string(), configured);
    }
    let effective_routing = profile.effective_routing(&cfg.defaults);
    let backend_instances = crate::config_show::backend_instance_summaries(&effective_routing);
    for instance in effective_routing.backend_instances.into_values() {
        let configured = instance.executable.is_some();
        let backend = instance
            .logical_backend
            .unwrap_or_else(|| instance.runner_kind.clone());
        backend_configured
            .entry(backend)
            .and_modify(|current| *current |= configured)
            .or_insert(configured);
    }

    let intake = intake::project(
        &merge_requests,
        &active_claims,
        profile_identity.max_open_managed_mrs,
    );

    let snapshot = StatusSnapshot {
        schema_version: 1,
        review_contract_version: crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION,
        generated_at,
        profile: profile_identity,
        observations: Observations {
            sync: sync_obs,
            availability: avail_obs,
            ledger: ledger_obs,
        },
        merge_requests,
        availability: availability_list,
        recent_ledger,
        constraints,
        blockers,
        blocked_work_items,
        issue_intake_rejections,
        dependency_blockers,
        errors,
        available_tickets,
        active_claims,
        pm_parent_states,
        pm_decomposition_attempt_counts,
        pm_max_attempts: profile.publishing.pm_max_attempts() as u32,
        fix_attempt_counts,
        merge_attempt_counts,
        review_held_work_ids,
        publishing_allow_pr: profile.publishing.allow_pull_request_creation,
        generated_artifact_deny_patterns: profile
            .publishing
            .generated_artifact_deny_patterns
            .clone(),
        max_parallel_workers: profile.max_parallel_workers(),
        open_managed_mr_count: intake.open_mrs,
        inflight_implementation_count: intake.inflight_implementations,
        implementation_intake_paused: intake.paused,
        backend_configured,
        backend_instances,
    };

    Ok(snapshot)
}

pub fn run(cfg: &GahConfig, profile_name: &str, json: bool) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let snapshot = build_snapshot(cfg, profile_name, now)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
    } else {
        println!("Status for Profile: {}", profile_name);
        println!(
            "Observations: Sync={}, Availability={}, Ledger={}",
            snapshot.observations.sync.status,
            snapshot.observations.availability.status,
            snapshot.observations.ledger.status
        );
        println!(
            "Implementation intake: {} (open MRs={}, in-flight={}, limit={})",
            if snapshot.implementation_intake_paused {
                "paused; draining lifecycle work"
            } else {
                "open"
            },
            snapshot.open_managed_mr_count,
            snapshot.inflight_implementation_count,
            snapshot.profile.max_open_managed_mrs
        );
        if !snapshot.backend_instances.is_empty() {
            println!("Backend instances:");
            for instance in &snapshot.backend_instances {
                println!(
                    "  - {}: runner={} backend={} executable={} isolated_state={}",
                    instance.backend_instance,
                    instance.runner_kind,
                    instance.logical_backend,
                    instance.executable_configured,
                    instance.isolated_state_configured
                );
            }
        }

        if snapshot.blockers.is_empty() {
            println!("Blockers: None");
        } else {
            println!("Blockers:");
            for b in &snapshot.blockers {
                println!(
                    "  - {}: {}",
                    b.kind,
                    b.message
                        .as_deref()
                        .unwrap_or(b.reason.as_deref().unwrap_or("unknown"))
                );
            }
        }

        if !snapshot.constraints.is_empty() {
            println!("Constraints:");
            for c in &snapshot.constraints {
                println!(
                    "  - {}: {}",
                    c.backend.as_deref().unwrap_or(""),
                    c.reason.as_deref().unwrap_or("unknown")
                );
            }
        }

        if !snapshot.errors.is_empty() {
            println!("Errors:");
            for e in &snapshot.errors {
                println!("  - [{}] {}", e.subsystem, e.message);
            }
        }

        if !snapshot.dependency_blockers.is_empty() {
            println!("Dependency-blocked work:");
            for blocked in &snapshot.dependency_blockers {
                let states = blocked
                    .dependencies
                    .iter()
                    .map(|dependency| {
                        format!(
                            "{}={} ({})",
                            dependency.identity,
                            dependency.normalized_state,
                            dependency.provider_state.as_deref().unwrap_or("unknown")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "  - {} [{}]: {}{}",
                    blocked.work_id,
                    blocked.reason_code,
                    blocked.reason,
                    if states.is_empty() {
                        String::new()
                    } else {
                        format!("; {states}")
                    }
                );
                println!("      eligible when: {}", blocked.eligible_when);
            }
        }

        if let Some(ledger) = &snapshot.recent_ledger {
            if let Some(diag) = &ledger.routing_diagnostics {
                println!("Recent Routing:");
                if let Some(summary) = &diag.human_summary {
                    println!("  {}", summary);
                }
                for candidate in &diag.candidates {
                    let mut line = format!(
                        "  - {}",
                        match &candidate.model {
                            Some(model) => format!("{}/{}", candidate.backend, model),
                            None => candidate.backend.clone(),
                        }
                    );
                    if let Some(pool) = &candidate.quota_pool {
                        line.push_str(&format!(" pool={pool}"));
                    }
                    if let Some(pace) = &candidate.pace_band {
                        line.push_str(&format!(" pace={pace}"));
                    }
                    if let Some(cost_class) = &candidate.cost_class {
                        line.push_str(&format!(" cost={cost_class}"));
                    }
                    if let Some(skip_reason) = &candidate.skip_reason {
                        line.push_str(&format!(" skipped={skip_reason}"));
                    }
                    println!("{line}");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
