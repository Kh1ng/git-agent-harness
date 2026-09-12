use super::*;

fn reviewer_metrics(samples: u64, successes: u64) -> ReviewerOutcomeMetrics {
    ReviewerOutcomeMetrics {
        completed_reviews: samples,
        outcome_samples: samples,
        successful_outcomes: successes,
        ..ReviewerOutcomeMetrics::default()
    }
}

#[test]
fn reviewer_history_reorders_only_after_the_minimum_sample() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.review_candidates = Some(vec![
        candidate_config("codex", Some("gpt"), None),
        candidate_config("claude", Some("sonnet"), None),
    ]);
    let mut runtime = RoutingRuntimeState::default();
    runtime.reviewer_outcomes.insert(
        CandidateIdentity::new("codex", Some("gpt")),
        reviewer_metrics(5, 2),
    );
    runtime.reviewer_outcomes.insert(
        CandidateIdentity::new("claude", Some("sonnet")),
        reviewer_metrics(5, 5),
    );

    let decision = decide_with_runtime(
        &defaults(),
        &profile,
        RouteRequest {
            mode: "review",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
            last_failure_class: None,
            exact_route_required: false,
        },
        &runtime,
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "claude");
    assert!(decision
        .routing_reason
        .contains("outcome-aware reviewer history"));
    let diagnostics = decision.routing_diagnostics.unwrap();
    assert_eq!(diagnostics.configured_order, ["codex/gpt", "claude/sonnet"]);
    assert_eq!(diagnostics.final_order, ["claude/sonnet", "codex/gpt"]);
    assert_eq!(
        diagnostics.reviewer_history_status.as_deref(),
        Some("reordered")
    );
    assert_eq!(diagnostics.reviewer_history_selected_samples, Some(5));
}

#[test]
fn missing_reviewer_samples_and_configured_last_glm_preserve_order() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.review_candidates = Some(vec![
        candidate_config("codex", Some("gpt"), None),
        candidate_config("claude", Some("sonnet"), None),
        candidate_config("opencode", Some("glm-5"), None),
    ]);
    let mut runtime = RoutingRuntimeState::default();
    runtime.reviewer_outcomes.insert(
        CandidateIdentity::new("codex", Some("gpt")),
        reviewer_metrics(5, 1),
    );
    runtime.reviewer_outcomes.insert(
        CandidateIdentity::new("claude", Some("sonnet")),
        reviewer_metrics(4, 4),
    );
    runtime.reviewer_outcomes.insert(
        CandidateIdentity::new("opencode", Some("glm-5")),
        reviewer_metrics(50, 50),
    );

    let decision = decide_with_runtime(
        &defaults(),
        &profile,
        RouteRequest {
            mode: "review",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
            last_failure_class: None,
            exact_route_required: false,
        },
        &runtime,
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(
        decision.routing_diagnostics.as_ref().unwrap().final_order,
        ["codex/gpt", "claude/sonnet", "opencode/glm-5"]
    );
    assert_eq!(
        decision
            .routing_diagnostics
            .unwrap()
            .reviewer_history_status
            .as_deref(),
        Some("insufficient_samples")
    );
}
