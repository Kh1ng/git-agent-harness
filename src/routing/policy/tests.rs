use super::super::decision::{
    decide_with, decide_with_runtime, decide_with_task, decide_with_task_runtime, RouteEvaluation,
};
use super::super::test_support::{
    backend_available, candidate_config, defaults, easy_docs_rule, implementation_request, path,
    profile, record_unavailable,
};
use super::super::{
    CandidateIdentity, ConcurrencyGuard, RouteError, RouteRequest, RoutingRuntimeState,
    TaskRoutingContext,
};
use super::is_genuine_agent_failure;
use crate::availability::{Reason, Source};
use tempfile::TempDir;
use time::OffsetDateTime;

#[test]
fn task_rule_precedes_generic_candidates_for_matching_implementation() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.improve_candidates =
        Some(vec![candidate_config("codex", Some("strong"), None)]);
    profile.routing.task_routing_rules = vec![easy_docs_rule(vec![candidate_config(
        "agy",
        Some("cheap"),
        None,
    )])];

    let decision = decide_with_task(
        &defaults(),
        &profile,
        implementation_request(),
        Some(TaskRoutingContext {
            task_class: Some("Documentation"),
            difficulty: Some("EASY"),
            risk: Some("low"),
        }),
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "agy");
    assert_eq!(decision.effective_model.as_deref(), Some("cheap"));
    assert_eq!(decision.routing_reason, "task routing rule #1");
}

#[test]
fn task_rule_falls_through_when_its_first_candidate_is_unavailable() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.task_routing_rules = vec![easy_docs_rule(vec![
        candidate_config("agy", Some("cheap"), None),
        candidate_config("codex", Some("fallback"), None),
    ])];
    record_unavailable(
        &path(&tmp),
        "agy",
        Some("cheap"),
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let decision = decide_with_task(
        &defaults(),
        &profile,
        implementation_request(),
        Some(TaskRoutingContext {
            task_class: Some("documentation"),
            difficulty: Some("easy"),
            risk: Some("low"),
        }),
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert!(decision.fallback_used);
    assert!(decision.routing_reason.contains("quota_exhausted"));
}

#[test]
fn equal_priority_task_pool_selects_least_used_candidate() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    let mut first = candidate_config("opencode", Some("hy3"), None);
    first.priority = 100;
    first.included_in_quota = true;
    let mut second = candidate_config("codex", Some("spark"), None);
    second.priority = 100;
    second.included_in_quota = true;
    profile.routing.task_routing_rules = vec![easy_docs_rule(vec![first, second])];
    let mut runtime = RoutingRuntimeState::default();
    runtime
        .recent_runs
        .insert(CandidateIdentity::new("opencode", Some("hy3")), 4);
    runtime
        .recent_runs
        .insert(CandidateIdentity::new("codex", Some("spark")), 1);

    let decision = decide_with_task_runtime(
        &defaults(),
        &profile,
        implementation_request(),
        Some(TaskRoutingContext {
            task_class: Some("documentation"),
            difficulty: Some("easy"),
            risk: Some("low"),
        }),
        &runtime,
        RouteEvaluation {
            state_path: &path(&tmp),
            now: OffsetDateTime::now_utc(),
        },
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(decision.effective_model.as_deref(), Some("spark"));
    assert!(
        decision
            .routing_diagnostics
            .unwrap()
            .policy_reordered_candidates
    );
}

#[test]
fn capability_escalation_excludes_previously_attempted_candidate() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    let mut first = candidate_config("opencode", Some("hy3"), None);
    first.priority = 100;
    let mut second = candidate_config("codex", Some("spark"), None);
    second.priority = 90;
    profile.routing.task_routing_rules = vec![easy_docs_rule(vec![first, second])];
    let mut runtime = RoutingRuntimeState::default();
    runtime
        .attempted
        .insert(CandidateIdentity::new("opencode", Some("hy3")));
    let mut request = implementation_request();
    request.last_failure_class = Some("agent_no_progress");

    let decision = decide_with_task_runtime(
        &defaults(),
        &profile,
        request,
        Some(TaskRoutingContext {
            task_class: Some("documentation"),
            difficulty: Some("easy"),
            risk: Some("low"),
        }),
        &runtime,
        RouteEvaluation {
            state_path: &path(&tmp),
            now: OffsetDateTime::now_utc(),
        },
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(decision.effective_model.as_deref(), Some("spark"));
    assert_eq!(
        decision.routing_diagnostics.unwrap().candidates[0]
            .skip_reason
            .as_deref(),
        Some("already_attempted_after_capability_failure")
    );
}

#[test]
fn capability_escalation_remains_sticky_after_a_later_infrastructure_failure() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    let mut first = candidate_config("opencode", Some("hy3"), None);
    first.priority = 100;
    let mut second = candidate_config("codex", Some("spark"), None);
    second.priority = 90;
    profile.routing.task_routing_rules = vec![easy_docs_rule(vec![first, second])];
    let mut runtime = RoutingRuntimeState::default();
    runtime
        .attempted
        .insert(CandidateIdentity::new("opencode", Some("hy3")));
    let mut request = implementation_request();
    request.last_failure_class = Some("backend_error");

    let decision = decide_with_task_runtime(
        &defaults(),
        &profile,
        request,
        Some(TaskRoutingContext {
            task_class: Some("documentation"),
            difficulty: Some("easy"),
            risk: Some("low"),
        }),
        &runtime,
        RouteEvaluation {
            state_path: &path(&tmp),
            now: OffsetDateTime::now_utc(),
        },
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(decision.effective_model.as_deref(), Some("spark"));
    assert_eq!(
        decision.routing_diagnostics.unwrap().candidates[0]
            .skip_reason
            .as_deref(),
        Some("already_attempted_after_capability_failure")
    );
}

#[test]
fn paid_candidate_requires_exact_operator_approval() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    let mut paid = candidate_config("opencode", Some("openai/gpt-paid"), None);
    paid.priority = 10;
    paid.requires_approval = true;
    profile.routing.task_routing_rules = vec![easy_docs_rule(vec![paid])];
    let task = Some(TaskRoutingContext {
        task_class: Some("documentation"),
        difficulty: Some("easy"),
        risk: Some("low"),
    });

    let err = decide_with_task_runtime(
        &defaults(),
        &profile,
        implementation_request(),
        task,
        &RoutingRuntimeState::default(),
        RouteEvaluation {
            state_path: &path(&tmp),
            now: OffsetDateTime::now_utc(),
        },
        backend_available,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<RouteError>(),
        Some(RouteError::ApprovalRequired { backend, model, .. })
            if backend == "opencode" && model.as_deref() == Some("openai/gpt-paid")
    ));

    let mut approved = RoutingRuntimeState::default();
    approved
        .approved
        .insert(CandidateIdentity::new("opencode", Some("openai/gpt-paid")));
    let decision = decide_with_task_runtime(
        &defaults(),
        &profile,
        implementation_request(),
        task,
        &approved,
        RouteEvaluation {
            state_path: &path(&tmp),
            now: OffsetDateTime::now_utc(),
        },
        backend_available,
    )
    .unwrap();
    assert_eq!(decision.effective_model.as_deref(), Some("openai/gpt-paid"));
    assert_eq!(
        decision
            .routing_diagnostics
            .unwrap()
            .selected_cost_class
            .as_deref(),
        Some("paid")
    );
}

#[test]
fn load_balancing_does_not_reorder_review_candidates() {
    // recent_runs is populated from review/pm history too (routing_runtime_state
    // in src/dispatch.rs deliberately tracks all agent-execution modes for
    // attribution), but the balancing TIE-BREAK must stay scoped to
    // implementation dispatch. Review's configured order is a
    // deliberate escalation chain (see explicit_candidates' "Claude ->
    // GLM must not fall back to AGY again" invariant elsewhere in this
    // file) and must not be silently reshuffled by usage counts.
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    let mut first = candidate_config("claude", Some("sonnet"), None);
    first.priority = 100;
    let mut second = candidate_config("opencode", Some("glm"), None);
    second.priority = 100;
    profile.routing.review_candidates = Some(vec![first, second]);
    let mut runtime = RoutingRuntimeState::default();
    // The configured-first candidate is far more heavily used -- if
    // load-balancing applied here, it would be passed over.
    runtime
        .recent_runs
        .insert(CandidateIdentity::new("claude", Some("sonnet")), 10);
    runtime
        .recent_runs
        .insert(CandidateIdentity::new("opencode", Some("glm")), 0);

    let decision = decide_with_runtime(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "review",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &runtime,
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model.as_deref(), Some("sonnet"));
}

#[test]
fn explicit_cli_model_override_cannot_bypass_approval_gate() {
    // requested_backend defaults to "auto" on every real dispatch (see
    // main.rs's --backend default), so an operator running
    // `gah dispatch --model <paid-model>` without an explicit --backend
    // must not be able to silently reach an unapproved requires_approval
    // candidate just because the model string matches.
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    let mut free = candidate_config("opencode", Some("hy3-free"), None);
    free.priority = 100;
    let mut paid = candidate_config("opencode", Some("openai/gpt-paid"), None);
    paid.priority = 10;
    paid.requires_approval = true;
    profile.routing.task_routing_rules = vec![easy_docs_rule(vec![free, paid])];
    let task = Some(TaskRoutingContext {
        task_class: Some("documentation"),
        difficulty: Some("easy"),
        risk: Some("low"),
    });
    let mut request = implementation_request();
    request.requested_model = Some("openai/gpt-paid");

    // Baseline: without an override, routing naturally picks the free
    // candidate -- confirms the override target genuinely differs from
    // what would have been selected, so this test exercises the
    // override path at all.
    let unoverridden = decide_with_task_runtime(
        &defaults(),
        &profile,
        implementation_request(),
        task,
        &RoutingRuntimeState::default(),
        RouteEvaluation {
            state_path: &path(&tmp),
            now: OffsetDateTime::now_utc(),
        },
        backend_available,
    )
    .unwrap();
    assert_eq!(unoverridden.effective_model.as_deref(), Some("hy3-free"));

    let err = decide_with_task_runtime(
        &defaults(),
        &profile,
        request.clone(),
        task,
        &RoutingRuntimeState::default(),
        RouteEvaluation {
            state_path: &path(&tmp),
            now: OffsetDateTime::now_utc(),
        },
        backend_available,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<RouteError>(),
        Some(RouteError::ApprovalRequired { backend, model, .. })
            if backend == "opencode" && model.as_deref() == Some("openai/gpt-paid")
    ));

    // Once approved, the exact same override succeeds.
    let mut approved = RoutingRuntimeState::default();
    approved
        .approved
        .insert(CandidateIdentity::new("opencode", Some("openai/gpt-paid")));
    let decision = decide_with_task_runtime(
        &defaults(),
        &profile,
        request,
        task,
        &approved,
        RouteEvaluation {
            state_path: &path(&tmp),
            now: OffsetDateTime::now_utc(),
        },
        backend_available,
    )
    .unwrap();
    assert_eq!(decision.effective_model.as_deref(), Some("openai/gpt-paid"));
}

#[test]
fn explicit_internal_review_route_cannot_bypass_paid_approval_gate() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    let mut paid = candidate_config(
        "opencode",
        Some("nous-portal/z-ai/glm-5.2"),
        Some("nous-portal-api"),
    );
    paid.requires_approval = true;
    profile.routing.review_candidates = Some(vec![paid.clone()]);
    profile.routing.escalatory_reviewers = vec![paid];
    let request = RouteRequest {
        last_failure_class: None,
        mode: "review",
        requested_backend: "opencode",
        requested_model: Some("nous-portal/z-ai/glm-5.2"),
        recommended_backend: None,
        recommended_model: None,
        session_id: None,
        usage_summary: None,
    };

    let err = decide_with_runtime(
        &defaults(),
        &profile,
        request.clone(),
        &RoutingRuntimeState::default(),
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<RouteError>(),
        Some(RouteError::ApprovalRequired { backend, model, .. })
            if backend == "opencode"
                && model.as_deref() == Some("nous-portal/z-ai/glm-5.2")
    ));

    let mut approved = RoutingRuntimeState::default();
    approved.approved.insert(CandidateIdentity::new(
        "opencode",
        Some("nous-portal/z-ai/glm-5.2"),
    ));
    let decision = decide_with_runtime(
        &defaults(),
        &profile,
        request,
        &approved,
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();
    assert_eq!(decision.effective_backend, "opencode");
    assert_eq!(
        decision.effective_model.as_deref(),
        Some("nous-portal/z-ai/glm-5.2")
    );
}

#[test]
fn temporary_subscribed_capacity_precedes_paid_route_approval() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    let mut profile = profile();
    let mut subscribed = candidate_config("claude", Some("sonnet"), Some("claude-main"));
    subscribed.included_in_quota = true;
    let mut paid = candidate_config(
        "opencode",
        Some("nous-portal/z-ai/glm-5.2"),
        Some("nous-portal-api"),
    );
    paid.requires_approval = true;
    profile.routing.review_candidates = Some(vec![subscribed, paid]);
    profile
        .max_concurrent_per_model
        .insert("claude/sonnet".into(), 1);
    let request = RouteRequest {
        last_failure_class: None,
        mode: "review",
        requested_backend: "auto",
        requested_model: None,
        recommended_backend: None,
        recommended_model: None,
        session_id: None,
        usage_summary: None,
    };

    let slot = ConcurrencyGuard::acquire("claude", Some("sonnet"));
    let err = decide_with_runtime(
        &defaults(),
        &profile,
        request.clone(),
        &RoutingRuntimeState::default(),
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<RouteError>(),
        Some(RouteError::NoEligibleBackend { skipped, .. })
            if skipped.iter().any(|candidate| candidate.reason == "max_concurrent_reached")
                && skipped
                    .iter()
                    .any(|candidate| candidate.reason == "operator_approval_required")
    ));

    drop(slot);
    let decision = decide_with_runtime(
        &defaults(),
        &profile,
        request.clone(),
        &RoutingRuntimeState::default(),
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model.as_deref(), Some("sonnet"));

    record_unavailable(
        &path(&tmp),
        "claude",
        Some("sonnet"),
        Reason::QuotaExhausted,
        Source::BackendError,
        None,
        None,
        now,
    )
    .unwrap();
    let err = decide_with_runtime(
        &defaults(),
        &profile,
        request,
        &RoutingRuntimeState::default(),
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<RouteError>(),
        Some(RouteError::ApprovalRequired { backend, model, .. })
            if backend == "opencode"
                && model.as_deref() == Some("nous-portal/z-ai/glm-5.2")
    ));
}

#[test]
fn missing_or_unmatched_task_metadata_preserves_generic_routing() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.improve_candidates =
        Some(vec![candidate_config("codex", Some("strong"), None)]);
    profile.routing.task_routing_rules = vec![easy_docs_rule(vec![candidate_config(
        "agy",
        Some("cheap"),
        None,
    )])];

    let missing = decide_with_task(
        &defaults(),
        &profile,
        implementation_request(),
        Some(TaskRoutingContext::default()),
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();
    assert_eq!(missing.effective_backend, "codex");

    let review = decide_with_task(
        &defaults(),
        &profile,
        RouteRequest {
            mode: "review",
            ..implementation_request()
        },
        Some(TaskRoutingContext {
            task_class: Some("documentation"),
            difficulty: Some("easy"),
            risk: Some("low"),
        }),
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();
    assert_ne!(review.routing_reason, "task routing rule #1");
}

#[test]
fn profile_routing_beats_global_policy() {
    let tmp = TempDir::new().unwrap();
    let decision = decide_with(
        &defaults(),
        &profile(),
        RouteRequest {
            last_failure_class: None,
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: Some("openhands"),
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();
    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.routing_reason, "profile routing policy");
}

#[test]
fn profile_routing_can_select_agy() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.default_backend = Some("agy".into());
    let decision = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "improve",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "agy");
    assert_eq!(decision.routing_reason, "profile routing policy");
}

#[test]
fn default_candidate_list_is_inherited_when_profile_only_overrides_other_fields() {
    let tmp = TempDir::new().unwrap();
    let mut defaults = defaults();
    defaults.routing.pm_candidates = Some(vec![
        candidate_config("codex", Some("gpt-5"), None),
        candidate_config("claude", Some("sonnet"), None),
    ]);
    let mut profile = profile();
    profile.routing.improve_backend = Some("agy".into());

    let decision = decide_with(
        &defaults,
        &profile,
        RouteRequest {
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
            last_failure_class: None,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5"));
    assert_eq!(decision.routing_reason, "global routing policy");
}

#[test]
fn explicit_review_fallback_preserves_the_remaining_review_order() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    let mut profile = profile();
    profile.routing.review_candidates = Some(vec![
        crate::config::CandidateConfig {
            backend: "agy".into(),
            model: Some("sonnet".into()),
            ..Default::default()
        },
        crate::config::CandidateConfig {
            backend: "agy-second".into(),
            model: Some("sonnet".into()),
            ..Default::default()
        },
        crate::config::CandidateConfig {
            backend: "claude".into(),
            model: Some("sonnet-5".into()),
            ..Default::default()
        },
        crate::config::CandidateConfig {
            backend: "opencode".into(),
            model: Some("nous-portal/z-ai/glm-5.2".into()),
            ..Default::default()
        },
    ]);
    record_unavailable(
        &path(&tmp),
        "agy",
        Some("sonnet"),
        Reason::BackendOutage,
        Source::BackendError,
        None,
        None,
        now,
    )
    .unwrap();
    let via_agy = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "review",
            requested_backend: "agy",
            requested_model: Some("sonnet"),
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(via_agy.effective_backend, "agy-second");

    record_unavailable(
        &path(&tmp),
        "claude",
        Some("sonnet-5"),
        Reason::BackendOutage,
        Source::BackendError,
        None,
        None,
        now,
    )
    .unwrap();
    let via_claude = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "review",
            requested_backend: "claude",
            requested_model: Some("sonnet-5"),
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(via_claude.effective_backend, "opencode");
    assert_eq!(
        via_claude.effective_model.as_deref(),
        Some("nous-portal/z-ai/glm-5.2")
    );
}

#[test]
fn explicit_review_fallback_preserves_order_when_request_omits_model() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    let mut profile = profile();
    profile.routing.review_candidates = Some(vec![
        crate::config::CandidateConfig {
            backend: "agy".into(),
            model: Some("sonnet".into()),
            ..Default::default()
        },
        crate::config::CandidateConfig {
            backend: "claude".into(),
            model: Some("sonnet-5".into()),
            ..Default::default()
        },
    ]);
    record_unavailable(
        &path(&tmp),
        "agy",
        None,
        Reason::BackendOutage,
        Source::BackendError,
        None,
        None,
        now,
    )
    .unwrap();
    // A manual/escalated review request that names the backend but not a
    // model must still locate its position in the configured pool and
    // preserve the remainder, not fall through to weak_review_backend.
    let via_agy = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "review",
            requested_backend: "agy",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(via_agy.effective_backend, "claude");
    assert_eq!(via_agy.effective_model.as_deref(), Some("sonnet-5"));
}

#[test]
fn is_genuine_agent_failure_classifies_correctly() {
    // TICKET-089 AC7/8
    assert!(is_genuine_agent_failure(Some("agent_failure")));
    assert!(is_genuine_agent_failure(Some("agent_no_progress")));
    assert!(is_genuine_agent_failure(Some("validation_failure")));
    assert!(!is_genuine_agent_failure(Some("harness_error")));
    assert!(!is_genuine_agent_failure(Some("environment_error")));
    assert!(!is_genuine_agent_failure(Some("backend_error")));
    assert!(!is_genuine_agent_failure(Some("human_blocked")));
    assert!(!is_genuine_agent_failure(Some("unknown")));
    assert!(!is_genuine_agent_failure(None));
}

#[test]
fn genuine_agent_failure_escalates_to_stronger_model() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        crate::config::CandidateConfig {
            backend: "openhands".into(),
            model: Some("deepseek-flash".into()),
            quota_pool: None,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        },
        crate::config::CandidateConfig {
            backend: "codex".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: None,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        },
    ]);

    let decision = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: Some("validation_failure"),
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
    assert!(decision
        .routing_reason
        .contains("escalated to stronger model"));
}

#[test]
fn non_agent_failure_does_not_escalate() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        crate::config::CandidateConfig {
            backend: "openhands".into(),
            model: Some("deepseek-flash".into()),
            quota_pool: None,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        },
        crate::config::CandidateConfig {
            backend: "codex".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: None,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        },
    ]);

    for failure in [None, Some("backend_error"), Some("harness_error")] {
        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: failure,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "openhands");
        assert_eq!(decision.effective_model.as_deref(), Some("deepseek-flash"));
        assert!(!decision.routing_reason.contains("escalated"));
    }
}

#[test]
fn cost_aware_ordering_prefers_underpace_included_quota() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        crate::config::CandidateConfig {
            backend: "openhands".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: None,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: Some(0.25),
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        },
        crate::config::CandidateConfig {
            backend: "codex".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: Some("codex-main".into()),
            priority: 0,
            included_in_quota: true,
            marginal_cost_usd: Some(0.0),
            quota_usage_percent: Some(20.0),
            quota_days_remaining: Some(5.0),
            requires_approval: false,
        },
    ]);

    let decision = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
    assert!(!decision.fallback_used);
    assert!(decision.routing_reason.contains("cost-aware reorder"));
    assert!(decision.routing_reason.contains("openhands/gpt-5.4"));
    let diagnostics = decision.routing_diagnostics.as_ref().unwrap();
    assert!(diagnostics.policy_reordered_candidates);
    assert_eq!(
        diagnostics.selected_quota_pool.as_deref(),
        Some("codex-main")
    );
    assert_eq!(
        diagnostics.selected_pace_band.as_deref(),
        Some("aggressive_burn")
    );
    assert_eq!(
        diagnostics.selected_cost_class.as_deref(),
        Some("included_quota")
    );
    assert_eq!(diagnostics.selected_over.len(), 1);
    assert!(diagnostics
        .human_summary
        .as_deref()
        .unwrap()
        .contains("policy reordered defaults"));
}

#[test]
fn cost_aware_ordering_conserves_scarce_included_quota() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        crate::config::CandidateConfig {
            backend: "codex".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: Some("codex-main".into()),
            priority: 0,
            included_in_quota: true,
            marginal_cost_usd: Some(0.0),
            quota_usage_percent: Some(85.0),
            quota_days_remaining: Some(5.0),
            requires_approval: false,
        },
        crate::config::CandidateConfig {
            backend: "openhands".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: None,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: Some(0.25),
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        },
    ]);

    let decision = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "openhands");
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
    assert!(!decision.fallback_used);
    assert!(decision.routing_reason.contains("codex/gpt-5.4"));
}

#[test]
fn cost_aware_ordering_respects_explicit_priority_override() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        crate::config::CandidateConfig {
            backend: "codex".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: Some("codex-main".into()),
            priority: 10,
            included_in_quota: true,
            marginal_cost_usd: Some(0.0),
            quota_usage_percent: Some(85.0),
            quota_days_remaining: Some(5.0),
            requires_approval: false,
        },
        crate::config::CandidateConfig {
            backend: "openhands".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: None,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: Some(0.25),
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        },
    ]);

    let decision = decide_with(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: None,
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
    assert!(!decision.routing_reason.contains("cost-aware reorder"));
}
