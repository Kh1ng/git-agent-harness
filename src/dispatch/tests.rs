use super::{
    capacity_deferred_error, ensure_terminal_failure_attribution, is_policy_approval_gate,
    should_notify_dispatch_failure,
};
use crate::routing::{RouteError, SkippedBackend};

fn no_eligible(reason: &str) -> anyhow::Error {
    RouteError::NoEligibleBackend {
        preferred_backend: "claude".into(),
        preferred_model: Some("sonnet".into()),
        skipped: vec![SkippedBackend {
            backend: "claude".into(),
            model: Some("sonnet".into()),
            reason: reason.into(),
            unavailable_until: None,
        }],
        earliest_reset: None,
    }
    .into()
}

#[test]
fn capacity_deferral_is_detected_through_anyhow_context_and_not_notified() {
    let error = no_eligible("max_concurrent_reached").context("routing review");
    assert!(capacity_deferred_error(&error));
    assert!(!should_notify_dispatch_failure(&error));
}

#[test]
fn genuine_no_eligible_route_still_notifies_as_a_failure() {
    let error = no_eligible("quota_exhausted");
    assert!(!capacity_deferred_error(&error));
    assert!(should_notify_dispatch_failure(&error));
}

#[test]
fn unattributed_terminal_error_gets_concrete_harness_fallback() {
    let mut class = None;
    let mut stage = None;

    ensure_terminal_failure_attribution(&mut class, &mut stage);

    assert_eq!(class.as_deref(), Some("harness_error"));
    assert_eq!(stage.as_deref(), Some("dispatch"));
}

#[test]
fn only_typed_paid_route_human_blocks_are_transition_deduplicated() {
    let profile = crate::ledger::test_util::profile();
    let mut entry =
        crate::ledger::LedgerEntry::new("test", &profile, "auto", "fix", "#639", None, None);
    entry.work_id = Some("#639".into());
    entry.human_required = true;
    entry.human_required_reason_code = Some("policy_approval".into());
    entry.failure_class = Some("human_blocked".into());
    assert!(is_policy_approval_gate(&entry));

    entry.failure_class = Some("backend_error".into());
    assert!(!is_policy_approval_gate(&entry));
    entry.failure_class = Some("human_blocked".into());
    entry.human_required_reason_code = Some("review_evidence_gate".into());
    assert!(!is_policy_approval_gate(&entry));
}

#[test]
fn terminal_error_fallback_preserves_existing_specific_attribution() {
    let mut class = Some("validation_failure".to_string());
    let mut stage = Some("post_validation".to_string());

    ensure_terminal_failure_attribution(&mut class, &mut stage);

    assert_eq!(class.as_deref(), Some("validation_failure"));
    assert_eq!(stage.as_deref(), Some("post_validation"));
}
