//! Issue #761: `route_candidates`'s live-`quota_store` fallback, split out
//! of `tests.rs` (which carries a tracked line-count baseline --
//! `tests/source_structure.rs` forbids adding a *new* baseline exception,
//! only lowering an existing one, so this test moved to its own file
//! rather than pushing `tests.rs` further over).

use super::*;

// Same scenario as cost_aware_ordering_prefers_underpace_included_quota in
// tests.rs, but the operator opted "codex" into quota-aware pacing
// (included_in_quota: true) WITHOUT hand-typing quota_usage_percent/
// quota_days_remaining -- those come from a live quota_store observation
// instead, proving the routing-side wiring (route_candidates's fallback to
// quota_store::latest_for_identity) actually reaches a real decision, not
// just that the record round-trips through some internal struct.
#[test]
fn cost_aware_ordering_uses_live_quota_store_data_when_config_does_not_hardcode_it() {
    let tmp = TempDir::new().unwrap();
    let quota_tmp = TempDir::new().unwrap();
    let _quota_store_guard = crate::test_support::QuotaStoreEnvGuard::set(
        quota_tmp.path().join("quota_observations.jsonl"),
    );
    let now = OffsetDateTime::now_utc();
    crate::quota_store::append(
        &crate::quota_store::store_path(),
        &crate::quota_store::QuotaObservationRecord {
            backend: "codex".to_string(),
            backend_instance: None,
            model: Some("gpt-5.4".to_string()),
            quota_pool: None,
            quota_window: Some("weekly".to_string()),
            quota_used_percent: Some(20.0),
            quota_remaining_percent: None,
            quota_reset_at: (now + time::Duration::days(5)).format(&Rfc3339).ok(),
            observed_at: now.format(&Rfc3339).ok(),
            checked_at: None,
            check_error: None,
            usage_source: Some("codex_status_json".to_string()),
            mistral_admin: None,
        },
    )
    .unwrap();

    let mut profile = profile();
    profile.routing.pm_candidates = Some(vec![
        crate::config::CandidateConfig {
            backend: "openhands".into(),
            instance: None,
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
            instance: None,
            model: Some("gpt-5.4".into()),
            quota_pool: Some("codex-main".into()),
            priority: 0,
            included_in_quota: true,
            marginal_cost_usd: Some(0.0),
            // Deliberately not hardcoded -- must come from the quota_store
            // fixture appended above instead.
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
            exact_route_required: false,
        },
        &path(&tmp),
        now,
        backend_available,
    )
    .unwrap();

    assert_eq!(decision.effective_backend, "codex");
    assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
    let diagnostics = decision.routing_diagnostics.as_ref().unwrap();
    assert!(diagnostics.policy_reordered_candidates);
    assert_eq!(
        diagnostics.selected_pace_band.as_deref(),
        Some("aggressive_burn"),
        "live quota_store data (20% used, 5 days to reset) must reach quota_pace \
         the same way a hardcoded config value would"
    );
}
