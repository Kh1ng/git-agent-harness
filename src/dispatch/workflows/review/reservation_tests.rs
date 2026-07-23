use super::{
    mark_review_budget_exhausted, mark_review_shutdown_cancelled, reserve_review_route,
    review_attempt_environment, review_failure_output, review_outcome_allows_reroute,
    review_terminal_failure_summary,
};
use crate::config::tests::test_profile_for_notifications;
use crate::ledger::{LedgerEntry, LedgerUsage};
use crate::routing::{current_concurrent, RouteDecision};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Barrier};

fn attempt_record(
    attempt_number: u32,
    backend: &str,
    model: Option<&str>,
) -> crate::ledger::AttemptRecord {
    crate::ledger::AttemptRecord {
        attempt_number,
        backend: backend.into(),
        effective_model: model.map(str::to_string),
        exit_code: Some(1),
        validation_result: Some("not_run".into()),
        failure_class: Some(crate::ledger::FailureClass::BackendError.as_str().into()),
        failure_stage: Some(crate::ledger::FailureStage::Review.as_str().into()),
        duration_seconds: Some(1.0),
        diff_path: None,
        cli_version: None,
        usage: LedgerUsage::default(),
    }
}

#[test]
fn healthy_hard_ceiling_does_not_retry_or_escalate() {
    assert!(!review_outcome_allows_reroute(
        &crate::runner::ReviewProcessOutcome::HardTimeout
    ));
    assert!(review_outcome_allows_reroute(
        &crate::runner::ReviewProcessOutcome::IdleTimeout
    ));
    assert!(review_outcome_allows_reroute(
        &crate::runner::ReviewProcessOutcome::NonZeroExit(1)
    ));
}

#[test]
fn idle_timeout_failure_output_preserves_partial_output_and_adds_stall_signal() {
    let output = review_failure_output(
        &crate::runner::ReviewProcessOutcome::IdleTimeout,
        "started review",
        "",
        600,
    );
    assert!(output.starts_with("started review\n"));
    assert!(output.contains(
        "GAH: killed after 600s with no new worktree progress (stalled before changes, not just slow)."
    ));
}

#[test]
fn review_terminal_summary_deduplicates_attempted_routes_in_order() {
    let profile = test_profile_for_notifications();
    let mut ledger = LedgerEntry::new("gah", &profile, "vibe", "review", "test", None, None);
    ledger.attempts_started = Some(3);
    ledger
        .attempts
        .push(attempt_record(1, "vibe", Some("first")));
    ledger
        .attempts
        .push(attempt_record(2, "vibe", Some("first")));
    ledger
        .attempts
        .push(attempt_record(3, "claude", Some("sonnet")));

    let summary = review_terminal_failure_summary(&ledger, "terminal failure");
    assert_eq!(
        summary,
        "review failed after 3 attempt(s): terminal failure; attempted routes: vibe/first -> claude/sonnet"
    );
}

#[test]
fn review_attempt_environment_isolates_only_agy_second() {
    let mut profile = test_profile_for_notifications();
    profile.agy_second_home = Some("/tmp/agy-account-2".into());
    let base = vec![("HOME".to_string(), "/home/operator".to_string())];

    let primary_identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "agy",
        None::<String>,
        None::<String>,
    );
    let primary = review_attempt_environment(&profile, &primary_identity, &base);
    assert_eq!(primary, base);

    let secondary_identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "agy-second",
        None::<String>,
        None::<String>,
    );
    let secondary = review_attempt_environment(&profile, &secondary_identity, &base);
    assert_eq!(
        secondary,
        vec![("HOME".to_string(), "/tmp/agy-account-2".to_string())]
    );
}

#[test]
fn review_attempt_environment_uses_declared_instance_state_root() {
    let profile = test_profile_for_notifications();
    let base = vec![
        ("HOME".to_string(), "/home/operator".to_string()),
        ("PATH".to_string(), "/bin".to_string()),
    ];
    let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "opencode",
        Some("openai/gpt-5"),
        None::<String>,
    );
    identity.backend_instance = "opencode-api".into();
    identity.set_state_root(Some("/var/lib/gah/opencode-api".into()));

    let environment = review_attempt_environment(&profile, &identity, &base);

    assert_eq!(
        environment,
        vec![
            ("PATH".to_string(), "/bin".to_string()),
            ("HOME".to_string(), "/var/lib/gah/opencode-api".to_string()),
        ]
    );
}

#[test]
fn shutdown_clears_provisional_fallback_confidence_and_human_hold() {
    let profile = test_profile_for_notifications();
    let mut ledger = LedgerEntry::new("gah", &profile, "claude", "review", "test", None, None);
    ledger.confidence_impact = Some("low".into());
    ledger.human_required = true;

    mark_review_shutdown_cancelled(&mut ledger, 15);

    assert_eq!(
        ledger.validation_result.as_deref(),
        Some("cancelled_shutdown")
    );
    assert_eq!(ledger.failure_class.as_deref(), Some("harness_error"));
    assert_eq!(ledger.failure_stage.as_deref(), Some("review"));
    assert_eq!(ledger.backend_exit_code, Some(-15));
    assert_eq!(ledger.confidence_impact, None);
    assert!(!ledger.human_required);
}

#[test]
fn route_attribution_does_not_clear_a_review_budget_hold() {
    let profile = test_profile_for_notifications();
    let mut ledger = LedgerEntry::new("gah", &profile, "vibe", "review", "test", None, None);
    let route = RouteDecision::from_identity(
        crate::execution_identity::ExecutionIdentity::legacy_route(
            "vibe",
            Some("reviewer"),
            "vibe",
            Some("reviewer"),
            None::<String>,
        ),
        "test".into(),
        false,
        None,
        false,
        None,
    );

    mark_review_budget_exhausted(&mut ledger, &route, "budget exhausted");

    assert!(ledger.human_required);
    assert_eq!(
        ledger.human_required_reason_code.as_deref(),
        Some("retry_budget_exhausted")
    );
    assert_eq!(ledger.failure_class.as_deref(), Some("human_blocked"));
    assert_eq!(ledger.failure_stage.as_deref(), Some("review"));
}

#[test]
fn three_reviews_never_overlap_on_a_backend_model_capped_at_one() {
    let backend = format!("review-reservation-test-{}", std::process::id());
    let model = "sonnet-test";
    let mut profile = test_profile_for_notifications();
    profile
        .max_concurrent_per_model
        .insert(format!("{backend}/{model}"), 1);
    let route = RouteDecision::from_identity(
        crate::execution_identity::ExecutionIdentity::legacy_route(
            backend.clone(),
            Some(model),
            backend.clone(),
            Some(model),
            None::<String>,
        ),
        "test".into(),
        false,
        None,
        false,
        None,
    );

    let profile = Arc::new(profile);
    let route = Arc::new(route);
    let start = Arc::new(Barrier::new(3));
    let max_seen = Arc::new(AtomicU32::new(0));
    let workers: Vec<_> = (0..3)
        .map(|_| {
            let profile = Arc::clone(&profile);
            let route = Arc::clone(&route);
            let start = Arc::clone(&start);
            let max_seen = Arc::clone(&max_seen);
            std::thread::spawn(move || {
                start.wait();
                let _slot = reserve_review_route(&profile, &route, None).unwrap();
                max_seen.fetch_max(
                    current_concurrent(&route.effective_backend, route.effective_model.as_deref()),
                    Ordering::SeqCst,
                );
                std::thread::sleep(std::time::Duration::from_millis(25));
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }

    assert_eq!(max_seen.load(Ordering::SeqCst), 1);
    assert_eq!(current_concurrent(&backend, Some(model)), 0);
}
