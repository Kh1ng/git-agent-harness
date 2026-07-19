use super::{route_state_fingerprint, NextAction};
use anyhow::Result;

pub(super) fn is_validation_gate_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.is::<crate::dispatch::ValidationGateError>())
}

pub(super) fn suppress_recent_capacity_deferrals(
    cfg: &crate::config::GahConfig,
    snapshot: &mut crate::status::StatusSnapshot,
    events: &[crate::events::ControllerEvent],
    entries: &[crate::ledger::LedgerEntry],
    profile_name: &str,
    repo_id: &str,
) -> std::collections::HashSet<String> {
    let now = time::OffsetDateTime::now_utc();
    let route_state = route_state_fingerprint(cfg, profile_name, now).ok();
    let deferred = super::recently_capacity_deferred_work_ids(
        events,
        entries,
        profile_name,
        repo_id,
        now,
        route_state.as_deref(),
    );
    super::retain_snapshot_candidates(snapshot, &deferred, &std::collections::HashSet::new());
    deferred
}

/// Persist a stuck-loop transition only when this work item has no active
/// durable human gate. The profile lock serializes controller writers, while
/// the fresh ledger read also protects parallel refill slots whose snapshot
/// predates a sibling's gate append.
pub(super) fn append_stuck_loop_gate_if_transition(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    work_id: &str,
    reason: &str,
    review_generation: Option<&str>,
) -> Result<bool> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let mut gate =
        crate::ledger::LedgerEntry::new(profile_name, profile, "auto", "fix", work_id, None, None);
    gate.work_id = Some(work_id.to_string());
    gate.human_required = true;
    gate.dispatch_reason = Some("stuck_loop_gate".to_string());
    gate.human_required_reason_code = Some("stuck_loop_gate".to_string());
    gate.error_summary = Some(reason.to_string());
    if let Some(review_generation) = review_generation {
        gate.review_contract_version = Some(crate::ledger::REVIEW_CONTRACT_VERSION);
        gate.review_generation = Some(review_generation.to_string());
    }
    crate::ledger::append_human_gate_if_transition(cfg, &gate)
}

pub(super) fn action_review_generation(
    snapshot: &crate::status::StatusSnapshot,
    action: &NextAction,
) -> Option<String> {
    let branch = match action {
        NextAction::ReviewMr { branch, .. }
        | NextAction::MarkReadyForReview { branch, .. }
        | NextAction::FixMr { branch, .. }
        | NextAction::MergeMr { branch, .. } => branch,
        _ => return None,
    };
    snapshot
        .merge_requests
        .iter()
        .find(|mr| mr.branch == *branch)
        .and_then(|mr| mr.review_generation.clone())
}
