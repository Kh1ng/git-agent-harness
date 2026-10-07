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
    /// #1336: the source's latest check says it needs a login. The
    /// provider's redacted remediation rides `error`, and `failing_since`
    /// dates the current streak so repeat markers stay one condition.
    AuthRequired,
    /// #1336: a configured candidate expects this allowance source, but the
    /// node holds no credential for it, so no check can ever run here.
    NotConfigured,
}

/// #1336: a quota check error class meaning "the provider needs a login or
/// key before this source can report anything".
pub(crate) fn is_auth_required(error: Option<&str>) -> bool {
    error.is_some_and(|error| error.starts_with("auth_required:"))
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
    /// When the check last ran. Absent only for a `not_configured` source,
    /// which has never been checked on this node (#1336).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    pub status: QuotaCheckStatus,
    /// Account readings remain visible even when no routing candidate uses them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quota_observations: Vec<crate::quota_store::QuotaObservationRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// #1336: start of the current run of consecutive `auth_required`
    /// failures. Every refresh tick re-marks the same expired login; this
    /// timestamp keeps the whole run one "how long has it been failing".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failing_since: Option<String>,
}

/// #1336: node-local credentials behind gated allowance sources. `false`
/// means no check for that source can ever run on this node, so a candidate
/// expecting it must be shown "not configured", not absent.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SourceConfiguration {
    /// `MISTRAL_ADMIN_API_KEY` is present (Vibe allowance source).
    pub vibe_admin: bool,
    /// A Mistral dashboard cookie is configured (dashboard allowance source).
    pub mistral_dashboard: bool,
    /// Nous Portal access exists: `NOUS_API_KEY` or a Hermes sign-in.
    pub nous_portal: bool,
}

/// #1336: an allowance source a configured `included_in_quota` candidate
/// expects readings from. Sources whose credentials are absent on this node
/// are never refreshed and never recorded, so without this expectation they
/// would silently vanish from the snapshot instead of reporting that they
/// are not configured here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedQuotaSource {
    pub backend: String,
    pub credential_id: Option<String>,
    /// Required instance label; `None` accepts any recorded instance.
    pub backend_instance: Option<String>,
    /// Billing/subscription service behind the source, when known.
    pub provider: Option<String>,
    /// Fixed, secret-free remediation shown with "not configured".
    pub detail: String,
}

impl ExpectedQuotaSource {
    /// Whether a check row proves this expected source exists on the node.
    fn satisfied_by(&self, check: &QuotaCheck) -> bool {
        check.credential_id == self.credential_id
            && check.backend == self.backend
            && self
                .backend_instance
                .as_deref()
                .is_none_or(|instance| check.backend_instance.as_deref() == Some(instance))
    }

    fn not_configured_check(&self) -> QuotaCheck {
        QuotaCheck {
            backend: self.backend.clone(),
            credential_id: self.credential_id.clone(),
            provider: self.provider.clone(),
            backend_instance: self.backend_instance.clone(),
            model: None,
            quota_pool: None,
            checked_at: None,
            status: QuotaCheckStatus::NotConfigured,
            quota_observations: Vec::new(),
            error: Some(self.detail.clone()),
            failing_since: None,
        }
    }
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

pub(super) fn build_quota_checks(
    records: &[QuotaObservationRecord],
    expected: &[ExpectedQuotaSource],
) -> Vec<QuotaCheck> {
    type SourceKey = (
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let current = crate::quota_store::current_source_records(records);
    let mut latest = BTreeMap::new();
    // #1336: every check for one source, so the current auth_required run
    // can be dated from its first marker instead of its latest one.
    let mut history: BTreeMap<SourceKey, Vec<(OffsetDateTime, String, &QuotaObservationRecord)>> =
        BTreeMap::new();
    for record in current {
        if record.backend == "vibe"
            && record.credential_id.is_none()
            && record.account_usage.is_none()
            && record.quota_window.is_none()
            && record.quota_remaining_percent.is_none()
            && record.quota_reset_at.is_none()
            && record.check_error.as_deref()
                == Some("auth_required: MISTRAL_ADMIN_API_KEY is not configured")
        {
            continue;
        }
        // A successful Antigravity probe marker only throttles the probe; its
        // readings are listed under their pools. A failed one stays visible.
        if record
            .backend_instance
            .as_deref()
            .is_some_and(|instance| instance.ends_with(":usage-probe"))
            && record.check_error.is_none()
        {
            continue;
        }
        let Some(checked_at) = check_timestamp(record) else {
            continue;
        };
        let Ok(parsed) = OffsetDateTime::parse(&checked_at, &Rfc3339) else {
            continue;
        };
        let key: SourceKey = (
            record.credential_id.clone(),
            record.backend.clone(),
            record.backend_instance.clone(),
            record.model.clone(),
            record.quota_pool.clone(),
        );
        history
            .entry(key.clone())
            .or_default()
            .push((parsed, checked_at.clone(), record));
        if latest
            .get(&key)
            .is_none_or(|(_, _, current)| parsed >= *current)
        {
            latest.insert(key, (record, checked_at, parsed));
        }
    }
    let mut checks: Vec<QuotaCheck> = latest
        .into_iter()
        .map(|(key, (record, checked_at, _))| {
            let (credential_id, backend, backend_instance, model, quota_pool) = key;
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
            let quota_observations = super::aggregate_observations(None, None, &scoped, &identity);
            let has_data = record.quota_window.is_some()
                || record.quota_remaining_percent.is_some()
                || record.quota_reset_at.is_some()
                || record.account_usage.is_some();
            let status = if is_auth_required(record.check_error.as_deref()) {
                QuotaCheckStatus::AuthRequired
            } else if record.check_error.is_some() {
                QuotaCheckStatus::Failed
            } else if has_data {
                QuotaCheckStatus::Data
            } else {
                QuotaCheckStatus::NoData
            };
            let failing_since = if status == QuotaCheckStatus::AuthRequired {
                auth_required_since(
                    history
                        .get(&(
                            credential_id.clone(),
                            backend.clone(),
                            backend_instance.clone(),
                            model.clone(),
                            quota_pool.clone(),
                        ))
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                )
            } else {
                None
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
                checked_at: Some(checked_at),
                status,
                quota_observations,
                error: record.check_error.as_deref().map(crate::redact::redact),
                failing_since,
            }
        })
        .collect();
    // #1336: a configured candidate can expect an allowance source this
    // node has no credential for; nothing ever checks it, so surface it as
    // not configured rather than letting it disappear.
    for source in expected {
        if checks.iter().any(|check| source.satisfied_by(check)) {
            continue;
        }
        checks.push(source.not_configured_check());
    }
    checks.sort_by(|left, right| {
        left.credential_id
            .cmp(&right.credential_id)
            .then_with(|| left.backend.cmp(&right.backend))
            .then_with(|| left.backend_instance.cmp(&right.backend_instance))
            .then_with(|| left.model.cmp(&right.model))
            .then_with(|| left.quota_pool.cmp(&right.quota_pool))
    });
    checks
}

/// #1336: earliest check in the current run of consecutive `auth_required`
/// failures for one source, so "needs login" reports how long the login has
/// actually been failing instead of when the latest marker landed.
fn auth_required_since(
    history: &[(OffsetDateTime, String, &QuotaObservationRecord)],
) -> Option<String> {
    let mut sorted = history.to_vec();
    sorted.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.checked_at.cmp(&right.2.checked_at))
    });
    sorted
        .iter()
        .rev()
        .take_while(|(_, _, record)| is_auth_required(record.check_error.as_deref()))
        .last()
        .map(|(_, checked_at, _)| checked_at.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota_snapshot::{QuotaCandidateStatus, UsageSummary};

    fn expected_vibe_admin_source() -> ExpectedQuotaSource {
        ExpectedQuotaSource {
            backend: "vibe".into(),
            credential_id: None,
            backend_instance: None,
            provider: Some("mistral".into()),
            detail: "no Mistral Admin API allowance source on this node".into(),
        }
    }

    #[test]
    fn an_unconfigured_admin_auth_method_does_not_poison_the_connected_dashboard() {
        let now = Some("2026-10-03T04:00:00Z");
        let mut dashboard = record("mistral-dashboard", now, now, QuotaCheckStatus::Data);
        dashboard.backend_instance = Some("mistral-dashboard:verified".into());
        dashboard.usage_source = Some("mistral_dashboard".into());
        let mut admin = record("vibe", None, now, QuotaCheckStatus::Failed);
        admin.check_error = Some("auth_required: MISTRAL_ADMIN_API_KEY is not configured".into());
        let checks = build_quota_checks(&[dashboard.clone(), admin.clone()], &[]);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].backend, "mistral-dashboard");
        admin.credential_id = Some("named-admin".into());
        assert_eq!(build_quota_checks(&[dashboard, admin], &[]).len(), 2);
    }

    /// #1336: the store may still hold the pre-fix Vibe markers; with a
    /// configured candidate expecting the source, the snapshot must show it
    /// as not configured instead of dropping it silently.
    #[test]
    fn an_expected_source_with_only_stale_unconfigured_markers_shows_not_configured() {
        let now = Some("2026-10-03T04:00:00Z");
        let mut dashboard = record("mistral-dashboard", now, now, QuotaCheckStatus::Data);
        dashboard.backend_instance = Some("mistral-dashboard:verified".into());
        dashboard.usage_source = Some("mistral_dashboard".into());
        let mut marker = record("vibe", None, None, QuotaCheckStatus::Failed);
        marker.checked_at = Some("2026-10-01T04:00:00Z".into());
        marker.check_error = Some("auth_required: MISTRAL_ADMIN_API_KEY is not configured".into());

        let checks = build_quota_checks(&[dashboard, marker], &[expected_vibe_admin_source()]);
        let vibe = checks
            .iter()
            .find(|check| check.backend == "vibe")
            .expect("expected source stays visible");
        assert_eq!(vibe.status, QuotaCheckStatus::NotConfigured);
        assert_eq!(vibe.checked_at, None);
        assert_eq!(vibe.failing_since, None);
        assert_eq!(
            vibe.error.as_deref(),
            Some("no Mistral Admin API allowance source on this node")
        );
        assert_eq!(vibe.provider.as_deref(), Some("mistral"));
    }

    /// #1336: a source that does report must not be duplicated by the
    /// expectation, and a satisfied expectation produces no extra row.
    #[test]
    fn an_expected_source_with_records_is_not_duplicated() {
        let mut reading = record("vibe", None, None, QuotaCheckStatus::Data);
        reading.checked_at = Some("2026-10-03T04:00:00Z".into());
        let checks = build_quota_checks(&[reading], &[expected_vibe_admin_source()]);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, QuotaCheckStatus::Data);
    }

    /// #1336 acceptance: an auth_required latest check surfaces as
    /// "needs login" with the provider's remediation text and the start of
    /// the failing streak, not the latest 30-minute marker.
    #[test]
    fn auth_required_check_keeps_remediation_and_streak_start() {
        let mut ok = record("claude", None, None, QuotaCheckStatus::Data);
        ok.backend_instance = Some("claude".into());
        ok.checked_at = Some("2026-10-01T08:00:00Z".into());
        ok.observed_at = Some("2026-10-01T08:00:00Z".into());
        let mut first = auth_marker("claude", "2026-10-02T08:00:00Z");
        let mut latest = auth_marker("claude", "2026-10-02T08:30:00Z");
        for record in [&mut first, &mut latest] {
            record.backend_instance = Some("claude".into());
        }
        let records = vec![ok, first, latest];

        let checks = build_quota_checks(&records, &[]);
        assert_eq!(checks.len(), 1);
        let check = &checks[0];
        assert_eq!(check.status, QuotaCheckStatus::AuthRequired);
        assert_eq!(
            check.error.as_deref(),
            Some("auth_required: Claude OAuth login expired; run claude auth login")
        );
        assert_eq!(check.checked_at.as_deref(), Some("2026-10-02T08:30:00Z"));
        assert_eq!(check.failing_since.as_deref(), Some("2026-10-02T08:00:00Z"));
    }

    /// #1336: the streak restarts after a successful check and after a
    /// non-auth failure, so a fresh expiry reads as a fresh condition.
    #[test]
    fn failing_streak_restarts_after_success_and_after_other_failures() {
        let mut success = record("claude", None, None, QuotaCheckStatus::Data);
        success.checked_at = Some("2026-10-01T08:00:00Z".into());
        success.observed_at = Some("2026-10-01T08:00:00Z".into());
        let first_run = auth_marker("claude", "2026-10-02T08:00:00Z");
        let mut restored = record("claude", None, None, QuotaCheckStatus::Data);
        restored.checked_at = Some("2026-10-02T09:00:00Z".into());
        restored.observed_at = Some("2026-10-02T09:00:00Z".into());
        let second_run = auth_marker("claude", "2026-10-03T08:00:00Z");
        let mut transient = record("claude", None, None, QuotaCheckStatus::Failed);
        transient.checked_at = Some("2026-10-03T08:30:00Z".into());
        transient.check_error = Some("network unreachable".into());
        let third_run = auth_marker("claude", "2026-10-03T09:00:00Z");

        let checks = build_quota_checks(
            &[
                success, first_run, restored, second_run, transient, third_run,
            ],
            &[],
        );
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, QuotaCheckStatus::AuthRequired);
        assert_eq!(
            checks[0].failing_since.as_deref(),
            Some("2026-10-03T09:00:00Z")
        );
    }

    /// #1336 acceptance: a configured-but-missing source appears as not
    /// configured when the node never wrote any record for it.
    #[test]
    fn a_configured_but_missing_source_appears_not_configured() {
        let checks = build_quota_checks(&[], &[expected_vibe_admin_source()]);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, QuotaCheckStatus::NotConfigured);
        assert_eq!(checks[0].backend, "vibe");
        assert_eq!(checks[0].checked_at, None);
        // Unknown stays unknown: no check ever ran, so no timestamp exists.
        assert_eq!(checks[0].failing_since, None);
    }

    /// A named sibling source must not satisfy an ambient expectation.
    #[test]
    fn an_expected_ambient_source_is_not_satisfied_by_named_records() {
        let mut named = record("vibe", None, None, QuotaCheckStatus::Data);
        named.checked_at = Some("2026-10-03T04:00:00Z".into());
        named.credential_id = Some("named-admin".into());
        let checks = build_quota_checks(&[named], &[expected_vibe_admin_source()]);
        let statuses: Vec<_> = checks
            .iter()
            .map(|check| (check.backend.as_str(), check.status))
            .collect();
        assert!(statuses.contains(&("vibe", QuotaCheckStatus::Data)));
        assert!(statuses.contains(&("vibe", QuotaCheckStatus::NotConfigured)));
    }

    #[test]
    fn admin_refresh_without_spend_limit_has_no_data() {
        let mut refresh = record(
            "vibe",
            Some("2026-10-03T04:00:00Z"),
            Some("2026-10-03T04:00:00Z"),
            QuotaCheckStatus::NoData,
        );
        refresh.usage_source = Some("mistral_admin_refresh".into());
        let checks = build_quota_checks(&[refresh.clone()], &[]);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, QuotaCheckStatus::NoData);

        refresh.check_error = Some("spend limit unavailable".into());
        let checks = build_quota_checks(&[refresh], &[]);
        assert_eq!(checks[0].status, QuotaCheckStatus::Failed);
    }

    fn auth_marker(backend: &str, checked_at: &str) -> QuotaObservationRecord {
        let mut record = record(backend, None, None, QuotaCheckStatus::Failed);
        record.checked_at = Some(checked_at.into());
        record.check_error =
            Some("auth_required: Claude OAuth login expired; run claude auth login".into());
        record
    }

    #[test]
    fn only_failed_agy_probe_markers_are_listed() {
        let now = Some("2026-10-03T04:00:00Z");
        let mut ok = record("agy", None, now, QuotaCheckStatus::NoData);
        ok.backend_instance = Some("agy:usage-probe".into());
        assert!(build_quota_checks(&[ok], &[]).is_empty());
        let mut failed = record("agy", None, now, QuotaCheckStatus::Failed);
        failed.backend_instance = Some("agy:usage-probe".into());
        assert_eq!(build_quota_checks(&[failed], &[]).len(), 1);
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
                usage_source: None,
                account_usage: None,
                credential_id: None,
            }],
        }];

        let freshness = build_freshness(None, &candidates, &account_quota);
        let checks = build_quota_checks(&account_quota, &[]);

        assert_eq!(
            freshness.quota_checked_at.as_deref(),
            Some("2026-08-29T08:25:01Z")
        );
        assert_eq!(
            freshness.quota_observed_at.as_deref(),
            Some("2026-08-22T19:47:14Z")
        );
        assert_eq!(checks[0].status, QuotaCheckStatus::NoData);
        assert_eq!(
            checks[0].checked_at.as_deref(),
            Some("2026-08-29T08:25:01Z")
        );
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

        let checks = build_quota_checks(&records, &[]);
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
        let checks = build_quota_checks(&[first, second], &[]);
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
        let checks = build_quota_checks(&records, &[]);
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
        let checks = build_quota_checks(&records, &[]);
        let nous = checks
            .iter()
            .find(|check| check.provider.as_deref() == Some("nous"))
            .unwrap();
        assert_eq!(nous.status, QuotaCheckStatus::Failed);
        assert!(nous.quota_observations.is_empty());
    }
}
