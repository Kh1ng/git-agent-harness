use super::{latest_timestamp, QuotaCandidateStatus, QuotaFreshness};
use crate::quota_store::QuotaObservationRecord;
use serde::Serialize;
use std::collections::BTreeMap;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaCheckStatus {
    Data,
    NoData,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaCheck {
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    pub checked_at: String,
    pub status: QuotaCheckStatus,
    /// Account readings remain visible even when no routing candidate uses them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quota_observations: Vec<crate::quota_store::QuotaObservationRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(super) fn build_freshness(
    ledger_observed_at: Option<String>,
    candidates: &[QuotaCandidateStatus],
    account_quota: &[QuotaObservationRecord],
) -> QuotaFreshness {
    QuotaFreshness {
        ledger_observed_at,
        availability_observed_at: latest_timestamp(
            candidates
                .iter()
                .filter_map(|candidate| candidate.observed_at.clone()),
        ),
        // The account store is global across profiles. Every row proves a
        // backend check happened even when it carries no parseable quota
        // data, which is intentionally distinct from quota_observed_at.
        quota_checked_at: latest_timestamp(account_quota.iter().filter_map(check_timestamp)),
        quota_observed_at: latest_timestamp(
            candidates
                .iter()
                .flat_map(|candidate| candidate.quota_observations.iter())
                .filter_map(|observation| observation.observed_at.clone()),
        ),
    }
}

fn check_timestamp(record: &QuotaObservationRecord) -> Option<String> {
    record
        .checked_at
        .clone()
        .or_else(|| record.observed_at.clone())
}

pub(super) fn build_quota_checks(records: &[QuotaObservationRecord]) -> Vec<QuotaCheck> {
    let mut latest = BTreeMap::new();
    for record in crate::quota_store::current_source_records(records) {
        if record.backend == "vibe"
            && record.credential_id.is_none()
            && record.account_usage.is_none()
            && record.mistral_admin.is_none()
            && record.quota_window.is_none()
            && record.quota_remaining_percent.is_none()
            && record.quota_reset_at.is_none()
            && record.check_error.as_deref()
                == Some("auth_required: MISTRAL_ADMIN_API_KEY is not configured")
        {
            continue;
        }
        let Some(checked_at) = check_timestamp(record) else {
            continue;
        };
        let Ok(parsed) = OffsetDateTime::parse(&checked_at, &Rfc3339) else {
            continue;
        };
        let key = (
            record.credential_id.clone(),
            record.backend.clone(),
            record.backend_instance.clone(),
            record.model.clone(),
            record.quota_pool.clone(),
        );
        if latest
            .get(&key)
            .is_none_or(|(_, _, current)| parsed >= *current)
        {
            latest.insert(key, (record, checked_at, parsed));
        }
    }
    latest
        .into_iter()
        .map(
            |(
                (credential_id, backend, backend_instance, model, quota_pool),
                (record, checked_at, _),
            )| {
                let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
                    &backend,
                    model.as_deref(),
                    quota_pool.as_deref(),
                );
                if let Some(instance) = &backend_instance {
                    identity.backend_instance = instance.clone();
                }
                // Exact scope prevents a broad legacy reading from being
                // presented as a verified balance for a named account.
                let scoped: Vec<_> = records
                    .iter()
                    .filter(|reading| {
                        reading.backend == backend
                            && reading.credential_id == credential_id
                            && reading.backend_instance == backend_instance
                            && reading.model == model
                            && reading.quota_pool == quota_pool
                    })
                    .cloned()
                    .collect();
                let quota_observations =
                    super::aggregate_observations(None, None, &scoped, &identity);
                let has_data = record.quota_window.is_some()
                    || record.quota_remaining_percent.is_some()
                    || record.quota_reset_at.is_some()
                    || record.mistral_admin.is_some()
                    || record.account_usage.is_some();
                let status = if record.check_error.is_some() {
                    QuotaCheckStatus::Failed
                } else if has_data {
                    QuotaCheckStatus::Data
                } else {
                    QuotaCheckStatus::NoData
                };
                QuotaCheck {
                    credential_id,
                    provider: if let Some(provider) = record
                        .usage_source
                        .as_deref()
                        .and_then(|source| source.strip_prefix("credential_api:"))
                    {
                        Some(provider.into())
                    } else if record.usage_source.as_deref() == Some("nous_portal_account") {
                        Some("nous".into())
                    } else {
                        super::quota_provider(&backend, model.as_deref())
                    },
                    backend,
                    backend_instance,
                    model,
                    quota_pool,
                    checked_at,
                    status,
                    quota_observations,
                    error: record.check_error.as_deref().map(crate::redact::redact),
                }
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota_snapshot::{QuotaCandidateStatus, UsageSummary};
    #[test]
    fn an_unconfigured_admin_auth_method_does_not_poison_the_connected_dashboard() {
        let now = Some("2026-10-03T04:00:00Z");
        let mut dashboard = record("mistral-dashboard", now, now, QuotaCheckStatus::Data);
        dashboard.backend_instance = Some("mistral-dashboard:verified".into());
        dashboard.usage_source = Some("mistral_dashboard".into());
        let mut admin = record("vibe", None, now, QuotaCheckStatus::Failed);
        admin.check_error = Some("auth_required: MISTRAL_ADMIN_API_KEY is not configured".into());
        let checks = build_quota_checks(&[dashboard.clone(), admin.clone()]);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].backend, "mistral-dashboard");
        admin.credential_id = Some("named-admin".into());
        assert_eq!(build_quota_checks(&[dashboard, admin]).len(), 2);
    }

    fn record(
        backend: &str,
        observed_at: Option<&str>,
        checked_at: Option<&str>,
        status: QuotaCheckStatus,
    ) -> QuotaObservationRecord {
        QuotaObservationRecord {
            backend: backend.to_string(),
            backend_instance: None,
            model: None,
            quota_pool: None,
            quota_window: (status == QuotaCheckStatus::Data).then(|| "weekly".to_string()),
            quota_remaining_percent: None,
            quota_reset_at: None,
            observed_at: observed_at.map(str::to_string),
            checked_at: checked_at.map(str::to_string),
            check_error: (status == QuotaCheckStatus::Failed)
                .then(|| format!("{backend} check failed")),
            usage_source: None,
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        }
    }

    #[test]
    fn recent_legacy_no_data_check_is_distinct_from_last_data_observation() {
        let account_quota = vec![record(
            "codex",
            Some("2026-08-29T08:25:01Z"),
            None,
            QuotaCheckStatus::NoData,
        )];
        let candidates = vec![QuotaCandidateStatus {
            provider: Some("openai".to_string()),
            modes: vec!["default".to_string()],
            backend: "codex".to_string(),
            backend_instance: None,
            model: None,
            quota_pool: None,
            configured: true,
            eligible_now: true,
            reason: None,
            unavailable_until: None,
            source: None,
            last_error_summary: None,
            observed_at: None,
            usage: UsageSummary::default(),
            quota_observations: vec![QuotaObservationRecord {
                backend: "codex".to_string(),
                backend_instance: None,
                model: None,
                quota_pool: None,
                quota_window: Some("weekly".to_string()),
                quota_remaining_percent: Some(42.0),
                quota_reset_at: None,
                observed_at: Some("2026-08-22T19:47:14Z".to_string()),
                checked_at: None,
                check_error: None,
                mistral_admin: None,
                usage_source: None,
                account_usage: None,
                credential_id: None,
            }],
        }];

        let freshness = build_freshness(None, &candidates, &account_quota);
        let checks = build_quota_checks(&account_quota);

        assert_eq!(
            freshness.quota_checked_at.as_deref(),
            Some("2026-08-29T08:25:01Z")
        );
        assert_eq!(
            freshness.quota_observed_at.as_deref(),
            Some("2026-08-22T19:47:14Z")
        );
        assert_eq!(checks[0].status, QuotaCheckStatus::NoData);
        assert_eq!(checks[0].checked_at, "2026-08-29T08:25:01Z");
    }

    #[test]
    fn latest_check_per_backend_exposes_data_no_data_and_failure() {
        let records = vec![
            record(
                "codex",
                Some("2026-08-29T08:00:00Z"),
                Some("2026-08-29T08:00:00Z"),
                QuotaCheckStatus::Data,
            ),
            record(
                "codex",
                None,
                Some("2026-08-29T08:25:00Z"),
                QuotaCheckStatus::Failed,
            ),
            record(
                "vibe",
                None,
                Some("2026-08-29T08:25:01Z"),
                QuotaCheckStatus::NoData,
            ),
            record(
                "agy",
                Some("2026-08-29T08:24:00Z"),
                Some("2026-08-29T08:24:00Z"),
                QuotaCheckStatus::Data,
            ),
        ];

        let checks = build_quota_checks(&records);
        let status = |backend| {
            checks
                .iter()
                .find(|check| check.backend == backend)
                .map(|check| check.status)
        };

        assert_eq!(checks.len(), 3);
        assert_eq!(status("agy"), Some(QuotaCheckStatus::Data));
        assert_eq!(status("codex"), Some(QuotaCheckStatus::Failed));
        assert_eq!(status("vibe"), Some(QuotaCheckStatus::NoData));
    }

    #[test]
    fn sibling_accounts_keep_independent_check_health() {
        let mut first = record(
            "vibe",
            Some("2026-10-02T23:00:00Z"),
            Some("2026-10-02T23:00:00Z"),
            QuotaCheckStatus::Data,
        );
        first.backend_instance = Some("vibe-1".into());
        first.quota_pool = Some("vibe-1-monthly".into());
        let mut second = record(
            "vibe",
            None,
            Some("2026-10-02T23:01:00Z"),
            QuotaCheckStatus::Failed,
        );
        second.backend_instance = Some("vibe-2".into());
        second.quota_pool = Some("vibe-2-monthly".into());
        let checks = build_quota_checks(&[first, second]);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].backend_instance.as_deref(), Some("vibe-1"));
        assert_eq!(checks[0].status, QuotaCheckStatus::Data);
        assert_eq!(checks[1].status, QuotaCheckStatus::Failed);
        assert_eq!(checks[0].provider.as_deref(), Some("mistral"));
    }

    #[test]
    fn check_only_account_windows_are_exactly_scoped_and_invalidated() {
        let mut first = record(
            "opencode",
            Some("2026-10-02T23:00:00Z"),
            Some("2026-10-02T23:00:00Z"),
            QuotaCheckStatus::Data,
        );
        first.backend_instance = Some("opencode:nous-portal-api".into());
        first.quota_pool = Some("nous-portal-api".into());
        first.quota_window = Some("subscription-monthly".into());
        first.quota_remaining_percent = Some(0.0);
        first.usage_source = Some("nous_portal_account".into());
        let broad = record(
            "opencode",
            Some("2026-10-02T23:01:00Z"),
            Some("2026-10-02T23:01:00Z"),
            QuotaCheckStatus::Data,
        );
        let mut records = vec![first.clone(), broad];
        let checks = build_quota_checks(&records);
        let nous = checks
            .iter()
            .find(|check| check.provider.as_deref() == Some("nous"))
            .unwrap();
        assert_eq!(nous.quota_observations.len(), 1);
        assert_eq!(
            nous.quota_observations[0].quota_remaining_percent,
            Some(0.0)
        );
        let mut failure = first;
        failure.checked_at = Some("2026-10-02T23:02:00Z".into());
        failure.observed_at = None;
        failure.quota_window = None;
        failure.quota_remaining_percent = None;
        failure.check_error = Some("credential expired".into());
        records.push(failure);
        let checks = build_quota_checks(&records);
        let nous = checks
            .iter()
            .find(|check| check.provider.as_deref() == Some("nous"))
            .unwrap();
        assert_eq!(nous.status, QuotaCheckStatus::Failed);
        assert!(nous.quota_observations.is_empty());
    }
}
