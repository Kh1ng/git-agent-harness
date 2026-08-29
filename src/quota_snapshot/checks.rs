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
    pub checked_at: String,
    pub status: QuotaCheckStatus,
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
    let mut latest = BTreeMap::<String, (&QuotaObservationRecord, String, OffsetDateTime)>::new();
    for record in records {
        let Some(checked_at) = check_timestamp(record) else {
            continue;
        };
        let Ok(parsed) = OffsetDateTime::parse(&checked_at, &Rfc3339) else {
            continue;
        };
        if latest
            .get(&record.backend)
            .is_none_or(|(_, _, current)| parsed >= *current)
        {
            latest.insert(record.backend.clone(), (record, checked_at, parsed));
        }
    }
    latest
        .into_iter()
        .map(|(backend, (record, checked_at, _))| {
            let has_data = record.quota_window.is_some()
                || record.quota_used_percent.is_some()
                || record.quota_remaining_percent.is_some()
                || record.quota_reset_at.is_some()
                || record.mistral_admin.is_some();
            let status = if record.check_error.is_some() {
                QuotaCheckStatus::Failed
            } else if has_data {
                QuotaCheckStatus::Data
            } else {
                QuotaCheckStatus::NoData
            };
            QuotaCheck {
                backend,
                checked_at,
                status,
                error: record.check_error.as_deref().map(crate::redact::redact),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota_snapshot::{QuotaCandidateStatus, QuotaObservation, UsageSummary};

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
            quota_used_percent: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            observed_at: observed_at.map(str::to_string),
            checked_at: checked_at.map(str::to_string),
            check_error: (status == QuotaCheckStatus::Failed)
                .then(|| format!("{backend} check failed")),
            usage_source: None,
            mistral_admin: None,
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
            quota_observations: vec![QuotaObservation {
                backend: "codex".to_string(),
                backend_instance: None,
                model: None,
                quota_pool: None,
                quota_window: Some("weekly".to_string()),
                quota_used_percent: None,
                quota_remaining_percent: Some(42.0),
                quota_reset_at: None,
                observed_at: Some("2026-08-22T19:47:14Z".to_string()),
                usage_source: None,
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
}
