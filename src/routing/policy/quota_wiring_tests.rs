//! Issue #761: `route_candidates`'s live-`quota_store` fallback, split out
//! of `tests.rs` (which carries a tracked line-count baseline --
//! `tests/source_structure.rs` forbids adding a *new* baseline exception,
//! only lowering an existing one, so this test moved to its own file
//! rather than pushing `tests.rs` further over).

use super::*;
use crate::routing::policy::{live_quota_pacing_inputs, ExecutionIdentity};

#[test]
fn live_weekly_pacing_rejects_other_windows_stale_and_failed_checks() {
    let now = OffsetDateTime::now_utc();
    let identity = ExecutionIdentity::legacy_candidate("codex", Some("gpt-5.4"), None::<String>);
    let fresh = crate::quota_store::QuotaObservationRecord {
        backend: "codex".into(),
        backend_instance: None,
        model: Some("gpt-5.4".into()),
        quota_pool: None,
        quota_window: Some("weekly".into()),
        quota_remaining_percent: Some(80.0),
        quota_reset_at: (now + time::Duration::days(5)).format(&Rfc3339).ok(),
        observed_at: now.format(&Rfc3339).ok(),
        checked_at: None,
        check_error: None,
        usage_source: Some("test".into()),
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
    };
    assert_eq!(
        live_quota_pacing_inputs(std::slice::from_ref(&fresh), &identity, now).0,
        Some(20.0)
    );
    for window in ["subscription-monthly", "5h", "daily"] {
        let mut other = fresh.clone();
        other.quota_window = Some(window.into());
        assert_eq!(
            live_quota_pacing_inputs(&[other.clone()], &identity, now),
            (None, None)
        );
        assert_eq!(
            live_quota_pacing_inputs(&[fresh.clone(), other], &identity, now).0,
            Some(20.0)
        );
    }
    for observed in [
        now - time::Duration::minutes(31),
        now + time::Duration::seconds(1),
    ] {
        let mut invalid = fresh.clone();
        invalid.observed_at = observed.format(&Rfc3339).ok();
        assert_eq!(
            live_quota_pacing_inputs(&[invalid], &identity, now),
            (None, None)
        );
    }
    let mut expired = fresh.clone();
    expired.quota_reset_at = now.format(&Rfc3339).ok();
    assert_eq!(
        live_quota_pacing_inputs(&[expired], &identity, now),
        (None, None)
    );
    let mut failed = fresh.clone();
    failed.check_error = Some("failed".into());
    failed.checked_at = (now + time::Duration::seconds(1)).format(&Rfc3339).ok();
    assert_eq!(
        live_quota_pacing_inputs(&[fresh, failed], &identity, now),
        (None, None)
    );
}

// Fresh quota observations must reach the routing decision and diagnostics.
#[test]
fn cost_aware_ordering_uses_live_quota_store_data() {
    let tmp = TempDir::new().unwrap();
    let (_quota_tmp, _quota_store_guard) = seed_weekly_quota(80.0);
    let now = OffsetDateTime::now_utc();

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
            requires_approval: false,
            ..Default::default()
        },
        crate::config::CandidateConfig {
            backend: "codex".into(),
            instance: None,
            model: Some("gpt-5.4".into()),
            quota_pool: Some("codex-main".into()),
            priority: 0,
            included_in_quota: true,
            marginal_cost_usd: Some(0.0),
            requires_approval: false,
            ..Default::default()
        },
    ]);

    let candidates = crate::routing::policy::route_candidates(
        &profile.routing,
        profile.routing.pm_candidates.as_ref().unwrap(),
    );
    assert_eq!(candidates[0].quota_usage_percent, None);
    assert_eq!(candidates[0].quota_days_remaining, None);
    assert_eq!(candidates[1].quota_usage_percent, Some(20.0));
    assert!(candidates[1].quota_days_remaining.is_some());

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
        Some("mild_burn"),
        "live quota_store data (20% used, 5 days to reset) must reach quota_pace"
    );
}

pub(super) fn seed_weekly_quota(
    remaining: f64,
) -> (TempDir, crate::test_support::QuotaStoreEnvGuard) {
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
            quota_remaining_percent: Some(remaining),
            quota_reset_at: (now + time::Duration::days(5)).format(&Rfc3339).ok(),
            observed_at: now.format(&Rfc3339).ok(),
            checked_at: None,
            check_error: None,
            usage_source: Some("codex_status_json".to_string()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        },
    )
    .unwrap();

    (quota_tmp, _quota_store_guard)
}
