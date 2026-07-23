use crate::models::AvailableTicket;
use crate::status::{ObservationStatus, Observations, ProfileIdentity, StatusSnapshot};
use crate::sync::{RecommendedAction, SyncMrJson};

pub(super) fn empty_snapshot() -> StatusSnapshot {
    StatusSnapshot {
        schema_version: 1,
        review_contract_version: crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION,
        generated_at: "2026-07-05T00:00:00Z".into(),
        profile: ProfileIdentity {
            profile: "real".into(),
            display_name: "Real".into(),
            repo_id: "real".into(),
            provider: "github".into(),
            local_path: "/tmp/repo".into(),
            default_target_branch: "main".into(),
            merge_policy: crate::config::MergePolicy::default(),
            max_fix_attempts_per_mr: 2,
            max_implementation_failures_per_ticket: 2,
            max_open_managed_mrs: 1,
            issue_intake_policy: crate::models::IssueIntakePolicy {
                mode: "canonical_autonomous_only".into(),
                canonical_autonomous_label: "exec:autonomous".into(),
                trusted_human_authors: vec![],
                trusted_bot_authors: vec![],
                github_issue_author_allowlist: vec![],
            },
        },
        observations: Observations {
            sync: ObservationStatus { status: "ok" },
            availability: ObservationStatus { status: "ok" },
            ledger: ObservationStatus { status: "ok" },
        },
        merge_requests: vec![],
        availability: vec![],
        recent_ledger: None,
        constraints: vec![],
        blockers: vec![],
        blocked_work_items: vec![],
        issue_intake_rejections: vec![],
        dependency_blockers: vec![],
        errors: vec![],
        available_tickets: vec![],
        active_claims: vec![],
        pm_parent_states: vec![],
        pm_decomposition_attempt_counts: std::collections::HashMap::new(),
        pm_max_attempts: 2,
        fix_attempt_counts: std::collections::HashMap::new(),
        merge_attempt_counts: std::collections::HashMap::new(),
        review_held_work_ids: std::collections::HashSet::new(),
        publishing_allow_pr: true,
        generated_artifact_deny_patterns: vec![],
        max_parallel_workers: 1,
        open_managed_mr_count: 0,
        inflight_implementation_count: 0,
        implementation_intake_paused: false,
        backend_configured: std::collections::HashMap::new(),
        backend_instances: vec![],
    }
}

pub(super) fn mr(branch: &str, classification: &str) -> SyncMrJson {
    mr_with_ci(branch, classification, false)
}

pub(super) fn mr_with_ci(branch: &str, classification: &str, ci_passed: bool) -> SyncMrJson {
    SyncMrJson {
        profile: None,
        branch: branch.into(),
        work_id: Some(format!("TICKET-{branch}")),
        id: Some("1".into()),
        url: Some(format!("https://example/{branch}")),
        state: Some("OPEN".into()),
        draft: false,
        merge_status: None,
        merged: classification == "MERGED",
        merged_at: None,
        ci_passed,
        ci_pending: false,
        title: None,
        effective_backend: None,
        effective_model: None,
        review_verdict: None,
        review_gate_reason: None,
        source_sha: None,
        review_contract_version: crate::ledger::REVIEW_CONTRACT_VERSION,
        review_generation: None,
        review_generation_status: None,
        classification: classification.into(),
        recommended_action: RecommendedAction::from_class(classification),
    }
}

/// Issue #156: a `READY_FOR_HUMAN` MR whose CI is non-terminal / unknown
/// (GitLab `head_pipeline` gap: running/pending/missing). `ci_passed` is
/// false but `ci_pending` is true, so it must surface as a re-check rather
/// than silently no-op.
pub(super) fn mr_ci_pending(branch: &str, classification: &str) -> SyncMrJson {
    SyncMrJson {
        profile: None,
        branch: branch.into(),
        work_id: Some(format!("TICKET-{branch}")),
        id: Some("1".into()),
        url: Some(format!("https://example/{branch}")),
        state: Some("OPEN".into()),
        draft: false,
        merge_status: None,
        merged: classification == "MERGED",
        merged_at: None,
        ci_passed: false,
        ci_pending: true,
        title: None,
        effective_backend: None,
        effective_model: None,
        review_verdict: None,
        review_gate_reason: None,
        source_sha: None,
        review_contract_version: crate::ledger::REVIEW_CONTRACT_VERSION,
        review_generation: None,
        review_generation_status: None,
        classification: classification.into(),
        recommended_action: RecommendedAction::from_class(classification),
    }
}

pub(super) fn ticket(
    path: &str,
    work_id: Option<&str>,
    prior_attempt_count: usize,
    last_failure_class: Option<&str>,
    has_active_mr: bool,
    human_required: bool,
) -> AvailableTicket {
    // For tests: genuine_agent_failure_count equals prior_attempt_count
    // unless the caller sets it explicitly. Tests that need different
    // values construct AvailableTicket directly.
    let genuine_agent_failure_count =
        if last_failure_class.is_some_and(crate::controller::is_genuine_agent_failure) {
            prior_attempt_count
        } else {
            0
        };
    AvailableTicket {
        ticket_path: path.into(),
        work_id: work_id.map(str::to_string),
        normalized_work_identity: crate::work_claim::normalize_work_identity(
            work_id.unwrap_or(path),
        ),
        source: crate::models::CandidateSource::LegacyTicket,
        execution_policy: crate::models::CandidateExecutionPolicy {
            intake_mode: "legacy".into(),
            explicit_autonomy_required: false,
            autonomous_metadata_present: false,
            dispatchable_now: true,
            exclusion_reason_code: None,
            exclusion_reason: None,
        },
        title: None,
        recommended_backend: None,
        recommended_model: None,
        priority: crate::models::TicketPriority::Unspecified,
        prior_attempt_count,
        genuine_agent_failure_count,
        last_failure_class: last_failure_class.map(str::to_string),
        has_active_mr,
        human_required,
        human_required_reason_code: None,
        has_active_claim: false,
    }
}
