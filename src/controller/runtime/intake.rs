use crate::controller::NextAction;
use crate::status::StatusSnapshot;
use std::collections::HashSet;

pub(super) fn action_creates_managed_mr(action: &NextAction) -> bool {
    matches!(
        action,
        NextAction::DispatchTicket { .. } | NextAction::Retry { .. } | NextAction::Escalate { .. }
    )
}

pub(super) fn action_intake_key(action: &NextAction) -> Option<String> {
    match action {
        NextAction::DispatchTicket {
            work_id,
            ticket_path,
            ..
        } => Some(crate::work_claim::normalize_work_identity(
            work_id.as_deref().unwrap_or(ticket_path),
        )),
        NextAction::Retry { work_id, .. } | NextAction::Escalate { work_id, .. } => {
            Some(crate::work_claim::normalize_work_identity(work_id))
        }
        _ => None,
    }
}

pub(super) fn apply_parallel_projection(
    snapshot: &mut StatusSnapshot,
    active_intake_keys: &HashSet<String>,
    limit: u32,
) {
    let durable_claims = snapshot
        .active_claims
        .iter()
        .map(|claim| claim.work_id.as_str())
        .collect::<HashSet<_>>();
    let unobserved_local = active_intake_keys
        .iter()
        .filter(|key| !durable_claims.contains(key.as_str()))
        .count() as u32;
    snapshot.profile.max_open_managed_mrs = limit;
    snapshot.inflight_implementation_count = snapshot
        .inflight_implementation_count
        .saturating_add(unobserved_local);
    snapshot.implementation_intake_paused = snapshot
        .open_managed_mr_count
        .saturating_add(snapshot.inflight_implementation_count)
        >= limit;
}

pub(super) fn retain_unclaimed_work(
    snapshot: &mut StatusSnapshot,
    claimed_work_ids: &[String],
    executed_work_ids: &HashSet<String>,
) {
    let eligible = |work_id: Option<&str>| {
        work_id
            .map(|id| {
                let normalized = crate::work_claim::normalize_work_identity(id);
                !claimed_work_ids
                    .iter()
                    .any(|claimed| claimed == &normalized)
                    && !executed_work_ids.contains(&normalized)
            })
            .unwrap_or(true)
    };
    snapshot
        .available_tickets
        .retain(|ticket| eligible(ticket.work_id.as_deref()));
    snapshot
        .issue_intake_rejections
        .retain(|issue| eligible(issue.work_id.as_deref()));
    snapshot
        .merge_requests
        .retain(|mr| eligible(mr.work_id.as_deref()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_actions_that_can_publish_a_new_mr_consume_intake_slots() {
        let ticket = NextAction::DispatchTicket {
            ticket_path: "ticket.md".into(),
            work_id: Some("#1".into()),
            recommended_backend: None,
            recommended_model: None,
            reason: "test".into(),
        };
        let review = NextAction::ReviewMr {
            work_id: Some("#1".into()),
            branch: "gah/1".into(),
            mr_url: None,
            reason: "test".into(),
        };

        assert!(action_creates_managed_mr(&ticket));
        assert_eq!(action_intake_key(&ticket).as_deref(), Some("#1"));
        assert!(!action_creates_managed_mr(&review));
        assert_eq!(action_intake_key(&review), None);
    }
}
