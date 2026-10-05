use super::super::test_support::{
    backend_available, candidate_config, defaults, path, profile, record_unavailable,
};
use super::CandidateIdentity;
use super::{
    decide_with, decide_with_runtime, RouteDecision, RouteError, RouteRequest, RoutingRuntimeState,
};
use crate::availability::{Reason, Source};
use crate::config::{Profile, RoutingPolicy};
use tempfile::TempDir;
use time::OffsetDateTime;

fn request<'a>(mode: &'a str, backend: &'a str, model: Option<&'a str>) -> RouteRequest<'a> {
    RouteRequest {
        last_failure_class: None,
        mode,
        requested_backend: backend,
        requested_model: model,
        recommended_backend: None,
        recommended_model: None,
        session_id: None,
        usage_summary: None,
        exact_route_required: false,
    }
}

/// Review is restricted to claude/opus while the ordinary pools still list
/// other reviewers and implementers.
fn opus_only_review_profile() -> Profile {
    let mut profile = profile();
    let pool = vec![
        candidate_config("codex", Some("gpt-4"), None),
        candidate_config("claude", Some("sonnet"), None),
    ];
    profile.routing.review_candidates = Some(pool.clone());
    profile.routing.improve_candidates = Some(pool);
    profile.routing.allowed_models.insert(
        "review".into(),
        vec![candidate_config("claude", Some("opus"), None)],
    );
    profile
}

fn decide(
    profile: &Profile,
    req: RouteRequest<'_>,
    tmp: &TempDir,
) -> anyhow::Result<RouteDecision> {
    decide_with(
        &defaults(),
        profile,
        req,
        &path(tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
}

#[test]
fn auto_review_routes_only_to_the_allowed_model() {
    let tmp = TempDir::new().unwrap();
    let decision = decide(
        &opus_only_review_profile(),
        request("review", "auto", None),
        &tmp,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model.as_deref(), Some("opus"));
    assert!(!decision.fallback_used);
    assert_eq!(decision.routing_reason, "allowed models for review jobs");
}

#[test]
fn review_waits_instead_of_falling_back_when_the_allowed_model_is_unavailable() {
    let tmp = TempDir::new().unwrap();
    let now = OffsetDateTime::now_utc();
    record_unavailable(
        &path(&tmp),
        "claude",
        Some("opus"),
        Reason::RateLimited,
        Source::BackendError,
        Some(now + time::Duration::minutes(10)),
        None,
        now,
    )
    .unwrap();

    // `defaults()` enables review fallback; the allow-list must still hold.
    let err = decide(
        &opus_only_review_profile(),
        request("review", "auto", None),
        &tmp,
    )
    .unwrap_err();

    let route_error = err.downcast_ref::<RouteError>().unwrap();
    assert!(route_error.is_capacity_deferral());
}

#[test]
fn explicit_route_outside_the_allow_list_is_refused() {
    let tmp = TempDir::new().unwrap();
    let profile = opus_only_review_profile();

    let err = decide(&profile, request("review", "codex", Some("gpt-4")), &tmp).unwrap_err();
    assert!(err
        .to_string()
        .contains("codex/gpt-4 is not in routing.allowed_models for review jobs"));

    let allowed = decide(&profile, request("review", "claude", Some("opus")), &tmp).unwrap();
    assert_eq!(allowed.effective_model.as_deref(), Some("opus"));
}

#[test]
fn job_kinds_without_an_entry_keep_their_ordinary_pool() {
    let tmp = TempDir::new().unwrap();
    let profile = opus_only_review_profile();

    let auto = decide(&profile, request("improve", "auto", None), &tmp).unwrap();
    assert_ne!(auto.effective_model.as_deref(), Some("opus"));
    assert!(auto.routing_reason.starts_with("profile routing policy"));

    let explicit = decide(&profile, request("fix", "claude", Some("sonnet")), &tmp).unwrap();
    assert_eq!(explicit.effective_model.as_deref(), Some("sonnet"));
}

#[test]
fn an_entry_without_a_model_allows_every_model_of_that_backend() {
    let tmp = TempDir::new().unwrap();
    let mut profile = opus_only_review_profile();
    profile.routing.allowed_models.insert(
        "review".into(),
        vec![candidate_config("claude", None, None)],
    );

    decide(&profile, request("review", "claude", Some("sonnet")), &tmp).unwrap();
    decide(&profile, request("review", "codex", Some("gpt-4")), &tmp).unwrap_err();
}

#[test]
fn an_unknown_job_kind_is_a_config_error() {
    let mut profile = profile();
    profile
        .routing
        .allowed_models
        .insert("dispatch_ticket".into(), vec![]);

    let errors = crate::config::check_profile_candidate_model_consistency(&defaults(), &profile)
        .unwrap_err();
    assert_eq!(
        errors,
        ["routing.allowed_models: unknown job kind 'dispatch_ticket'"]
    );
}

#[test]
fn allowed_models_merge_by_job_kind_with_profile_entries_winning() {
    let only = |backend: &str| {
        vec![crate::config::CandidateConfig {
            backend: backend.into(),
            ..crate::config::CandidateConfig::default()
        }]
    };
    let defaults = RoutingPolicy {
        allowed_models: [("review".into(), only("codex")), ("pm".into(), only("agy"))].into(),
        ..RoutingPolicy::default()
    };
    let profile = RoutingPolicy {
        allowed_models: [("review".into(), only("claude"))].into(),
        ..RoutingPolicy::default()
    };

    let merged = profile.merged_with_defaults(&defaults);
    assert!(merged.allows_model("review", "claude", Some("opus")));
    assert!(!merged.allows_model("review", "codex", None));
    assert!(merged.allows_model("pm", "agy", None));
    assert!(!merged.allows_model("pm", "claude", None));
    assert!(merged.allows_model("improve", "anything", None));
}

#[test]
fn auto_backend_model_override_outside_the_allow_list_is_refused() {
    let tmp = TempDir::new().unwrap();
    let err = decide(
        &opus_only_review_profile(),
        request("review", "auto", Some("sonnet")),
        &tmp,
    )
    .unwrap_err();
    assert!(err.to_string().contains(
        "claude/sonnet is not in routing.allowed_models for review jobs on this profile"
    ));
}

#[test]
fn auto_backend_model_override_on_the_allow_list_is_selected() {
    let tmp = TempDir::new().unwrap();
    let decision = decide(
        &opus_only_review_profile(),
        request("review", "auto", Some("opus")),
        &tmp,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model.as_deref(), Some("opus"));
}

#[test]
fn auto_backend_model_override_on_a_backend_level_allow_list_entry_is_selected() {
    let tmp = TempDir::new().unwrap();
    let mut profile = opus_only_review_profile();
    profile.routing.allowed_models.insert(
        "review".into(),
        vec![candidate_config("claude", None, None)],
    );

    let decision = decide(&profile, request("review", "auto", Some("sonnet")), &tmp).unwrap();
    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model.as_deref(), Some("sonnet"));
}

fn approval_gated_review_profile() -> Profile {
    let mut profile = profile();
    let mut entry = candidate_config("claude", Some("opus"), Some("claude-team"));
    entry.requires_approval = true;
    profile
        .routing
        .allowed_models
        .insert("review".into(), vec![entry]);
    profile
}

#[test]
fn allow_list_only_entry_requires_approval_on_the_explicit_route() {
    let tmp = TempDir::new().unwrap();
    let err = decide(
        &approval_gated_review_profile(),
        request("review", "claude", Some("opus")),
        &tmp,
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<RouteError>(),
        Some(RouteError::ApprovalRequired { backend, model, .. })
            if backend == "claude" && model.as_deref() == Some("opus")
    ));
}

#[test]
fn approved_allow_list_entry_dispatches_on_the_explicit_route_with_its_quota_pool() {
    let tmp = TempDir::new().unwrap();
    let mut runtime = RoutingRuntimeState::default();
    runtime
        .approved
        .insert(CandidateIdentity::new("claude", Some("opus")));

    let decision = decide_with_runtime(
        &defaults(),
        &approval_gated_review_profile(),
        request("review", "claude", Some("opus")),
        &runtime,
        &path(&tmp),
        OffsetDateTime::now_utc(),
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model.as_deref(), Some("opus"));
    assert_eq!(
        decision.effective_quota_pool.as_deref(),
        Some("claude-team")
    );
}

#[test]
fn auto_backend_model_override_keeps_the_quota_pool_from_the_allow_list() {
    let tmp = TempDir::new().unwrap();
    let mut profile = opus_only_review_profile();
    profile.routing.allowed_models.insert(
        "review".into(),
        vec![candidate_config(
            "claude",
            Some("opus"),
            Some("claude-team"),
        )],
    );

    let decision = decide(&profile, request("review", "auto", Some("opus")), &tmp).unwrap();
    assert_eq!(decision.effective_backend, "claude");
    assert_eq!(decision.effective_model.as_deref(), Some("opus"));
    assert_eq!(
        decision.effective_quota_pool.as_deref(),
        Some("claude-team")
    );
}
