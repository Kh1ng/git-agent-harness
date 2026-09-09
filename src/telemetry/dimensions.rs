use super::{final_identity, identity_for_attempt, AggregationDimension};
use crate::ledger::LedgerEntry;
use time::OffsetDateTime;

/// Get dimension value from ledger entry or attempt
pub(super) fn get_dimension_value_from_attempt(
    entry: &LedgerEntry,
    attempt: Option<&crate::ledger::AttemptRecord>,
    dimension: AggregationDimension,
) -> String {
    match attempt {
        None => match dimension {
            AggregationDimension::Outcome => resource_outcome(
                entry.backend_exit_code,
                entry.validation_result.as_deref(),
                entry.failure_class.as_deref(),
            ),
            AggregationDimension::Project => entry.repo_id.clone(),
            AggregationDimension::Ticket => entry
                .work_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::ExecutionType => entry.mode.clone(),
            AggregationDimension::Runner => final_identity(entry)
                .map(|identity| identity.runner_kind.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Backend => entry.effective_backend.clone(),
            AggregationDimension::BackendInstance => entry
                .usage
                .backend_instance
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Provider => entry
                .usage
                .provider
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::AuthClass => entry
                .usage
                .usage_classification
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::QuotaPool => entry
                .usage
                .quota_pool
                .clone()
                .or_else(|| final_identity(entry).and_then(|identity| identity.quota_pool.clone()))
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Model => entry.effective_model.clone().unwrap_or_else(|| {
                entry
                    .requested_model
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string())
            }),
            AggregationDimension::Account => entry
                .usage
                .account_label
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Date => {
                if let Ok(entry_time) = OffsetDateTime::parse(
                    &entry.timestamp,
                    &time::format_description::well_known::Rfc3339,
                ) {
                    entry_time
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_else(|_| "unknown".to_string())
                        .split('T')
                        .next()
                        .unwrap_or("unknown")
                        .to_string()
                } else {
                    "unknown".to_string()
                }
            }
            AggregationDimension::DateRange => {
                if let Ok(entry_time) = OffsetDateTime::parse(
                    &entry.timestamp,
                    &time::format_description::well_known::Rfc3339,
                ) {
                    format!("{}-{:02}", entry_time.year(), entry_time.month() as u8)
                } else {
                    "unknown".to_string()
                }
            }
        },
        Some(att) => match dimension {
            AggregationDimension::Outcome => resource_outcome(
                att.exit_code,
                att.validation_result.as_deref(),
                att.failure_class.as_deref(),
            ),
            AggregationDimension::Project => entry.repo_id.clone(),
            AggregationDimension::Ticket => entry
                .work_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::ExecutionType => entry.mode.clone(),
            AggregationDimension::Runner => identity_for_attempt(entry, att.attempt_number)
                .map(|identity| identity.runner_kind.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Backend => identity_for_attempt(entry, att.attempt_number)
                .map(|identity| identity.logical_backend.clone())
                .unwrap_or_else(|| att.backend.clone()),
            AggregationDimension::BackendInstance => att
                .usage
                .backend_instance
                .clone()
                .or_else(|| {
                    identity_for_attempt(entry, att.attempt_number)
                        .map(|identity| identity.backend_instance.clone())
                })
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Provider => att
                .usage
                .provider
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::AuthClass => att
                .usage
                .usage_classification
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::QuotaPool => att
                .usage
                .quota_pool
                .clone()
                .or_else(|| {
                    identity_for_attempt(entry, att.attempt_number)
                        .and_then(|identity| identity.quota_pool.clone())
                })
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Model => att
                .usage
                .actual_model
                .clone()
                .or_else(|| att.effective_model.clone())
                .or_else(|| entry.effective_model.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Account => att
                .usage
                .account_label
                .clone()
                .or_else(|| {
                    identity_for_attempt(entry, att.attempt_number)
                        .and_then(|identity| identity.account_label.clone())
                })
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Date => {
                let timestamp = att
                    .usage
                    .observed_at
                    .clone()
                    .or_else(|| Some(entry.timestamp.clone()))
                    .unwrap_or_else(|| "unknown".to_string());
                if let Ok(entry_time) = OffsetDateTime::parse(
                    &timestamp,
                    &time::format_description::well_known::Rfc3339,
                ) {
                    entry_time
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_else(|_| "unknown".to_string())
                        .split('T')
                        .next()
                        .unwrap_or("unknown")
                        .to_string()
                } else {
                    "unknown".to_string()
                }
            }
            AggregationDimension::DateRange => {
                let timestamp = att
                    .usage
                    .observed_at
                    .clone()
                    .or_else(|| Some(entry.timestamp.clone()))
                    .unwrap_or_else(|| "unknown".to_string());
                if let Ok(entry_time) = OffsetDateTime::parse(
                    &timestamp,
                    &time::format_description::well_known::Rfc3339,
                ) {
                    format!("{}-{:02}", entry_time.year(), entry_time.month() as u8)
                } else {
                    "unknown".to_string()
                }
            }
        },
    }
}

/// Get dimension key name
pub(super) fn dimension_key(dimension: AggregationDimension) -> String {
    match dimension {
        AggregationDimension::Project => "project".to_string(),
        AggregationDimension::Ticket => "ticket".to_string(),
        AggregationDimension::ExecutionType => "execution_type".to_string(),
        AggregationDimension::Runner => "runner".to_string(),
        AggregationDimension::Backend => "backend".to_string(),
        AggregationDimension::BackendInstance => "backend_instance".to_string(),
        AggregationDimension::Provider => "provider".to_string(),
        AggregationDimension::AuthClass => "auth_class".to_string(),
        AggregationDimension::QuotaPool => "quota_pool".to_string(),
        AggregationDimension::Model => "model".to_string(),
        AggregationDimension::Account => "account".to_string(),
        AggregationDimension::Date => "date".to_string(),
        AggregationDimension::DateRange => "date_range".to_string(),
        AggregationDimension::Outcome => "outcome".to_string(),
    }
}
/// Keep cancellation separate from provider/validation failures in resource reports.
fn resource_outcome(exit: Option<i32>, validation: Option<&str>, failure: Option<&str>) -> String {
    if exit == Some(-2) || validation.is_some_and(|value| value.starts_with("cancelled")) {
        "cancelled"
    } else if failure.is_some() || validation == Some("fail") || exit.is_some_and(|code| code != 0)
    {
        "failed"
    } else if validation == Some("pass") || exit == Some(0) {
        "success"
    } else {
        "unknown"
    }
    .into()
}
