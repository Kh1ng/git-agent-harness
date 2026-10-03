//! Select account windows without crossing a named credential or retired source.
use super::{has_quota_data, QuotaObservationRecord};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

fn observation_matches_identity(
    record: &QuotaObservationRecord,
    identity: &crate::execution_identity::ExecutionIdentity,
) -> bool {
    record.backend == identity.logical_backend
        && (record.model.is_none() || record.model == identity.effective_model)
        && record
            .backend_instance
            .as_deref()
            .is_none_or(|instance| instance == identity.backend_instance)
        && record
            .quota_pool
            .as_deref()
            .is_none_or(|pool| Some(pool) == identity.quota_pool.as_deref())
}

/// Latest reading for each limit window belonging to this execution identity.
/// A five-hour refresh must not replace an independent weekly balance.
pub fn latest_windows_for_identity<'a>(
    records: &'a [QuotaObservationRecord],
    identity: &crate::execution_identity::ExecutionIdentity,
) -> Vec<&'a QuotaObservationRecord> {
    latest_windows_for_identity_and_credential(records, identity, identity.credential_id.as_deref())
}

/// An explicit credential binding cannot inherit ambient or sibling source
/// readings. A shared verified billing pool remains visible at account scope.
pub fn latest_windows_for_identity_and_credential<'a>(
    records: &'a [QuotaObservationRecord],
    identity: &crate::execution_identity::ExecutionIdentity,
    credential_id: Option<&str>,
) -> Vec<&'a QuotaObservationRecord> {
    let matching: Vec<_> = current_source_records(records)
        .into_iter()
        .filter(|record| match credential_id {
            Some(id) => record.credential_id.as_deref() == Some(id),
            None => observation_matches_identity(record, identity),
        })
        .collect();
    let timestamp = |record: &QuotaObservationRecord| {
        record
            .checked_at
            .as_deref()
            .or(record.observed_at.as_deref())
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
    };
    let mut windows = std::collections::BTreeMap::<_, &QuotaObservationRecord>::new();
    let mut invalidated = std::collections::BTreeMap::<_, OffsetDateTime>::new();
    for check in matching
        .iter()
        .filter(|check| check.check_error.is_some() || !has_quota_data(check))
    {
        if let Some(checked) = timestamp(check) {
            let current = invalidated
                .entry((&check.credential_id, &check.quota_window))
                .or_insert(checked);
            *current = (*current).max(checked);
        }
    }
    for record in matching
        .iter()
        .copied()
        .filter(|record| has_quota_data(record) && record.check_error.is_none())
    {
        // A failed or empty check invalidates earlier data for that window.
        // An account-wide check with no window invalidates all its windows.
        if [
            invalidated.get(&(&record.credential_id, &None)),
            invalidated.get(&(&record.credential_id, &record.quota_window)),
        ]
        .into_iter()
        .flatten()
        .any(|checked| timestamp(record).is_none_or(|observed| *checked >= observed))
        {
            continue;
        }
        let key = (&record.model, &record.quota_window);
        if windows
            .get(&key)
            .is_none_or(|current| timestamp(record) >= timestamp(current))
        {
            windows.insert(key, record);
        }
    }
    windows.into_values().collect()
}

/// Rotating a named source to a different verified account must not leave its
/// former account reading active. Unnamed legacy observations remain unchanged.
pub(crate) fn current_source_records(
    records: &[QuotaObservationRecord],
) -> Vec<&QuotaObservationRecord> {
    let mut latest = std::collections::BTreeMap::<&String, &QuotaObservationRecord>::new();
    for record in records {
        if let Some(id) = &record.credential_id {
            // Source publication and lifecycle mutations share a per-source
            // lock. Append order therefore identifies its current binding even
            // if a previously captured check is future-dated or the clock rolls
            // back. Window freshness still uses timestamps independently.
            latest.insert(id, record);
        }
    }
    records
        .iter()
        .filter(|record| {
            record.credential_id.as_ref().is_none_or(|id| {
                let current = latest[id];
                current.usage_source.as_deref() != Some("credential_removed")
                    && record.backend == current.backend
                    && record.backend_instance == current.backend_instance
                    && record.quota_pool == current.quota_pool
            })
        })
        .collect()
}
