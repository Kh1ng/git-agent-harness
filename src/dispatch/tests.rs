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

/// Issue #119: a backend that emits the documented `gah.behavior_summary`
/// event must have its per-attempt behavior metrics captured into the attempt
/// usage, with provenance `structured_event_derived`. Absent the event, the
/// metrics stay unknown (`None`) rather than a fabricated zero.
#[test]
fn attempt_usage_captures_documented_behavior_summary_event() {
    use crate::usage_attribution::UsageAttribution;

    let dir = std::env::temp_dir().join(format!("gah-behavior-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log_path = dir.join("attempt.log");
    std::fs::write(
        &log_path,
        "agent starting\n\
         {\"type\":\"gah.behavior_summary\",\"tool_calls\":4,\"shell_calls\":1,\"file_edits\":2,\"test_runs\":3}\n\
         agent finished\n",
    )
    .unwrap();

    let usage = crate::dispatch::attempts::attempt_usage(
        log_path.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("codex"), None),
        None,
        None,
    );

    let metrics = usage.behavior_metrics.expect("behavior metrics captured");
    let tc = metrics.tool_calls.expect("tool_calls known");
    assert_eq!(tc.count, Some(4));
    assert_eq!(
        tc.quality,
        crate::ledger::BehaviorMetricQuality::StructuredEventDerived
    );
    assert_eq!(metrics.shell_calls.as_ref().unwrap().count, Some(1));
    assert_eq!(metrics.file_edits.as_ref().unwrap().count, Some(2));
    assert_eq!(metrics.test_runs.as_ref().unwrap().count, Some(3));

    std::fs::remove_dir_all(&dir).ok();
}

/// Issue #119: a backend that never emits the behavior summary event leaves
/// behavior_metrics unknown, never an empty/zero record.
#[test]
fn attempt_usage_leaves_behavior_unknown_without_event() {
    use crate::usage_attribution::UsageAttribution;

    let dir = std::env::temp_dir().join(format!("gah-behavior-test-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log_path = dir.join("attempt.log");
    std::fs::write(&log_path, "agent did some work\nagent finished\n").unwrap();

    let usage = crate::dispatch::attempts::attempt_usage(
        log_path.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("codex"), None),
        None,
        None,
    );

    assert!(
        usage.behavior_metrics.is_none(),
        "no event => behavior_metrics stays unknown (None), never zero"
    );

    std::fs::remove_dir_all(&dir).ok();
}
