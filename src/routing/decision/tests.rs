use super::super::test_support::{
    backend_available, candidate_config, defaults, path, profile, record_available,
    record_unavailable,
};
use super::{decide_with, decide_with_runtime, RouteError, RouteRequest};
use super::{CandidateIdentity, RoutingRuntimeState};
use crate::availability::{Reason, Source};
use tempfile::TempDir;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[test]
fn codex_fallback_model_extracted_from_profile_codex_args() {
    let tmp = TempDir::new().unwrap();
    let defaults = defaults();
    let mut profile = profile();
    profile.codex_args = vec!["-m".to_string(), "gpt-5.4-mini".to_string()];

    let decision = decide_with(
        &defaults,
        &profile,
        RouteRequest {
            mode: "improve",
            requested_backend: "codex",
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
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4-mini"));
}

#[test]
fn codex_stale_args_do_not_override_resolved_model() {
    let tmp = TempDir::new().unwrap();
    let defaults = defaults();
    let mut profile = profile();
    profile.codex_args = vec!["-m".to_string(), "gpt-5.4-mini".to_string()];

    let decision = decide_with(
        &defaults,
        &profile,
        RouteRequest {
            mode: "improve",
            requested_backend: "codex",
            requested_model: Some("gpt-5.4"),
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
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
}

#[test]
fn auto_backend_honors_cli_model_override_in_effective_identity() {
    let tmp = TempDir::new().unwrap();
    let defaults = defaults();
    let profile = profile();

    let decision = decide_with(
        &defaults,
        &profile,
        RouteRequest {
            mode: "improve",
            requested_backend: "auto",
            requested_model: Some("custom/test-model"),
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

    assert_eq!(
        decision.effective_model.as_deref(),
        Some("custom/test-model")
    );
    let diagnostics = decision.routing_diagnostics.unwrap();
    assert_eq!(
        diagnostics.selected_model.as_deref(),
        Some("custom/test-model")
    );
    assert!(diagnostics
        .human_summary
        .unwrap()
        .contains("explicit CLI model override"));
}

#[test]
fn profile_scalar_override_preserves_inherited_default_model() {
    let tmp = TempDir::new().unwrap();
    let mut defaults = defaults();
    defaults.routing.improve_model = Some("gpt-5.4".into());
    let mut profile = profile();
    profile.routing.improve_backend = Some("agy".into());

    let decision = decide_with(
        &defaults,
        &profile,
        RouteRequest {
            mode: "improve",
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

    assert_eq!(decision.effective_backend, "agy");
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(decision.routing_reason, "profile routing policy");
}

#[test]
fn preferred_backend_unavailable_falls_back() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &path(&tmp),
        "claude",
        None,
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::hours(1)),
        None,
        now,
    )
    .unwrap();

    let decision = decide_with(
        &defaults(),
        &profile(),
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
        now,
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert!(decision.fallback_used);
    assert!(decision.routing_reason.contains("quota_exhausted"));
}

#[test]
fn preferred_backend_available_keeps_normal_selection() {
    let tmp = TempDir::new().unwrap();
    let decision = decide_with(
        &defaults(),
        &profile(),
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

    assert_eq!(decision.effective_backend, "claude");
    assert!(!decision.fallback_used);
}

#[test]
fn expired_temporary_record_restores_eligibility() {
    let tmp = TempDir::new().unwrap();
    let observed = OffsetDateTime::now_utc() - time::Duration::hours(2);
    record_unavailable(
        &path(&tmp),
        "claude",
        None,
        Reason::RateLimited,
        Source::BackendError,
        Some(observed + time::Duration::minutes(30)),
        None,
        observed,
    )
    .unwrap();

    let decision = decide_with(
        &defaults(),
        &profile(),
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

    assert_eq!(decision.effective_backend, "claude");
}

#[test]
fn backend_wide_block_blocks_all_models() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &path(&tmp),
        "codex",
        None,
        Reason::ManualDisable,
        Source::Manual,
        None,
        None,
        now,
    )
    .unwrap();

    let decision = decide_with(
        &defaults(),
        &profile(),
        RouteRequest {
            last_failure_class: None,
            mode: "fix",
            requested_backend: "auto",
            requested_model: Some("gpt-5"),
            recommended_backend: Some("codex"),
            recommended_model: Some("gpt-5"),
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();

    assert_ne!(decision.effective_backend, "codex");
    assert!(decision
        .routing_reason
        .contains("backend-wide manual_disable"));
}

#[test]
fn model_specific_block_only_blocks_that_model() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &path(&tmp),
        "codex",
        Some("gpt-5"),
        Reason::RateLimited,
        Source::BackendError,
        Some(now + time::Duration::minutes(10)),
        None,
        now,
    )
    .unwrap();
    record_available(
        &path(&tmp),
        "codex",
        Some("gpt-5-mini"),
        Source::Manual,
        now,
    )
    .unwrap();

    let blocked = decide_with(
        &defaults(),
        &profile(),
        RouteRequest {
            last_failure_class: None,
            mode: "fix",
            requested_backend: "auto",
            requested_model: Some("gpt-5"),
            recommended_backend: Some("codex"),
            recommended_model: Some("gpt-5"),
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();
    assert_ne!(blocked.effective_backend, "codex");

    let allowed = decide_with(
        &defaults(),
        &profile(),
        RouteRequest {
            last_failure_class: None,
            mode: "fix",
            requested_backend: "auto",
            requested_model: Some("gpt-5-mini"),
            recommended_backend: Some("codex"),
            recommended_model: Some("gpt-5-mini"),
            session_id: None,
            usage_summary: None,
        },
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(allowed.effective_backend, "codex");
}

#[test]
fn manual_disable_blocks_indefinitely() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &path(&tmp),
        "claude",
        None,
        Reason::ManualDisable,
        Source::Manual,
        None,
        Some("disabled".into()),
        now,
    )
    .unwrap();

    let decision = decide_with(
        &defaults(),
        &profile(),
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
        now + time::Duration::days(30),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
}

#[test]
fn all_candidates_unavailable_returns_earliest_reset() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    for (backend, mins) in [("claude", 30), ("codex", 10), ("openhands", 20)] {
        record_unavailable(
            &path(&tmp),
            backend,
            None,
            Reason::RateLimited,
            Source::BackendError,
            Some(now + time::Duration::minutes(mins)),
            None,
            now,
        )
        .unwrap();
    }

    let err = decide_with(
        &defaults(),
        &profile(),
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
        now,
        backend_available,
    )
    .unwrap_err();

    let route_err = err.downcast_ref::<RouteError>().unwrap();
    match route_err {
        RouteError::NoEligibleBackend { earliest_reset, .. } => {
            let expected = (now + time::Duration::minutes(10))
                .format(&Rfc3339)
                .unwrap();
            assert_eq!(earliest_reset.as_deref(), Some(expected.as_str()));
        }
        other => panic!("expected no eligible backend, got {other:?}"),
    }
}

#[test]
fn fallback_route_records_availability_reason() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &path(&tmp),
        "claude",
        None,
        Reason::BackendOutage,
        Source::BackendError,
        Some(now + time::Duration::minutes(5)),
        None,
        now,
    )
    .unwrap();

    let decision = decide_with(
        &defaults(),
        &profile(),
        RouteRequest {
            last_failure_class: None,
            mode: "review",
            requested_backend: "claude",
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

    assert_eq!(decision.effective_backend, "codex");
    assert!(decision.routing_reason.contains("backend_outage"));
    assert!(decision.human_required);
    assert_eq!(decision.confidence_impact.as_deref(), Some("low"));
}

#[test]
fn malformed_availability_state_surfaces_error() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(path(&tmp), "{ not json").unwrap();
    let err = decide_with(
        &defaults(),
        &profile(),
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
    .unwrap_err();

    assert!(format!("{:#}", err).contains("parsing availability state"));
}

#[test]
fn candidate_list_honored_when_available() {
    let tmp = TempDir::new().unwrap();
    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        candidate_config("codex", Some("gpt-4"), None),
        candidate_config("claude", None, None),
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
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-4"));
    assert_eq!(decision.routing_reason, "profile routing policy");
    assert!(!decision.fallback_used);
}

#[test]
fn candidate_list_skips_unavailable_candidates() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();

    record_unavailable(
        &path(&tmp),
        "codex",
        Some("gpt-4"),
        Reason::RateLimited,
        Source::BackendError,
        Some(now + time::Duration::minutes(10)),
        None,
        now,
    )
    .unwrap();

    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        candidate_config("codex", Some("gpt-4"), None),
        candidate_config("claude", None, None),
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
        now,
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model, None);
    assert!(decision.fallback_used);
    assert!(decision
        .routing_reason
        .contains("codex/gpt-4: model-specific rate_limited"));
    let diagnostics = decision.routing_diagnostics.as_ref().unwrap();
    assert!(!diagnostics.policy_reordered_candidates);
    assert_eq!(diagnostics.candidates.len(), 2);
    assert_eq!(diagnostics.candidates[0].backend, "codex");
    assert_eq!(
        diagnostics.candidates[0].skip_reason.as_deref(),
        Some("model-specific rate_limited")
    );
    assert_eq!(diagnostics.candidates[1].backend, "claude");
    assert_eq!(diagnostics.selected_backend.as_deref(), Some("claude"));
}

#[test]
fn candidate_list_expired_availability_re_enters() {
    let tmp = TempDir::new().unwrap();
    let observed = OffsetDateTime::now_utc() - time::Duration::hours(2);

    record_unavailable(
        &path(&tmp),
        "codex",
        Some("gpt-4"),
        Reason::RateLimited,
        Source::BackendError,
        Some(observed + time::Duration::minutes(30)),
        None,
        observed,
    )
    .unwrap();

    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        candidate_config("codex", Some("gpt-4"), None),
        candidate_config("claude", None, None),
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
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-4"));
    assert!(!decision.fallback_used);
}

#[test]
fn candidate_list_exhausted_errors() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();

    for (backend, model) in [("codex", Some("gpt-4")), ("claude", None)] {
        record_unavailable(
            &path(&tmp),
            backend,
            model,
            Reason::RateLimited,
            Source::BackendError,
            Some(now + time::Duration::minutes(10)),
            None,
            now,
        )
        .unwrap();
    }

    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        candidate_config("codex", Some("gpt-4"), None),
        candidate_config("claude", None, None),
    ]);

    let err = decide_with(
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
        now,
        backend_available,
    )
    .unwrap_err();

    let route_err = err.downcast_ref::<RouteError>().unwrap();
    match route_err {
        RouteError::NoEligibleBackend {
            preferred_backend,
            preferred_model,
            skipped,
            earliest_reset,
        } => {
            assert_eq!(preferred_backend, "codex");
            assert_eq!(preferred_model.as_deref(), Some("gpt-4"));
            assert_eq!(skipped.len(), 2);
            assert_eq!(skipped[0].backend, "codex");
            assert_eq!(skipped[0].reason, "model-specific rate_limited");
            assert_eq!(skipped[1].backend, "claude");
            assert_eq!(skipped[1].reason, "backend-wide rate_limited");
            assert!(earliest_reset.is_some());
        }
        other => panic!("expected no eligible backend, got {other:?}"),
    }
}

#[test]
fn routing_honors_shared_quota_pool() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();

    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        candidate_config("claude", Some("claude-sonnet"), Some("claude-main")),
        candidate_config("claude", Some("claude-haiku"), Some("claude-main")),
        candidate_config("codex", Some("gpt-4"), Some("codex-main")),
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
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model.as_deref(), Some("claude-sonnet"));
    assert_eq!(
        decision.effective_quota_pool.as_deref(),
        Some("claude-main")
    );

    crate::availability::record_unavailable(
        &path(&tmp),
        "claude",
        Some("claude-sonnet"),
        Some("claude-main"),
        Reason::QuotaExhausted,
        Source::BackendError,
        Some(now + time::Duration::minutes(10)),
        None,
        now,
    )
    .unwrap();

    let decision2 = decide_with(
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
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(decision2.effective_backend, "codex");
    assert_eq!(decision2.effective_model.as_deref(), Some("gpt-4"));
    assert_eq!(
        decision2.effective_quota_pool.as_deref(),
        Some("codex-main")
    );
    assert!(decision2.fallback_used);
}

#[test]
fn validation_retry_skips_every_route_already_tried_in_the_same_dispatch() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    let mut profile = profile();
    profile.routing.improve_candidates = Some(vec![
        candidate_config("agy", Some("sonnet"), None),
        candidate_config("agy-second", Some("sonnet"), None),
        candidate_config("claude", Some("sonnet"), None),
    ]);
    let mut runtime = RoutingRuntimeState::default();
    runtime
        .dispatch_attempted
        .insert(CandidateIdentity::new("agy", Some("sonnet")));
    let request = RouteRequest {
        last_failure_class: Some("validation_failure"),
        mode: "improve",
        requested_backend: "auto",
        requested_model: None,
        recommended_backend: None,
        recommended_model: None,
        session_id: None,
        usage_summary: None,
    };

    let second = decide_with_runtime(
        &defaults(),
        &profile,
        request.clone(),
        &runtime,
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(second.effective_backend, "agy-second");
    assert_eq!(second.effective_model.as_deref(), Some("sonnet"));

    runtime
        .dispatch_attempted
        .insert(CandidateIdentity::new("agy-second", Some("sonnet")));
    let third = decide_with_runtime(
        &defaults(),
        &profile,
        request,
        &runtime,
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();
    assert_eq!(third.effective_backend, "claude");
    assert_eq!(third.effective_model.as_deref(), Some("sonnet"));
}

#[test]
fn validation_retry_returns_structured_no_route_when_all_candidates_were_tried() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    let mut profile = profile();
    profile.routing.improve_candidates = Some(vec![
        candidate_config("agy", Some("sonnet"), None),
        candidate_config("agy-second", Some("sonnet"), None),
    ]);
    let mut runtime = RoutingRuntimeState::default();
    runtime
        .dispatch_attempted
        .insert(CandidateIdentity::new("agy", Some("sonnet")));
    runtime
        .dispatch_attempted
        .insert(CandidateIdentity::new("agy-second", Some("sonnet")));

    let err = decide_with_runtime(
        &defaults(),
        &profile,
        RouteRequest {
            last_failure_class: Some("validation_failure"),
            mode: "improve",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
        },
        &runtime,
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap_err();

    assert!(matches!(
        err.downcast_ref::<RouteError>(),
        Some(RouteError::NoEligibleBackend { skipped, .. })
            if skipped.iter().all(|candidate| candidate.reason == "already_attempted_after_capability_failure")
    ));
}
