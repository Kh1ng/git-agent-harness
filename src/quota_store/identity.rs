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
    let source = identity.quota_source.as_deref();
    latest_windows_for_identity_and_credential(
        records,
        identity,
        source.or(identity.credential_id.as_deref()),
    )
}

/// An explicit credential binding cannot inherit ambient or sibling source
/// readings. A shared verified billing pool remains visible at account scope.
/// Without a credential binding, explicit instances exclude unscoped readings
/// before freshness selection so ambient checks cannot replace their windows.
pub fn latest_windows_for_identity_and_credential<'a>(
    records: &'a [QuotaObservationRecord],
    identity: &crate::execution_identity::ExecutionIdentity,
    credential_id: Option<&str>,
) -> Vec<&'a QuotaObservationRecord> {
    let matching: Vec<_> = current_source_records(records)
        .into_iter()
        .filter(|record| match credential_id {
            Some(id) => record.credential_id.as_deref() == Some(id),
            None => {
                observation_matches_identity(record, identity)
                    && (!identity.explicit_instance
                        || record.backend_instance.is_some()
                        || record.quota_pool.is_some())
            }
        })
        .collect();
    // One identity pins its instance, pool and credential, so the source key
    // is the credential only; window qualifiers are appended by the core.
    let source_key: fn(&QuotaObservationRecord) -> Vec<Option<&str>> =
        |record| vec![record.credential_id.as_deref()];
    select_latest_windows(matching, source_key)
}

/// Latest windows for every source identity under one logical backend
/// (#1339). Unlike [`latest_windows_for_identity`] this is not pinned to one
/// instance, pool or credential: each distinct source keeps its own windows,
/// so a backend-scoped view (the report) shows every configured account that
/// reported, instead of only legacy unscoped rows.
pub fn latest_windows_for_backend<'a>(
    records: &'a [QuotaObservationRecord],
    logical_backend: &str,
) -> Vec<&'a QuotaObservationRecord> {
    let matching: Vec<_> = current_source_records(records)
        .into_iter()
        .filter(|record| record.backend == logical_backend)
        .collect();
    // A backend view spans many accounts, so the source key carries the full
    // identity: a failed check must not wipe a sibling account's windows.
    let source_key: fn(&QuotaObservationRecord) -> Vec<Option<&str>> = |record| {
        vec![
            record.credential_id.as_deref(),
            record.backend_instance.as_deref(),
            record.quota_pool.as_deref(),
            record.model.as_deref(),
        ]
    };
    select_latest_windows(matching, source_key)
}

/// Shared freshness selection: a failed or empty check invalidates earlier
/// data for its source (an account-wide check with no window invalidates all
/// of that source's windows), and the newest valid reading wins per window.
fn select_latest_windows<'a, S>(
    matching: Vec<&'a QuotaObservationRecord>,
    source_key: S,
) -> Vec<&'a QuotaObservationRecord>
where
    S: Fn(&'a QuotaObservationRecord) -> Vec<Option<&'a str>>,
{
    let timestamp = |record: &QuotaObservationRecord| {
        record
            .checked_at
            .as_deref()
            .or(record.observed_at.as_deref())
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
    };
    let with_window = |record: &'a QuotaObservationRecord| {
        let mut key = (source_key)(record);
        key.push(record.quota_window.as_deref());
        key
    };
    let mut windows =
        std::collections::BTreeMap::<Vec<Option<&'a str>>, &QuotaObservationRecord>::new();
    let mut invalidated = std::collections::BTreeMap::<Vec<Option<&'a str>>, OffsetDateTime>::new();
    for check in matching
        .iter()
        .filter(|check| check.check_error.is_some() || !has_quota_data(check))
    {
        if let Some(checked) = timestamp(check) {
            let current = invalidated.entry(with_window(check)).or_insert(checked);
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
        let mut account_wide = source_key(record);
        account_wide.push(None);
        if [
            invalidated.get(&account_wide),
            invalidated.get(&with_window(record)),
        ]
        .into_iter()
        .flatten()
        .any(|checked| timestamp(record).is_none_or(|observed| *checked >= observed))
        {
            continue;
        }
        let key = with_window(record);
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
    let mut lifecycle = std::collections::BTreeMap::<&String, usize>::new();
    for (index, record) in records.iter().enumerate() {
        if let Some(id) = &record.credential_id {
            if matches!(
                record.usage_source.as_deref(),
                Some("credential_updated" | "credential_removed")
            ) {
                lifecycle.insert(id, index);
            }
            // Source publication and lifecycle mutations share a per-source
            // lock. Append order therefore identifies its current binding even
            // if a previously captured check is future-dated or the clock rolls
            // back. Window freshness still uses timestamps independently.
            latest.insert(id, record);
        }
    }
    records
        .iter()
        .enumerate()
        .filter(|(index, record)| {
            record.credential_id.as_ref().is_none_or(|id| {
                let current = latest[id];
                current.usage_source.as_deref() != Some("credential_removed")
                    && lifecycle.get(id).is_none_or(|cutoff| index >= cutoff)
                    && record.backend == current.backend
                    && record.backend_instance == current.backend_instance
                    && record.quota_pool == current.quota_pool
            })
        })
        .map(|(_, record)| record)
        .collect()
}
