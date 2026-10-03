//! Subscription preference and fresh capacity used by dispatch and chat handoff.
use crate::{
    config::{CandidateConfig, Defaults, Profile},
    execution_identity::ExecutionIdentity,
    quota_store::QuotaObservationRecord,
};
use serde::Serialize;
use std::cmp::Ordering;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug, Default, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct SubscriptionCapacity {
    pub known_capacity: bool,
    pub exhausted: bool,
    pub reset_at: Option<String>,
    pub reset_pressure: Option<f64>,
}

/// Expired, failed, future-dated, and stale readings never imply capacity or urgency.
pub fn capacity(
    identity: &ExecutionIdentity,
    records: &[QuotaObservationRecord],
    now: OffsetDateTime,
) -> SubscriptionCapacity {
    let latest = crate::quota_store::latest_windows_for_identity(records, identity);
    let mut result = SubscriptionCapacity::default();
    for record in latest {
        if identity.explicit_instance
            && identity.credential_id.is_none()
            && record.backend_instance.is_none()
            && record.quota_pool.is_none()
        {
            continue;
        }
        let Some(observed) = record
            .observed_at
            .as_deref()
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
        else {
            continue;
        };
        if record.check_error.is_some()
            || observed > now
            || now - observed > time::Duration::minutes(30)
        {
            continue;
        }
        let reset = record
            .quota_reset_at
            .as_deref()
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok());
        if reset.is_some_and(|reset| reset <= now) {
            continue;
        }
        let remaining = record
            .quota_remaining_percent
            .or_else(|| record.quota_used_percent.map(|used| 100.0 - used));
        let Some(remaining) =
            remaining.filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        else {
            continue;
        };
        if remaining == 0.0 {
            result.exhausted = true;
            result.reset_at = record.quota_reset_at.clone();
        } else {
            result.known_capacity = true;
        }
        // Short-term throttles restrict eligibility, but are not a budget to spend.
        // Provider window names are protocol values, not substring heuristics.
        let seconds = match record.quota_window.as_deref() {
            Some("weekly" | "seven_day" | "seven-day" | "7d" | "10080m") => Some(7.0 * 86400.0),
            Some("monthly" | "vibe-code-included-monthly") => Some(30.0 * 86400.0),
            _ => None,
        };
        if let (Some(reset), Some(seconds)) = (reset, seconds) {
            let pressure = (remaining / 100.0) / ((reset - now).as_seconds_f64() / seconds);
            result.reset_pressure = Some(
                result
                    .reset_pressure
                    .map_or(pressure, |previous| previous.max(pressure)),
            );
        }
    }
    result
}

fn preference(identity: &ExecutionIdentity, capacity: &SubscriptionCapacity) -> u8 {
    match identity.runner_kind.as_str() {
        "agy" => 0,
        "claude" => 1,
        "vibe"
            if !capacity
                .reset_pressure
                .is_some_and(|pressure| pressure > 1.0) =>
        {
            3
        }
        _ => 2,
    }
}

/// Operator priorities remain outside this comparator and take precedence.
pub fn compare(
    left: &ExecutionIdentity,
    left_capacity: &SubscriptionCapacity,
    right: &ExecutionIdentity,
    right_capacity: &SubscriptionCapacity,
) -> Ordering {
    left_capacity
        .exhausted
        .cmp(&right_capacity.exhausted)
        .then_with(|| preference(left, left_capacity).cmp(&preference(right, right_capacity)))
        .then_with(|| {
            right_capacity
                .known_capacity
                .cmp(&left_capacity.known_capacity)
        })
        .then_with(|| {
            right_capacity
                .reset_pressure
                .unwrap_or(0.0)
                .total_cmp(&left_capacity.reset_pressure.unwrap_or(0.0))
        })
}

#[derive(Debug, Serialize)]
pub struct HandoffRoute {
    pub identity: ExecutionIdentity,
    #[serde(flatten)]
    pub capacity: SubscriptionCapacity,
}

/// Every enabled concrete account is a candidate, including sibling accounts of the failed runner.
pub fn handoff_routes(
    defaults: &Defaults,
    profile: &Profile,
    model: Option<&str>,
    now: OffsetDateTime,
) -> anyhow::Result<Vec<HandoffRoute>> {
    let routing = profile.effective_routing(defaults);
    let mut identities = Vec::new();
    for (name, instance) in &routing.backend_instances {
        if !instance.enabled() {
            continue;
        }
        let backend = instance
            .logical_backend
            .as_deref()
            .unwrap_or(&instance.runner_kind);
        let identity = routing.execution_identity_for_candidate(&CandidateConfig {
            backend: backend.into(),
            instance: Some(name.clone()),
            model: model
                .filter(|model| {
                    instance.supported_models.is_empty()
                        || instance
                            .supported_models
                            .iter()
                            .any(|supported| supported == model)
                })
                .map(str::to_owned),
            ..Default::default()
        });
        if matches!(
            crate::runner::resolve_backend_instance_executable(instance),
            crate::runner::ExecutableResolution::Found(_)
        ) {
            identities.push(identity);
        }
    }
    for backend in [
        "agy",
        "claude",
        "codex",
        "hermes",
        "opencode",
        "vibe",
        "openhands",
    ] {
        if (profile.configured_backend_path(backend).is_some()
            || (backend == "openhands" && profile.oh_profile.is_some()))
            && crate::runner::backend_available_for_profile(profile, backend)
        {
            identities.push(routing.execution_identity_for_candidate(&CandidateConfig {
                backend: backend.into(),
                model: model.map(str::to_owned),
                ..Default::default()
            }));
        }
    }
    let records = crate::quota_store::load(&crate::quota_store::store_path())?;
    let mut routes = Vec::new();
    for identity in identities {
        let capacity = capacity(&identity, &records, now);
        if !capacity.exhausted
            && crate::availability::availability_for_identity(
                &crate::availability::resolve_state_path(),
                &identity,
                now,
            )?
            .eligible
        {
            routes.push(HandoffRoute { identity, capacity });
        }
    }
    routes.sort_by(|left, right| {
        compare(
            &left.identity,
            &left.capacity,
            &right.identity,
            &right.capacity,
        )
        .then_with(|| {
            left.identity
                .backend_instance
                .cmp(&right.identity.backend_instance)
        })
    });
    Ok(routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_account_checks_and_retired_credentials_remove_capacity() {
        let now = OffsetDateTime::parse("2026-10-03T18:00:00Z", &Rfc3339).unwrap();
        let mut identity =
            ExecutionIdentity::legacy_candidate("claude", None::<String>, None::<String>);
        identity.credential_id = Some("claude-one".into());
        let reading: QuotaObservationRecord = serde_json::from_value(serde_json::json!({
            "backend":"claude", "credential_id":"claude-one", "quota_window":"weekly",
            "quota_remaining_percent":80, "observed_at":"2026-10-03T17:59:00Z",
            "quota_reset_at":"2026-10-03T18:10:00Z"
        }))
        .unwrap();
        let failed: QuotaObservationRecord = serde_json::from_value(serde_json::json!({
            "backend":"claude", "credential_id":"claude-one", "checked_at":"2026-10-03T18:00:00Z",
            "check_error":"network"
        }))
        .unwrap();
        assert!(capacity(&identity, std::slice::from_ref(&reading), now).known_capacity);
        let invalidated = capacity(&identity, &[reading.clone(), failed], now);
        assert!(!invalidated.known_capacity);
        assert_eq!(invalidated.reset_pressure, None);
        let retired: QuotaObservationRecord = serde_json::from_value(serde_json::json!({
            "backend":"claude", "credential_id":"claude-one", "usage_source":"credential_removed"
        }))
        .unwrap();
        assert!(!capacity(&identity, &[reading, retired], now).known_capacity);
    }

    #[test]
    fn subscription_pressure_uses_explicit_budget_windows() {
        let now = OffsetDateTime::parse("2026-10-03T18:00:00Z", &Rfc3339).unwrap();
        let identity =
            ExecutionIdentity::legacy_candidate("claude", None::<String>, None::<String>);
        for (window, expected) in [
            ("seven_day", Some(504.0)),
            ("weekly", Some(504.0)),
            ("vibe-code-included-monthly", Some(2160.0)),
            ("five_hour", None),
            ("session", None),
            ("unknown_weekly_budget", None),
        ] {
            let reading = serde_json::from_value(serde_json::json!({
                "backend":"claude", "quota_window":window, "quota_remaining_percent":50,
                "observed_at":"2026-10-03T17:59:00Z", "quota_reset_at":"2026-10-03T18:10:00Z"
            }))
            .unwrap();
            let result = capacity(&identity, &[reading], now);
            assert!(result.known_capacity);
            assert_eq!(result.reset_pressure, expected, "{window}");
        }
    }

    #[test]
    fn account_windows_freshness_and_subscription_preference() {
        let now = OffsetDateTime::parse("2026-10-02T18:00:00Z", &Rfc3339).unwrap();
        let mut agy =
            ExecutionIdentity::legacy_candidate("agy", None::<String>, Some("primary:external"));
        agy.explicit_instance = true;
        agy.backend_instance = "agy-one".into();
        let mut record: QuotaObservationRecord = serde_json::from_value(serde_json::json!({ "backend":"agy", "backend_instance":"agy-one", "quota_pool":"primary:external", "quota_window":"weekly", "quota_remaining_percent":80, "observed_at":"2026-10-02T17:59:00Z", "quota_reset_at":"2026-10-09T18:00:00Z" })).unwrap();
        assert!(capacity(&agy, &[record.clone()], now).known_capacity);
        let mut sibling = agy.clone();
        sibling.backend_instance = "agy-two".into();
        assert!(!capacity(&sibling, &[record.clone()], now).known_capacity);
        record.quota_remaining_percent = Some(0.0);
        assert!(capacity(&agy, &[record.clone()], now).exhausted);
        record.quota_window = Some("five-hour".into());
        record.quota_remaining_percent = Some(90.0);
        record.observed_at = Some("2026-10-02T16:00:00Z".into());
        assert!(!capacity(&agy, &[record.clone()], now).known_capacity);
        record.observed_at = Some("2026-10-02T19:00:00Z".into());
        assert!(!capacity(&agy, &[record.clone()], now).known_capacity);
        record.observed_at = Some("2026-10-02T17:59:00Z".into());
        record.check_error = Some("network".into());
        assert!(!capacity(&agy, &[record.clone()], now).known_capacity);
        record.check_error = None;
        record.quota_reset_at = Some("2026-10-02T17:00:00Z".into());
        assert!(!capacity(&agy, &[record], now).known_capacity);
        let claude = ExecutionIdentity::legacy_candidate("claude", None::<String>, None::<String>);
        let hermes = ExecutionIdentity::legacy_candidate("hermes", None::<String>, None::<String>);
        let vibe = ExecutionIdentity::legacy_candidate("vibe", None::<String>, None::<String>);
        let unknown = SubscriptionCapacity::default();
        let known = SubscriptionCapacity {
            known_capacity: true,
            ..Default::default()
        };
        assert_eq!(compare(&agy, &unknown, &claude, &unknown), Ordering::Less);
        assert_eq!(compare(&agy, &unknown, &claude, &known), Ordering::Less);
        assert_eq!(compare(&hermes, &unknown, &vibe, &unknown), Ordering::Less);
        let urgent = SubscriptionCapacity {
            reset_pressure: Some(2.0),
            ..Default::default()
        };
        assert_eq!(compare(&vibe, &urgent, &hermes, &unknown), Ordering::Less);
    }
}
