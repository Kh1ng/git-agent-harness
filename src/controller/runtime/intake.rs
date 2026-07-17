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
        } => Some(work_id.clone().unwrap_or_else(|| ticket_path.clone())),
        NextAction::Retry { work_id, .. } | NextAction::Escalate { work_id, .. } => {
            Some(work_id.clone())
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
