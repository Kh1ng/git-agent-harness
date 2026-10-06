use super::*;

#[test]
fn quota_provider_identifies_the_billed_service_instead_of_the_harness() {
    assert_eq!(
        quota_provider("opencode", Some("nous-portal/deepseek/deepseek-v4")),
        Some("nous".into())
    );
    assert_eq!(
        quota_provider("vibe", Some("devstral-small")),
        Some("mistral".into())
    );
    assert_eq!(
        quota_provider("agy-second", Some("Claude Sonnet 4.6")),
        Some("antigravity".into())
    );
    assert_eq!(quota_provider("opencode", None), None);
    assert_eq!(
        quota_provider("opencode", Some("gah-router/gemini-3.1-pro")),
        None
    );
}
use crate::availability::{BlockScope, Reason, ScopeStatus, Source};
use crate::config::tests::test_profile_for_notifications;

/// A `GroupSummary` with every field zeroed/empty, so individual tests
/// only spell out the fields they actually care about via struct-update
/// syntax (`GroupSummary { entries: 3, ..empty_group() }`). No `Default`
/// impl exists on the production type (see `ledger/mod.rs`), so this mirrors
/// that module's own fixture convention.
fn empty_group() -> ledger::summary::GroupSummary {
    ledger::summary::GroupSummary {
        usage_unknown_reasons: Default::default(),
        group_key: "g".to_string(),
        entries: 0,
        attempts: 0,
        attempts_started: None,
        attempts_completed: None,
        attempts_started_unknown: 0,
        attempts_completed_unknown: 0,
        validation_pass: 0,
        success_rate: None,
        review_verdict_distribution: Default::default(),
        total_cost_usd: None,
        actual_cost_usd: None,
        estimated_cost_usd: None,
        average_cost_usd: None,
        average_duration_seconds: None,
        cost_per_approve_strong: None,
        input_tokens: None,
        output_tokens: None,
        reasoning_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        total_tokens: None,
        memory_gateway_capture_l0_recorded: None,
        requests_count: None,
        tokens_per_success: None,
        requests_per_success: None,
        predicted_average_cost_usd: None,
        predicted_average_duration_seconds: None,
        predicted_difficulty_match_rate: None,
        total_cpu_time_seconds: None,
        peak_rss_bytes: None,
        quota_observations: vec![],
    }
}

fn group_obs(
    backend: &str,
    model: Option<&str>,
    window: &str,
    remaining_percent: Option<f64>,
    observed_at: &str,
) -> crate::quota_store::QuotaObservationRecord {
    crate::quota_store::QuotaObservationRecord {
        backend: backend.to_string(),
        backend_instance: None,
        credential_id: None,
        model: model.map(str::to_string),
        quota_pool: None,
        quota_window: Some(window.to_string()),
        quota_remaining_percent: remaining_percent,
        quota_reset_at: None,
        observed_at: Some(observed_at.to_string()),
        checked_at: None,
        check_error: None,
        usage_source: None,
        mistral_admin: None,
        account_usage: None,
    }
}

fn account_record(
    backend: &str,
    model: Option<&str>,
    window: &str,
    remaining_percent: Option<f64>,
    observed_at: &str,
) -> quota_store::QuotaObservationRecord {
    quota_store::QuotaObservationRecord {
        backend: backend.to_string(),
        backend_instance: None,
        model: model.map(str::to_string),
        quota_pool: None,
        quota_window: Some(window.to_string()),
        quota_remaining_percent: remaining_percent,
        quota_reset_at: None,
        observed_at: Some(observed_at.to_string()),
        checked_at: None,
        check_error: None,
        usage_source: None,
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
    }
}

fn availability_status(
    backend: &str,
    backend_instance: Option<&str>,
    model: Option<&str>,
    quota_pool: Option<&str>,
    eligible: bool,
    scope: Option<BlockScope>,
) -> ScopeStatus {
    ScopeStatus {
        backend: backend.into(),
        backend_instance: backend_instance.map(str::to_string),
        model: model.map(str::to_string),
        quota_pool: quota_pool.map(str::to_string),
        eligible,
        reason: (!eligible).then_some(Reason::QuotaExhausted),
        unavailable_until: None,
        scope,
        source: Some(Source::Imported),
        last_error_summary: None,
        observed_at: Some("2026-07-20T00:00:00Z".into()),
    }
}

#[test]
fn candidate_status_inherits_backend_wide_and_cross_instance_pool_state() {
    let candidate = CandidateKey {
        backend: "opencode".into(),
        backend_instance: "opencode-account-a".into(),
        model: Some("shared-model".into()),
        quota_pool: None,
    };
    let mut lookup = AvailabilityScopeLookup::new();
    lookup.insert(
        ("opencode".into(), None, None, None),
        availability_status(
            "opencode",
            None,
            None,
            None,
            false,
            Some(BlockScope::BackendWide),
        ),
    );
    assert!(!find_scope_status(&lookup, &candidate).unwrap().eligible);

    let pooled = CandidateKey {
        quota_pool: Some("shared-pool".into()),
        ..candidate
    };
    lookup.insert(
        (
            "opencode".into(),
            Some("opencode-account-b".into()),
            Some("other-model".into()),
            Some("shared-pool".into()),
        ),
        availability_status(
            "opencode",
            Some("opencode-account-b"),
            Some("other-model"),
            Some("shared-pool"),
            true,
            None,
        ),
    );
    assert!(find_scope_status(&lookup, &pooled).unwrap().eligible);
}

// -- summarize_groups --------------------------------------------------

#[test]
fn summarize_groups_sums_across_groups_and_computes_success_rate() {
    let a = ledger::summary::GroupSummary {
        entries: 4,
        attempts: 5,
        validation_pass: 3,
        total_tokens: Some(100),
        requests_count: Some(10),
        actual_cost_usd: Some(1.5),
        estimated_cost_usd: Some(0.5),
        ..empty_group()
    };
    let b = ledger::summary::GroupSummary {
        entries: 6,
        attempts: 6,
        validation_pass: 3,
        total_tokens: Some(200),
        requests_count: Some(20),
        actual_cost_usd: Some(2.0),
        estimated_cost_usd: None,
        ..empty_group()
    };
    let summary = summarize_groups(vec![a, b]);
    assert_eq!(summary.entries, 10);
    assert_eq!(summary.attempts, 11);
    assert_eq!(summary.validation_pass, 6);
    assert_eq!(summary.total_tokens, Some(300));
    assert_eq!(summary.requests_count, Some(30));
    assert!((summary.actual_cost_usd.unwrap() - 3.5).abs() < f64::EPSILON);
    // Only `a` has an estimated cost; `b`'s None must not zero it out.
    assert!((summary.estimated_cost_usd.unwrap() - 0.5).abs() < f64::EPSILON);
    assert!((summary.success_rate.unwrap() - 0.6).abs() < f64::EPSILON);
}

#[test]
fn summarize_groups_empty_input_has_no_success_rate_or_totals() {
    let summary = summarize_groups(vec![]);
    assert_eq!(summary.entries, 0);
    assert_eq!(summary.success_rate, None);
    assert_eq!(summary.total_tokens, None);
    assert_eq!(summary.requests_count, None);
}

#[test]
fn summarize_groups_all_none_token_fields_stay_none_not_zero() {
    // Regression guard: a group that never reported tokens must leave the
    // aggregate at `None` ("unknown"), not silently become `Some(0)`.
    let a = ledger::summary::GroupSummary {
        entries: 2,
        attempts: 2,
        validation_pass: 1,
        ..empty_group()
    };
    let summary = summarize_groups(vec![a]);
    assert_eq!(summary.total_tokens, None);
    assert_eq!(summary.requests_count, None);
    assert_eq!(summary.actual_cost_usd, None);
}

#[test]
fn latest_timestamp_compares_instants_instead_of_rfc3339_text() {
    let latest = latest_timestamp(
        [
            "2026-07-20T23:00:00-05:00".to_string(),
            "2026-07-21T02:00:00Z".to_string(),
        ]
        .into_iter(),
    );
    assert_eq!(latest.as_deref(), Some("2026-07-20T23:00:00-05:00"));
}

// -- aggregate_usage -----------------------------------------------------

#[test]
fn aggregate_usage_prefers_model_group_over_backend_group() {
    let backend = ledger::summary::GroupSummary {
        entries: 10,
        ..empty_group()
    };
    let model = ledger::summary::GroupSummary {
        entries: 3,
        ..empty_group()
    };
    let usage = aggregate_usage(Some(&backend), Some(&model));
    assert_eq!(usage.entries, 3);
}

#[test]
fn aggregate_usage_falls_back_to_backend_group_when_no_model_group() {
    let backend = ledger::summary::GroupSummary {
        entries: 10,
        ..empty_group()
    };
    let usage = aggregate_usage(Some(&backend), None);
    assert_eq!(usage.entries, 10);
}

#[test]
fn aggregate_usage_defaults_when_neither_group_present() {
    let usage = aggregate_usage(None, None);
    assert_eq!(usage.entries, 0);
    assert_eq!(usage.success_rate, None);
}

// -- aggregate_observations ----------------------------------------------

#[test]
fn aggregate_observations_combines_backend_and_model_group_observations() {
    let backend = ledger::summary::GroupSummary {
        quota_observations: vec![group_obs(
            "codex",
            None,
            "weekly",
            Some(50.0),
            "2026-07-01T00:00:00Z",
        )],
        ..empty_group()
    };
    let model = ledger::summary::GroupSummary {
        quota_observations: vec![group_obs(
            "codex",
            Some("gpt-5"),
            "5h",
            Some(80.0),
            "2026-07-02T00:00:00Z",
        )],
        ..empty_group()
    };
    let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "codex",
        Some("gpt-5"),
        None::<String>,
    );
    let obs = aggregate_observations(Some(&backend), Some(&model), &[], &identity);
    assert_eq!(obs.len(), 2);
    assert!(obs
        .iter()
        .any(|o| o.quota_window.as_deref() == Some("weekly")));
    assert!(obs.iter().any(|o| o.quota_window.as_deref() == Some("5h")));
}

#[test]
fn aggregate_observations_scrubs_group_identity_but_preserves_account_records() {
    let mut record = group_obs("codex", None, "weekly", Some(42.0), "2026-07-03T00:00:00Z");
    record.backend_instance = Some("codex".into());
    record.quota_pool = Some("account-pool".into());
    record.credential_id = Some("account-credential".into());
    record.account_usage = Some(
        serde_json::from_value(serde_json::json!({
            "account_id": "account", "workspace_id": null,
            "period_start": "2026-07-01T00:00:00Z",
            "period_end": "2026-08-01T00:00:00Z", "currency": "USD", "models": []
        }))
        .unwrap(),
    );
    let group = ledger::summary::GroupSummary {
        quota_observations: vec![record.clone()],
        ..empty_group()
    };
    let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "codex",
        None::<String>,
        Some("account-pool"),
    );
    let mut account = record.clone();
    account.backend_instance = Some(identity.backend_instance.clone());
    for (backend, model) in [(Some(&group), None), (None, Some(&group))] {
        let observations =
            aggregate_observations(backend, model, std::slice::from_ref(&account), &identity);
        assert_eq!(observations.len(), 2);
        let broad = &observations[0];
        assert!(broad.backend_instance.is_none());
        assert!(broad.quota_pool.is_none());
        assert!(broad.credential_id.is_none());
        assert!(broad.account_usage.is_none());
        assert_eq!(broad.quota_remaining_percent, Some(42.0));
        assert_eq!(broad.observed_at, record.observed_at);
        assert_eq!(
            serde_json::to_value(&observations[1]).unwrap(),
            serde_json::to_value(&account).unwrap()
        );
        assert!(group.quota_observations[0].credential_id.is_some());
    }
}

#[test]
fn aggregate_observations_appends_matching_account_level_observation() {
    let account = vec![account_record(
        "codex",
        None,
        "weekly",
        Some(42.0),
        "2026-07-03T00:00:00Z",
    )];
    let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "codex",
        None::<String>,
        None::<String>,
    );
    let obs = aggregate_observations(None, None, &account, &identity);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].quota_remaining_percent, Some(42.0));
}

#[test]
fn bound_observations_follow_selected_source_instead_of_route_or_ambient_identity() {
    for (runner, collector) in [("opencode", "opencode"), ("vibe", "mistral-dashboard")] {
        let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            runner,
            Some("route-model"),
            Some("route-pool"),
        );
        identity.backend_instance = "runner-work".into();
        identity.credential_id = Some("work-source".into());
        let mut selected = account_record(
            collector,
            None,
            "monthly",
            Some(42.0),
            "2026-07-03T00:00:00Z",
        );
        selected.backend_instance = Some("provider-verified-account".into());
        selected.quota_pool = Some("provider-verified-pool".into());
        selected.credential_id = Some("work-source".into());
        let mut sibling = selected.clone();
        sibling.credential_id = Some("other-source".into());
        sibling.quota_remaining_percent = Some(17.0);
        let ambient = account_record(runner, None, "monthly", Some(90.0), "2026-07-03T00:00:00Z");
        let broad = ledger::summary::GroupSummary {
            quota_observations: vec![group_obs(
                runner,
                None,
                "monthly",
                Some(80.0),
                "2026-07-03T00:00:00Z",
            )],
            ..empty_group()
        };
        let observations = aggregate_observations(
            Some(&broad),
            Some(&broad),
            &[selected, sibling, ambient],
            &identity,
        );
        assert_eq!(observations.len(), 1, "only selected source for {runner}");
        assert_eq!(
            observations[0].credential_id.as_deref(),
            Some("work-source")
        );
        assert_eq!(observations[0].quota_remaining_percent, Some(42.0));
        assert_eq!(
            observations[0].quota_pool.as_deref(),
            Some("provider-verified-pool")
        );
    }
}

#[test]
fn unbound_observations_keep_backend_instance_and_pool_scoping() {
    let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "codex",
        None::<String>,
        Some("route-pool"),
    );
    identity.backend_instance = "runner-work".into();
    let mut selected = account_record("codex", None, "weekly", Some(42.0), "2026-07-03T00:00:00Z");
    selected.backend_instance = Some("runner-work".into());
    selected.quota_pool = Some("route-pool".into());
    let mut other_instance = selected.clone();
    other_instance.backend_instance = Some("runner-other".into());
    let mut other_pool = selected.clone();
    other_pool.quota_pool = Some("other-pool".into());
    let mut other_backend = selected.clone();
    other_backend.backend = "opencode".into();
    let broad = ledger::summary::GroupSummary {
        quota_observations: vec![group_obs(
            "codex",
            None,
            "5h",
            Some(80.0),
            "2026-07-03T00:00:00Z",
        )],
        ..empty_group()
    };
    let observations = aggregate_observations(
        Some(&broad),
        None,
        &[selected, other_instance, other_pool, other_backend],
        &identity,
    );
    assert_eq!(observations.len(), 2);
    assert!(observations
        .iter()
        .all(|observation| observation.credential_id.is_none()));
    assert!(observations
        .iter()
        .any(|observation| observation.quota_remaining_percent == Some(42.0)));
    assert!(observations
        .iter()
        .any(|observation| observation.quota_remaining_percent == Some(80.0)));
}

#[test]
fn aggregate_observations_does_not_leak_account_observation_across_model_scope() {
    // Candidate scoping: an account-level record for "gpt-4" must not
    // surface on a "gpt-5" candidate's observations just because the
    // backend matches.
    let account = vec![account_record(
        "codex",
        Some("gpt-4"),
        "weekly",
        Some(42.0),
        "2026-07-03T00:00:00Z",
    )];
    let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "codex",
        Some("gpt-5"),
        None::<String>,
    );
    let obs = aggregate_observations(None, None, &account, &identity);
    assert!(obs.is_empty());
}

#[test]
fn aggregate_observations_filters_broad_group_data_to_candidate_identity() {
    let backend = ledger::summary::GroupSummary {
        quota_observations: vec![
            group_obs(
                "agy",
                Some("Gemini"),
                "daily",
                Some(70.0),
                "2026-07-03T00:00:00Z",
            ),
            group_obs(
                "agy",
                Some("Claude"),
                "daily",
                Some(20.0),
                "2026-07-03T00:00:00Z",
            ),
            group_obs(
                "agy-second",
                Some("Gemini"),
                "daily",
                Some(5.0),
                "2026-07-03T00:00:00Z",
            ),
        ],
        ..empty_group()
    };

    let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "agy",
        Some("Gemini"),
        None::<String>,
    );
    let obs = aggregate_observations(Some(&backend), None, &[], &identity);

    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].backend, "agy");
    assert_eq!(obs[0].model.as_deref(), Some("Gemini"));
    assert_eq!(obs[0].quota_remaining_percent, Some(70.0));
}

#[test]
fn aggregate_observations_dedups_identical_entries_from_backend_and_model_groups() {
    let dup = group_obs("codex", None, "weekly", Some(50.0), "2026-07-01T00:00:00Z");
    let backend = ledger::summary::GroupSummary {
        quota_observations: vec![dup.clone()],
        ..empty_group()
    };
    let model = ledger::summary::GroupSummary {
        quota_observations: vec![dup],
        ..empty_group()
    };
    let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "codex",
        None::<String>,
        None::<String>,
    );
    let obs = aggregate_observations(Some(&backend), Some(&model), &[], &identity);
    assert_eq!(obs.len(), 1, "identical observations must collapse to one");
}

// -- add_candidate --------------------------------------------------------

#[test]
fn add_candidate_merges_modes_for_the_same_key_without_duplicating() {
    let routing = RoutingPolicy::default();
    let mut aggregates = Vec::new();
    let mut index = HashMap::new();
    let candidate = CandidateConfig {
        backend: "codex".to_string(),
        model: Some("gpt-5".to_string()),
        ..Default::default()
    };
    add_candidate(
        &routing,
        &mut aggregates,
        &mut index,
        "pm",
        candidate.clone(),
    );
    add_candidate(
        &routing,
        &mut aggregates,
        &mut index,
        "improve",
        candidate.clone(),
    );
    add_candidate(&routing, &mut aggregates, &mut index, "pm", candidate);

    assert_eq!(aggregates.len(), 1);
    assert_eq!(aggregates[0].1.modes, vec!["pm", "improve"]);
}

#[test]
fn add_candidate_treats_different_quota_pools_as_distinct_candidates() {
    // Candidate scoping: "agy" and "agy-second" are different instances
    // and must never collapse into a single row (see QuotaPage.tsx's own
    // `scopeIdentity` doc comment for the same invariant on the UI side).
    let routing = RoutingPolicy::default();
    let mut aggregates = Vec::new();
    let mut index = HashMap::new();
    let a = CandidateConfig {
        backend: "agy".to_string(),
        quota_pool: Some("agy".to_string()),
        ..Default::default()
    };
    let b = CandidateConfig {
        backend: "agy".to_string(),
        quota_pool: Some("agy-second".to_string()),
        ..Default::default()
    };
    add_candidate(&routing, &mut aggregates, &mut index, "review", a);
    add_candidate(&routing, &mut aggregates, &mut index, "review", b);

    assert_eq!(aggregates.len(), 2);
}

#[test]
fn add_candidate_uses_declared_instance_identity_and_quota_pool() {
    let mut routing = RoutingPolicy::default();
    routing.backend_instances.insert(
        "opencode-subscription".into(),
        crate::config::BackendInstanceConfig {
            runner_kind: "opencode".into(),
            logical_backend: Some("opencode".into()),
            quota_pool: Some("opencode-plan".into()),
            ..Default::default()
        },
    );
    let mut aggregates = Vec::new();
    let mut index = HashMap::new();
    add_candidate(
        &routing,
        &mut aggregates,
        &mut index,
        "improve",
        CandidateConfig {
            backend: "opencode".into(),
            instance: Some("opencode-subscription".into()),
            model: Some("gpt-5".into()),
            ..Default::default()
        },
    );

    assert_eq!(aggregates[0].0.backend_instance, "opencode-subscription");
    assert_eq!(aggregates[0].0.quota_pool.as_deref(), Some("opencode-plan"));
}

// -- build_candidates -----------------------------------------------------

#[test]
fn build_candidates_falls_back_to_default_backend_when_none_configured() {
    let routing = RoutingPolicy {
        default_backend: Some("vibe".to_string()),
        default_model: Some("mistral-medium".to_string()),
        ..RoutingPolicy::default()
    };
    let profile = test_profile_for_notifications();
    let backend_map = HashMap::new();
    let model_map = HashMap::new();
    let scope_lookup = HashMap::new();
    let account_quota = vec![];

    let candidates = build_candidates(
        &routing,
        &profile,
        &backend_map,
        &model_map,
        &scope_lookup,
        &account_quota,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].backend, "vibe");
    assert_eq!(candidates[0].modes, vec!["default"]);
    assert!(
        candidates[0].eligible_now,
        "no availability record for this scope must default to eligible, not blocked"
    );
}

#[test]
fn build_candidates_lists_allow_list_entries_under_their_job_kind() {
    let routing = RoutingPolicy {
        default_backend: Some("vibe".to_string()),
        allowed_models: [(
            "review".to_string(),
            vec![CandidateConfig {
                backend: "claude".to_string(),
                model: Some("opus".to_string()),
                ..CandidateConfig::default()
            }],
        )]
        .into(),
        ..RoutingPolicy::default()
    };
    let profile = test_profile_for_notifications();
    let candidates = build_candidates(
        &routing,
        &profile,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &[],
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].backend, "claude");
    assert_eq!(candidates[0].modes, vec!["review"]);
}

#[test]
fn build_candidates_keeps_distinct_quota_pools_separately_scoped() {
    // Two candidates sharing a backend but different quota_pool must not
    // share eligibility -- one being blocked must not leak onto the other.
    let routing = RoutingPolicy {
        review_candidates: Some(vec![
            CandidateConfig {
                backend: "agy".to_string(),
                quota_pool: Some("agy".to_string()),
                ..Default::default()
            },
            CandidateConfig {
                backend: "agy".to_string(),
                quota_pool: Some("agy-second".to_string()),
                ..Default::default()
            },
        ]),
        ..RoutingPolicy::default()
    };
    let profile = test_profile_for_notifications();
    let backend_map = HashMap::new();
    let model_map = HashMap::new();
    let mut scope_lookup = HashMap::new();
    scope_lookup.insert(
        ("agy".to_string(), None, None, Some("agy".to_string())),
        ScopeStatus {
            backend: "agy".to_string(),
            backend_instance: None,
            model: None,
            quota_pool: Some("agy".to_string()),
            eligible: false,
            reason: Some(Reason::QuotaExhausted),
            unavailable_until: Some("2026-07-12T00:00:00Z".to_string()),
            scope: Some(BlockScope::QuotaPool),
            source: Some(Source::BackendError),
            last_error_summary: None,
            observed_at: Some("2026-07-11T00:00:00Z".to_string()),
        },
    );
    let account_quota = vec![];

    let candidates = build_candidates(
        &routing,
        &profile,
        &backend_map,
        &model_map,
        &scope_lookup,
        &account_quota,
    );

    assert_eq!(candidates.len(), 2);
    let agy = candidates
        .iter()
        .find(|c| c.quota_pool.as_deref() == Some("agy"))
        .expect("agy pool present");
    assert!(!agy.eligible_now);
    assert_eq!(agy.reason.as_deref(), Some("quota_exhausted"));

    let agy_second = candidates
        .iter()
        .find(|c| c.quota_pool.as_deref() == Some("agy-second"))
        .expect("agy-second pool present");
    assert!(
        agy_second.eligible_now,
        "a block on the 'agy' pool must not leak onto the sibling 'agy-second' pool"
    );
}

#[test]
fn build_candidates_sorts_quota_observations_by_window_then_recency() {
    let routing = RoutingPolicy {
        default_backend: Some("codex".to_string()),
        ..RoutingPolicy::default()
    };
    let profile = test_profile_for_notifications();
    let mut backend_map = HashMap::new();
    backend_map.insert(
        "codex".to_string(),
        ledger::summary::GroupSummary {
            quota_observations: vec![
                group_obs("codex", None, "weekly", Some(10.0), "2026-07-01T00:00:00Z"),
                group_obs("codex", None, "5h", Some(90.0), "2026-07-02T00:00:00Z"),
            ],
            ..empty_group()
        },
    );
    let model_map = HashMap::new();
    let scope_lookup = HashMap::new();
    let account_quota = vec![];

    let candidates = build_candidates(
        &routing,
        &profile,
        &backend_map,
        &model_map,
        &scope_lookup,
        &account_quota,
    );

    assert_eq!(candidates.len(), 1);
    let windows: Vec<String> = candidates[0]
        .quota_observations
        .iter()
        .map(|o| o.quota_window.clone().unwrap())
        .collect();
    assert_eq!(windows, vec!["5h".to_string(), "weekly".to_string()]);
}

#[test]
fn build_candidates_scopes_same_named_model_usage_by_backend() {
    let routing = RoutingPolicy {
        review_candidates: Some(vec![
            CandidateConfig {
                backend: "codex".to_string(),
                model: Some("shared-name".to_string()),
                ..Default::default()
            },
            CandidateConfig {
                backend: "agy".to_string(),
                model: Some("shared-name".to_string()),
                ..Default::default()
            },
        ]),
        ..RoutingPolicy::default()
    };
    let profile = test_profile_for_notifications();
    let mut candidate_map = HashMap::new();
    candidate_map.insert(
        candidate_usage_key("codex", Some("shared-name")),
        ledger::summary::GroupSummary {
            entries: 3,
            ..empty_group()
        },
    );
    candidate_map.insert(
        candidate_usage_key("agy", Some("shared-name")),
        ledger::summary::GroupSummary {
            entries: 9,
            ..empty_group()
        },
    );

    let candidates = build_candidates(
        &routing,
        &profile,
        &HashMap::new(),
        &candidate_map,
        &HashMap::new(),
        &[],
    );

    assert_eq!(candidates[0].usage.entries, 3);
    assert_eq!(candidates[1].usage.entries, 9);
}

#[test]
fn build_candidates_creates_four_distinct_agy_scopes() {
    let routing = RoutingPolicy {
        pm_candidates: Some(vec![
            CandidateConfig {
                backend: "agy".to_string(),
                model: Some("Gemini 3.5 Flash (Medium)".to_string()),
                quota_pool: None,
                ..Default::default()
            },
            CandidateConfig {
                backend: "agy".to_string(),
                model: Some("Claude Sonnet 4.6 (Thinking)".to_string()),
                quota_pool: None,
                ..Default::default()
            },
            CandidateConfig {
                backend: "agy-second".to_string(),
                model: Some("Gemini 3.5 Flash".to_string()),
                quota_pool: Some("agy-second".to_string()),
                ..Default::default()
            },
            CandidateConfig {
                backend: "agy-second".to_string(),
                model: Some("Claude Sonnet 4.6".to_string()),
                quota_pool: None,
                ..Default::default()
            },
        ]),
        ..RoutingPolicy::default()
    };
    let profile = test_profile_for_notifications();
    let candidates = build_candidates(
        &routing,
        &profile,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &[],
    );

    assert_eq!(candidates.len(), 4);
    let pools: Vec<Option<String>> = candidates.iter().map(|c| c.quota_pool.clone()).collect();
    assert_eq!(
        pools,
        vec![
            Some("agy:google-native".to_string()),
            Some("agy:external".to_string()),
            Some("agy-second:google-native".to_string()),
            Some("agy-second:external".to_string()),
        ]
    );
}

#[test]
fn quota_usage_counts_each_unknown_reason_separately() {
    use ledger::UsageUnknownReason::*;
    let reasons = [
        NoAttemptStarted,
        BackendNotInvoked,
        UsageArtifactMissing,
        UsageArtifactUnparsed,
    ];
    let mut group = empty_group();
    group.usage_unknown_reasons = reasons.into_iter().map(|reason| (reason, 1)).collect();
    let candidate = aggregate_usage(Some(&group), None);
    assert_eq!(candidate.usage_unknown_reasons, group.usage_unknown_reasons);
    let total = summarize_groups(vec![group.clone(), group]);
    for reason in reasons {
        assert_eq!(total.usage_unknown_reasons[&reason], 2);
    }
    assert_eq!(total.total_tokens, None);
    assert_eq!(total.requests_count, None);
}
