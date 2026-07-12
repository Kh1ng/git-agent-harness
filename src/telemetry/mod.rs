//! Telemetry Export Module
//!
//! Provides durable, versioned persistence of usage, quota, and outcome telemetry
//! to a separate private dataset repository.
//!
//! This module implements the requirements from TICKET-130 for preserving
//! high-value execution telemetry independently of the operational ledger.

pub mod exporter;
pub mod extractor;
pub mod records;

// Re-export commonly used types and functions
// pub use exporter::{ExportFormat, TelemetryConfig, TelemetryExporter, telemetry_repo_exists};

use crate::config::GahConfig;
use crate::ledger::{read_entries, LedgerEntry, LedgerUsage};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use time::OffsetDateTime;

/// GroupBy options for telemetry export (matching existing ledger summary)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    None,
    Backend,
    Model,
}

/// Aggregation dimensions for telemetry reports
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationDimension {
    Project,
    Ticket,
    ExecutionType,
    Backend,
    BackendInstance,
    Provider,
    Model,
    Account,
    Date,
    DateRange,
}

/// Telemetry aggregation report structure
#[derive(Debug, Serialize, Clone)]
pub struct TelemetryReport {
    pub report_type: String,
    pub generated_at: String,
    pub time_range: Option<String>,
    pub profile: Option<String>,
    pub total_entries: usize,
    pub total_attempts: usize,
    pub successful_attempts: usize,
    pub failed_attempts: usize,
    pub total_cost_usd: f64,
    pub quota_backed_cost_usd: f64,
    pub api_cost_usd: f64,
    pub aggregated_data: Vec<AggregatedTelemetryData>,
}

/// Aggregated telemetry data for a specific dimension
#[derive(Debug, Serialize, Clone)]
pub struct AggregatedTelemetryData {
    pub dimension_key: String,
    pub dimension_value: String,
    pub entries: usize,
    pub attempts: usize,
    pub successful_attempts: usize,
    pub failed_attempts: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub requests_count: u64,
    pub estimated_cost_usd: f64,
    pub actual_cost_usd: f64,
    pub quota_backed_cost_usd: f64,
    pub api_cost_usd: f64,
    pub average_cost_per_attempt: f64,
    pub success_rate: f64,
    pub failure_details: BTreeMap<String, usize>,
}

/// Telemetry aggregation parameters
#[derive(Debug, Clone)]
pub struct AggregationParams {
    pub dimensions: Vec<AggregationDimension>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub profile: Option<String>,
    pub include_failed_attempts: bool,
    pub include_retried_attempts: bool,
    pub project: Option<String>,
    pub ticket: Option<String>,
    pub execution_type: Option<String>,
    pub backend_instance: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
}

impl std::str::FromStr for GroupBy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(GroupBy::None),
            "backend" => Ok(GroupBy::Backend),
            "model" => Ok(GroupBy::Model),
            _ => Err(format!("Unknown group by option: {}", s)),
        }
    }
}

impl std::str::FromStr for AggregationDimension {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "project" => Ok(AggregationDimension::Project),
            "ticket" => Ok(AggregationDimension::Ticket),
            "executiontype" | "execution_type" => Ok(AggregationDimension::ExecutionType),
            "backend" => Ok(AggregationDimension::Backend),
            "backendinstance" | "backend_instance" => Ok(AggregationDimension::BackendInstance),
            "provider" => Ok(AggregationDimension::Provider),
            "model" => Ok(AggregationDimension::Model),
            "account" => Ok(AggregationDimension::Account),
            "date" => Ok(AggregationDimension::Date),
            "daterange" | "date_range" => Ok(AggregationDimension::DateRange),
            _ => Err(format!("Unknown aggregation dimension: {}", s)),
        }
    }
}

/// Generate telemetry aggregation report
pub fn generate_telemetry_report(
    cfg: &GahConfig,
    params: AggregationParams,
) -> Result<TelemetryReport> {
    let entries = read_entries(cfg)?;

    // Filter entries based on parameters
    let filtered_entries = filter_entries_for_aggregation(&entries, &params)?;

    if filtered_entries.is_empty() {
        return Ok(TelemetryReport {
            report_type: "aggregated".to_string(),
            generated_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "unknown".to_string()),
            time_range: None,
            profile: params.profile.clone(),
            total_entries: 0,
            total_attempts: 0,
            successful_attempts: 0,
            failed_attempts: 0,
            total_cost_usd: 0.0,
            quota_backed_cost_usd: 0.0,
            api_cost_usd: 0.0,
            aggregated_data: vec![],
        });
    }

    // Generate aggregated data for each dimension
    let mut aggregated_data = Vec::new();

    for dimension in &params.dimensions {
        let dimension_data = aggregate_by_dimension(&filtered_entries, *dimension, &params);
        aggregated_data.extend(dimension_data);
    }

    // Calculate totals
    let (
        total_entries,
        total_attempts,
        successful_attempts,
        failed_attempts,
        total_cost,
        quota_backed_cost,
        api_cost,
    ) = calculate_totals(&filtered_entries, &params);

    Ok(TelemetryReport {
        report_type: "aggregated".to_string(),
        generated_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".to_string()),
        time_range: build_time_range(&params),
        profile: params.profile.clone(),
        total_entries,
        total_attempts,
        successful_attempts,
        failed_attempts,
        total_cost_usd: total_cost,
        quota_backed_cost_usd: quota_backed_cost,
        api_cost_usd: api_cost,
        aggregated_data,
    })
}

/// Helper to check if a filter option matches a value
fn matches_filter_value(filter: &Option<String>, value: &str) -> bool {
    match filter {
        Some(f) => f.to_lowercase() == value.to_lowercase(),
        None => true,
    }
}

/// Determine if the entry/attempt matches the given parameters
fn matches_filters(
    entry: &LedgerEntry,
    attempt: Option<&crate::ledger::AttemptRecord>,
    params: &AggregationParams,
) -> bool {
    let (proj_val, ticket_val, exec_val, instance_val, provider_val, model_val, account_val) =
        match attempt {
            None => (
                entry.repo_id.clone(),
                entry
                    .work_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                entry
                    .task_class
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                entry
                    .usage
                    .backend_instance
                    .clone()
                    .unwrap_or_else(|| entry.effective_backend.clone()),
                entry
                    .usage
                    .provider
                    .clone()
                    .unwrap_or_else(|| entry.provider.clone()),
                entry.effective_model.clone().unwrap_or_else(|| {
                    entry
                        .requested_model
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string())
                }),
                entry
                    .usage
                    .account_label
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
            ),
            Some(att) => (
                entry.repo_id.clone(),
                entry
                    .work_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                entry
                    .task_class
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                att.usage
                    .backend_instance
                    .clone()
                    .or_else(|| Some(att.backend.clone()))
                    .unwrap_or_else(|| "unknown".to_string()),
                att.usage
                    .provider
                    .clone()
                    .or_else(|| Some(entry.provider.clone()))
                    .unwrap_or_else(|| "unknown".to_string()),
                att.usage
                    .actual_model
                    .clone()
                    .or_else(|| att.effective_model.clone())
                    .or_else(|| entry.effective_model.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                att.usage
                    .account_label
                    .clone()
                    .or_else(|| entry.usage.account_label.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
            ),
        };

    matches_filter_value(&params.project, &proj_val)
        && matches_filter_value(&params.ticket, &ticket_val)
        && matches_filter_value(&params.execution_type, &exec_val)
        && matches_filter_value(&params.backend_instance, &instance_val)
        && matches_filter_value(&params.provider, &provider_val)
        && matches_filter_value(&params.model, &model_val)
        && matches_filter_value(&params.account, &account_val)
}

/// Filter entries for telemetry aggregation
fn filter_entries_for_aggregation(
    entries: &[LedgerEntry],
    params: &AggregationParams,
) -> Result<Vec<LedgerEntry>> {
    let mut filtered = entries.to_vec();

    // Filter by profile
    if let Some(profile_name) = &params.profile {
        filtered.retain(|e| e.profile == *profile_name);
    }

    // Filter by time range
    if let Some(since_str) = &params.since {
        let since_time = parse_timestamp(since_str)?;
        filtered.retain(|entry| {
            if let Ok(entry_time) = OffsetDateTime::parse(
                &entry.timestamp,
                &time::format_description::well_known::Rfc3339,
            ) {
                entry_time >= since_time
            } else {
                true
            }
        });
    }

    if let Some(until_str) = &params.until {
        let until_time = parse_timestamp(until_str)?;
        filtered.retain(|entry| {
            if let Ok(entry_time) = OffsetDateTime::parse(
                &entry.timestamp,
                &time::format_description::well_known::Rfc3339,
            ) {
                entry_time <= until_time
            } else {
                true
            }
        });
    }

    // Filter by other dimensions at the entry or attempt level
    filtered.retain(|entry| {
        if entry.attempts.is_empty() {
            matches_filters(entry, None, params)
        } else {
            entry
                .attempts
                .iter()
                .any(|att| matches_filters(entry, Some(att), params))
        }
    });

    Ok(filtered)
}

/// Parse timestamp string to OffsetDateTime
fn parse_timestamp(timestamp_str: &str) -> Result<OffsetDateTime> {
    use time::format_description::well_known::Rfc3339;

    if let Ok(datetime) = OffsetDateTime::parse(timestamp_str, &Rfc3339) {
        return Ok(datetime);
    }

    // Try parsing as date only
    let date_parts: Vec<&str> = timestamp_str.split('-').collect();
    if date_parts.len() == 3 {
        let year = date_parts[0].parse::<i32>().unwrap_or(2024);
        let month = date_parts[1].parse::<u8>().unwrap_or(1);
        let day = date_parts[2].parse::<u8>().unwrap_or(1);

        let month_enum = time::Month::try_from(month).unwrap_or(time::Month::January);
        let date = time::Date::from_calendar_date(year, month_enum, day)?;
        let primitive_datetime = date.with_hms_milli(0, 0, 0, 0)?;
        return Ok(primitive_datetime.assume_utc());
    }

    Err(anyhow::anyhow!(
        "Invalid timestamp format: {}",
        timestamp_str
    ))
}

/// Build time range string for report
fn build_time_range(params: &AggregationParams) -> Option<String> {
    match (&params.since, &params.until) {
        (Some(since), Some(until)) => Some(format!("{} to {}", since, until)),
        (Some(since), None) => Some(format!("from {}", since)),
        (None, Some(until)) => Some(format!("until {}", until)),
        (None, None) => None,
    }
}

/// Helper struct for aggregation to track unique entry counts
struct AggregationBuilder {
    pub data: AggregatedTelemetryData,
    pub seen_entries: std::collections::HashSet<String>,
}

/// Aggregate data by specific dimension
fn aggregate_by_dimension(
    entries: &[LedgerEntry],
    dimension: AggregationDimension,
    params: &AggregationParams,
) -> Vec<AggregatedTelemetryData> {
    let mut aggregated_map: BTreeMap<String, AggregationBuilder> = BTreeMap::new();

    for entry in entries {
        let entry_id = format!("{}_{}", entry.timestamp, entry.repo_id);

        if entry.attempts.is_empty() {
            if matches_filters(entry, None, params) {
                let is_failed = entry.validation_result.as_deref() == Some("fail")
                    || entry.failure_class.is_some()
                    || (entry.backend_exit_code.is_some() && entry.backend_exit_code != Some(0));

                if is_failed && !params.include_failed_attempts {
                    continue;
                }

                let dimension_value = get_dimension_value_from_attempt(entry, None, dimension);

                let builder = aggregated_map
                    .entry(dimension_value.clone())
                    .or_insert_with(|| AggregationBuilder {
                        data: AggregatedTelemetryData {
                            dimension_key: dimension_key(dimension),
                            dimension_value: dimension_value.clone(),
                            entries: 0,
                            attempts: 0,
                            successful_attempts: 0,
                            failed_attempts: 0,
                            input_tokens: 0,
                            output_tokens: 0,
                            total_tokens: 0,
                            requests_count: 0,
                            estimated_cost_usd: 0.0,
                            actual_cost_usd: 0.0,
                            quota_backed_cost_usd: 0.0,
                            api_cost_usd: 0.0,
                            average_cost_per_attempt: 0.0,
                            success_rate: 0.0,
                            failure_details: BTreeMap::new(),
                        },
                        seen_entries: std::collections::HashSet::new(),
                    });

                if builder.seen_entries.insert(entry_id.clone()) {
                    builder.data.entries += 1;
                }
                builder.data.attempts += 1;

                if entry.validation_result.as_deref() == Some("pass") {
                    builder.data.successful_attempts += 1;
                } else if is_failed {
                    builder.data.failed_attempts += 1;
                    if let Some(failure_class) = &entry.failure_class {
                        *builder
                            .data
                            .failure_details
                            .entry(failure_class.clone())
                            .or_insert(0) += 1;
                    }
                }

                // Sum usage metrics from entry
                let entry_usage = &entry.usage;
                builder.data.input_tokens += entry_usage.input_tokens.unwrap_or(0);
                builder.data.output_tokens += entry_usage.output_tokens.unwrap_or(0);
                builder.data.total_tokens += entry_usage.total_tokens.unwrap_or(0);
                builder.data.requests_count += entry_usage.requests_count.unwrap_or(0);
                builder.data.estimated_cost_usd += entry_usage.estimated_cost_usd.unwrap_or(0.0);
                builder.data.actual_cost_usd += entry_usage.actual_cost_usd.unwrap_or(0.0);

                // Classify entry usage as quota-backed vs API cost
                if is_quota_backed(entry_usage) {
                    builder.data.quota_backed_cost_usd +=
                        entry_usage.actual_cost_usd.unwrap_or(0.0);
                } else {
                    builder.data.api_cost_usd += entry_usage.actual_cost_usd.unwrap_or(0.0);
                }
            }
        } else {
            for (i, attempt) in entry.attempts.iter().enumerate() {
                if matches_filters(entry, Some(attempt), params) {
                    let is_retried = i < entry.attempts.len() - 1;
                    let is_failed = attempt.validation_result.as_deref() == Some("fail")
                        || attempt.failure_class.is_some()
                        || (attempt.exit_code.is_some() && attempt.exit_code != Some(0));

                    if is_retried && !params.include_retried_attempts {
                        continue;
                    }
                    if is_failed && !params.include_failed_attempts {
                        continue;
                    }

                    let dimension_value =
                        get_dimension_value_from_attempt(entry, Some(attempt), dimension);

                    let builder = aggregated_map
                        .entry(dimension_value.clone())
                        .or_insert_with(|| AggregationBuilder {
                            data: AggregatedTelemetryData {
                                dimension_key: dimension_key(dimension),
                                dimension_value: dimension_value.clone(),
                                entries: 0,
                                attempts: 0,
                                successful_attempts: 0,
                                failed_attempts: 0,
                                input_tokens: 0,
                                output_tokens: 0,
                                total_tokens: 0,
                                requests_count: 0,
                                estimated_cost_usd: 0.0,
                                actual_cost_usd: 0.0,
                                quota_backed_cost_usd: 0.0,
                                api_cost_usd: 0.0,
                                average_cost_per_attempt: 0.0,
                                success_rate: 0.0,
                                failure_details: BTreeMap::new(),
                            },
                            seen_entries: std::collections::HashSet::new(),
                        });

                    if builder.seen_entries.insert(entry_id.clone()) {
                        builder.data.entries += 1;
                    }
                    builder.data.attempts += 1;

                    if attempt.validation_result.as_deref() == Some("pass") {
                        builder.data.successful_attempts += 1;
                    } else if is_failed || is_retried {
                        builder.data.failed_attempts += 1;
                        if let Some(failure_class) = &attempt.failure_class {
                            *builder
                                .data
                                .failure_details
                                .entry(failure_class.clone())
                                .or_insert(0) += 1;
                        } else if is_retried {
                            *builder
                                .data
                                .failure_details
                                .entry("retried".to_string())
                                .or_insert(0) += 1;
                        } else {
                            *builder
                                .data
                                .failure_details
                                .entry("unknown_failure".to_string())
                                .or_insert(0) += 1;
                        }
                    }

                    // Sum attempt usage
                    let attempt_usage = &attempt.usage;
                    builder.data.input_tokens += attempt_usage.input_tokens.unwrap_or(0);
                    builder.data.output_tokens += attempt_usage.output_tokens.unwrap_or(0);
                    builder.data.total_tokens += attempt_usage.total_tokens.unwrap_or(0);
                    builder.data.requests_count += attempt_usage.requests_count.unwrap_or(0);
                    builder.data.estimated_cost_usd +=
                        attempt_usage.estimated_cost_usd.unwrap_or(0.0);
                    builder.data.actual_cost_usd += attempt_usage.actual_cost_usd.unwrap_or(0.0);

                    // Classify attempt usage
                    if is_quota_backed(attempt_usage) {
                        builder.data.quota_backed_cost_usd +=
                            attempt_usage.actual_cost_usd.unwrap_or(0.0);
                    } else {
                        builder.data.api_cost_usd += attempt_usage.actual_cost_usd.unwrap_or(0.0);
                    }
                }
            }
        }
    }

    // Calculate derived metrics
    let mut result = Vec::new();
    for builder in aggregated_map.into_values() {
        let mut data = builder.data;
        if data.attempts > 0 {
            data.average_cost_per_attempt = data.actual_cost_usd / data.attempts as f64;
            data.success_rate = data.successful_attempts as f64 / data.attempts as f64;
        }
        result.push(data);
    }

    result
}

/// Get dimension value from ledger entry or attempt
fn get_dimension_value_from_attempt(
    entry: &LedgerEntry,
    attempt: Option<&crate::ledger::AttemptRecord>,
    dimension: AggregationDimension,
) -> String {
    match attempt {
        None => match dimension {
            AggregationDimension::Project => entry.repo_id.clone(),
            AggregationDimension::Ticket => entry
                .work_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::ExecutionType => entry
                .task_class
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Backend => entry.effective_backend.clone(),
            AggregationDimension::BackendInstance => entry
                .usage
                .backend_instance
                .clone()
                .unwrap_or_else(|| entry.effective_backend.clone()),
            AggregationDimension::Provider => entry
                .usage
                .provider
                .clone()
                .unwrap_or_else(|| entry.provider.clone()),
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
                    format!("{}-{}", entry_time.year(), entry_time.month() as u8)
                } else {
                    "unknown".to_string()
                }
            }
        },
        Some(att) => match dimension {
            AggregationDimension::Project => entry.repo_id.clone(),
            AggregationDimension::Ticket => entry
                .work_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::ExecutionType => entry
                .task_class
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Backend => att.backend.clone(),
            AggregationDimension::BackendInstance => att
                .usage
                .backend_instance
                .clone()
                .or_else(|| Some(att.backend.clone()))
                .unwrap_or_else(|| "unknown".to_string()),
            AggregationDimension::Provider => att
                .usage
                .provider
                .clone()
                .or_else(|| Some(entry.provider.clone()))
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
                .or_else(|| entry.usage.account_label.clone())
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
                    format!("{}-{}", entry_time.year(), entry_time.month() as u8)
                } else {
                    "unknown".to_string()
                }
            }
        },
    }
}

/// Get dimension key name
fn dimension_key(dimension: AggregationDimension) -> String {
    match dimension {
        AggregationDimension::Project => "project".to_string(),
        AggregationDimension::Ticket => "ticket".to_string(),
        AggregationDimension::ExecutionType => "execution_type".to_string(),
        AggregationDimension::Backend => "backend".to_string(),
        AggregationDimension::BackendInstance => "backend_instance".to_string(),
        AggregationDimension::Provider => "provider".to_string(),
        AggregationDimension::Model => "model".to_string(),
        AggregationDimension::Account => "account".to_string(),
        AggregationDimension::Date => "date".to_string(),
        AggregationDimension::DateRange => "date_range".to_string(),
    }
}

/// Check whether usage belongs to a subscription/quota pool rather than a
/// metered API account. `quota_backed` is the canonical ledger value; retain
/// `subscription` for older records written before normalization.
fn is_quota_backed(usage: &LedgerUsage) -> bool {
    usage.quota_window.is_some()
        || matches!(
            usage.usage_classification.as_deref(),
            Some("quota_backed" | "subscription")
        )
}

/// Calculate totals across all entries
fn calculate_totals(
    entries: &[LedgerEntry],
    params: &AggregationParams,
) -> (usize, usize, usize, usize, f64, f64, f64) {
    let mut total_entries = 0;
    let mut total_attempts = 0;
    let mut successful_attempts = 0;
    let mut failed_attempts = 0;
    let mut total_cost = 0.0;
    let mut quota_backed_cost = 0.0;
    let mut api_cost = 0.0;

    for entry in entries {
        let mut entry_matched = false;

        if entry.attempts.is_empty() {
            if matches_filters(entry, None, params) {
                let is_failed = entry.validation_result.as_deref() == Some("fail")
                    || entry.failure_class.is_some()
                    || (entry.backend_exit_code.is_some() && entry.backend_exit_code != Some(0));

                if is_failed && !params.include_failed_attempts {
                    continue;
                }

                entry_matched = true;
                total_attempts += 1;

                if entry.validation_result.as_deref() == Some("pass") {
                    successful_attempts += 1;
                } else if is_failed {
                    failed_attempts += 1;
                }

                let entry_usage = &entry.usage;
                let actual = entry_usage.actual_cost_usd.unwrap_or(0.0);
                total_cost += actual;
                if is_quota_backed(entry_usage) {
                    quota_backed_cost += actual;
                } else {
                    api_cost += actual;
                }
            }
        } else {
            for (i, attempt) in entry.attempts.iter().enumerate() {
                if matches_filters(entry, Some(attempt), params) {
                    let is_retried = i < entry.attempts.len() - 1;
                    let is_failed = attempt.validation_result.as_deref() == Some("fail")
                        || attempt.failure_class.is_some()
                        || (attempt.exit_code.is_some() && attempt.exit_code != Some(0));

                    if is_retried && !params.include_retried_attempts {
                        continue;
                    }
                    if is_failed && !params.include_failed_attempts {
                        continue;
                    }

                    entry_matched = true;
                    total_attempts += 1;

                    if attempt.validation_result.as_deref() == Some("pass") {
                        successful_attempts += 1;
                    } else if is_failed || is_retried {
                        failed_attempts += 1;
                    }

                    let attempt_usage = &attempt.usage;
                    let actual = attempt_usage.actual_cost_usd.unwrap_or(0.0);
                    total_cost += actual;
                    if is_quota_backed(attempt_usage) {
                        quota_backed_cost += actual;
                    } else {
                        api_cost += actual;
                    }
                }
            }
        }

        if entry_matched {
            total_entries += 1;
        }
    }

    (
        total_entries,
        total_attempts,
        successful_attempts,
        failed_attempts,
        total_cost,
        quota_backed_cost,
        api_cost,
    )
}

/// Main telemetry export function that handles the full workflow
pub fn export_telemetry_from_config(
    cfg: &GahConfig,
    telemetry_repo_path: Option<&str>,
    format: exporter::ExportFormat,
    generate_manifests: bool,
    _group_by: GroupBy,
    since: Option<&str>,
    profile: Option<&str>,
) -> Result<exporter::TelemetryExporter> {
    use crate::ledger::read_entries;

    // Load ledger entries
    let entries = read_entries(cfg)?;

    // Filter entries based on parameters
    let entries = filter_entries(&entries, profile, since)?;

    if entries.is_empty() {
        log::info!("No ledger entries found for telemetry export");
        // Return an exporter with no records exported
        let config = exporter::TelemetryConfig {
            telemetry_repo_path: determine_telemetry_repo_path(telemetry_repo_path),
            format,
            generate_manifests,
            commit_batch_size: None,
        };
        return exporter::TelemetryExporter::new(config);
    }

    // Determine telemetry repo path
    let repo_path = determine_telemetry_repo_path(telemetry_repo_path);

    // Create and run exporter
    let mut exporter = exporter::TelemetryExporter::new(exporter::TelemetryConfig {
        telemetry_repo_path: repo_path.clone(),
        format,
        generate_manifests,
        commit_batch_size: None,
    })?;

    // Load already exported IDs to avoid duplicates
    exporter.load_exported_ids()?;

    // Export telemetry records
    let _exported_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc().unix_timestamp().to_string());

    exporter.export_from_entries(&entries)?;

    Ok(exporter)
}

/// Filter entries based on profile and time
fn filter_entries(
    entries: &[LedgerEntry],
    profile: Option<&str>,
    since: Option<&str>,
) -> Result<Vec<LedgerEntry>> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    let mut filtered = entries.to_vec();

    // Filter by profile if specified
    if let Some(profile_name) = profile {
        filtered.retain(|e| e.profile == profile_name);
    }

    // Filter by time if specified
    if let Some(since_str) = since {
        let since_time: OffsetDateTime =
            if let Ok(datetime) = OffsetDateTime::parse(since_str, &Rfc3339) {
                // Absolute datetime
                datetime
            } else {
                // Try parsing as date only
                let since_date = since_str.trim();
                let date_parts: Vec<&str> = since_date.split('-').collect();
                if date_parts.len() == 3 {
                    let year = date_parts[0].parse::<i32>().unwrap_or(2024);
                    let month = date_parts[1].parse::<u8>().unwrap_or(1);
                    let day = date_parts[2].parse::<u8>().unwrap_or(1);

                    // Create a time at midnight UTC
                    let month_enum = time::Month::try_from(month).unwrap_or(time::Month::January);
                    let date = time::Date::from_calendar_date(year, month_enum, day)?;
                    let primitive_datetime = date.with_hms_milli(0, 0, 0, 0)?;
                    // Convert PrimitiveDateTime to OffsetDateTime (UTC)
                    primitive_datetime.assume_utc()
                } else {
                    // Default to all entries if we can't parse
                    return Ok(filtered);
                }
            };

        filtered.retain(|entry| {
            if let Ok(entry_time) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
                entry_time >= since_time
            } else {
                // If we can't parse the timestamp, include it
                true
            }
        });
    }

    Ok(filtered)
}

/// Determine the telemetry repository path
fn determine_telemetry_repo_path(telemetry_repo_path: Option<&str>) -> std::path::PathBuf {
    if let Some(path) = telemetry_repo_path {
        return std::path::PathBuf::from(path);
    }

    // Look for telemetry submodule in current directory
    let current_dir = std::env::current_dir().unwrap_or_default();
    let submodule_path = current_dir.join("telemetry");

    if submodule_path.exists() && submodule_path.is_dir() {
        return submodule_path;
    }

    // Fallback to config directory
    let config_dir = crate::config::default_config_dir();
    config_dir.join("telemetry")
}

/// Tests for telemetry functionality
#[cfg(test)]
pub mod tests;

/// CLI interface for telemetry commands
pub mod cli {
    use super::*;
    use anyhow::Result;
    use std::path::PathBuf;

    /// Run telemetry export command
    #[allow(clippy::too_many_arguments)]
    pub fn run_export(
        telemetry_repo_path: Option<&str>,
        format: Option<exporter::ExportFormat>,
        output: Option<&str>,
        since: Option<&str>,
        profile: Option<&str>,
        group_by: Option<GroupBy>,
        generate_manifests: bool,
        config_path: Option<&str>,
    ) -> Result<()> {
        let cfg = crate::config::load(config_path)?;

        // Determine telemetry repo path
        let repo_path = if let Some(path) = telemetry_repo_path {
            PathBuf::from(path)
        } else {
            determine_telemetry_repo_path(None)
        };

        // Determine output path
        let output_path = if let Some(output) = output {
            PathBuf::from(output)
        } else {
            repo_path.clone()
        };

        // Create output directory if it doesn't exist
        if !output_path.exists() {
            std::fs::create_dir_all(&output_path)?;
        }

        let export_format = format.unwrap_or(exporter::ExportFormat::Jsonl);
        let group_by_option = group_by.unwrap_or(GroupBy::None);

        log::info!("Exporting telemetry to: {}", output_path.display());
        log::info!("Format: {:?}", export_format);
        log::info!("Generate manifests: {}", generate_manifests);

        let _exporter = export_telemetry_from_config(
            &cfg,
            Some(output_path.to_str().unwrap()),
            export_format,
            generate_manifests,
            group_by_option,
            since,
            profile,
        )?;

        log::info!("Telemetry export completed successfully");
        Ok(())
    }

    /// Run telemetry aggregation report command
    #[allow(clippy::too_many_arguments)]
    pub fn run_aggregate(
        dimensions: Vec<AggregationDimension>,
        since: Option<&str>,
        until: Option<&str>,
        profile: Option<&str>,
        include_failed: bool,
        include_retried: bool,
        json: bool,
        config_path: Option<&str>,
        project: Option<&str>,
        ticket: Option<&str>,
        execution_type: Option<&str>,
        backend_instance: Option<&str>,
        provider: Option<&str>,
        model: Option<&str>,
        account: Option<&str>,
    ) -> Result<()> {
        let cfg = crate::config::load(config_path)?;

        let params = AggregationParams {
            dimensions,
            since: since.map(|s| s.to_string()),
            until: until.map(|s| s.to_string()),
            profile: profile.map(|s| s.to_string()),
            include_failed_attempts: include_failed,
            include_retried_attempts: include_retried,
            project: project.map(|s| s.to_string()),
            ticket: ticket.map(|s| s.to_string()),
            execution_type: execution_type.map(|s| s.to_string()),
            backend_instance: backend_instance.map(|s| s.to_string()),
            provider: provider.map(|s| s.to_string()),
            model: model.map(|s| s.to_string()),
            account: account.map(|s| s.to_string()),
        };

        let report = generate_telemetry_report(&cfg, params)?;

        if json {
            let json_output = serde_json::to_string_pretty(&report)?;
            println!("{}", json_output);
        } else {
            print_telemetry_report(&report);
        }

        Ok(())
    }

    /// Print telemetry report in human-readable format
    fn print_telemetry_report(report: &TelemetryReport) {
        println!("Telemetry Aggregation Report");
        println!("=============================");
        println!("Generated: {}", report.generated_at);
        if let Some(time_range) = &report.time_range {
            println!("Time Range: {}", time_range);
        }
        println!(
            "Profile: {}",
            report.profile.clone().unwrap_or_else(|| "all".to_string())
        );
        println!();

        println!("Summary:");
        println!("  Total Entries: {}", report.total_entries);
        println!("  Total Attempts: {}", report.total_attempts);
        println!("  Successful Attempts: {}", report.successful_attempts);
        println!("  Failed Attempts: {}", report.failed_attempts);
        println!("  Total Cost: ${:.2}", report.total_cost_usd);
        println!("  Quota-Backed Cost: ${:.2}", report.quota_backed_cost_usd);
        println!("  API Cost: ${:.2}", report.api_cost_usd);
        println!();

        if !report.aggregated_data.is_empty() {
            println!("Detailed Breakdown:");
            println!();

            // Group by dimension for display
            let mut grouped_data: BTreeMap<String, Vec<&AggregatedTelemetryData>> = BTreeMap::new();
            for data in &report.aggregated_data {
                grouped_data
                    .entry(data.dimension_key.clone())
                    .or_default()
                    .push(data);
            }

            for (dimension_key, data_list) in &grouped_data {
                println!("By {}:", dimension_key);
                println!(
                    "{:<30} {:<15} {:<15} {:<15} {:<15} {:<15}",
                    "Value", "Attempts", "Success Rate", "Total Cost", "Quota Cost", "API Cost"
                );
                println!(
                    "{:<30} {:<15} {:<15} {:<15} {:<15} {:<15}",
                    "-".repeat(30),
                    "-".repeat(15),
                    "-".repeat(15),
                    "-".repeat(15),
                    "-".repeat(15),
                    "-".repeat(15)
                );

                for data in data_list {
                    let success_rate_pct = (data.success_rate * 100.0).round();
                    println!(
                        "{:<30} {:<15} {:<15.1}% {:<15.2} {:<15.2} {:<15.2}",
                        data.dimension_value,
                        data.attempts,
                        success_rate_pct,
                        data.actual_cost_usd,
                        data.quota_backed_cost_usd,
                        data.api_cost_usd
                    );
                }
                println!();
            }
        }
    }

    /// Run telemetry status command
    pub fn run_status(telemetry_repo_path: Option<&str>, config_path: Option<&str>) -> Result<()> {
        let _cfg = crate::config::load(config_path)?;

        // Determine telemetry repo path
        let repo_path = if let Some(path) = telemetry_repo_path {
            PathBuf::from(path)
        } else {
            determine_telemetry_repo_path(None)
        };

        if !exporter::telemetry_repo_exists(&repo_path) {
            println!("Telemetry repository not found at: {}", repo_path.display());
            println!("Telemetry export is optional and not required for normal operation.");
            return Ok(());
        }

        // Count existing records
        let mut total_records = 0;
        let mut records_by_type: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();

        let raw_dir = repo_path.join("raw");
        if raw_dir.exists() {
            for subdir in ["usage", "quota", "outcomes"] {
                let subdir_path = raw_dir.join(subdir);
                if subdir_path.exists() {
                    count_records_in_directory(
                        &subdir_path,
                        &mut total_records,
                        &mut records_by_type,
                    )?;
                }
            }
        }

        println!("Telemetry Repository Status");
        println!("==========================");
        println!("Path: {}", repo_path.display());
        println!("Exists: {}", exporter::telemetry_repo_exists(&repo_path));
        println!("Total Records: {}", total_records);
        println!();
        println!("Records by type:");
        for (record_type, count) in &records_by_type {
            println!("  {}: {}", record_type, count);
        }

        // Check manifests
        let manifests_dir = repo_path.join("manifests");
        if manifests_dir.exists() {
            let manifest_count = std::fs::read_dir(&manifests_dir)?.count();
            println!();
            println!("Manifests: {}", manifest_count);
        }

        Ok(())
    }

    /// Count records in a directory
    fn count_records_in_directory(
        dir: &std::path::Path,
        total_records: &mut usize,
        records_by_type: &mut std::collections::BTreeMap<String, usize>,
    ) -> Result<()> {
        if !dir.exists() || !dir.is_dir() {
            return Ok(());
        }

        let mut dirs_to_visit = vec![dir.to_path_buf()];

        while let Some(current_dir) = dirs_to_visit.pop() {
            for entry in std::fs::read_dir(&current_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    dirs_to_visit.push(path);
                } else if path
                    .extension()
                    .map(|e| e.to_string_lossy() == "jsonl")
                    .unwrap_or(false)
                {
                    // Count records in JSONL file
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let record_count = content
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .filter(|line| {
                                // Try to parse as JSON to verify it's valid
                                serde_json::from_str::<serde_json::Value>(line).is_ok()
                            })
                            .count();

                        *total_records += record_count;

                        // Determine record type from file path
                        let record_type = if path
                            .parent()
                            .map(|p| {
                                p.file_name()
                                    .map(|n| n.to_string_lossy() == "usage")
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false)
                        {
                            "attempt_usage".to_string()
                        } else if path
                            .parent()
                            .map(|p| {
                                p.file_name()
                                    .map(|n| n.to_string_lossy() == "quota")
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false)
                        {
                            "quota_observation".to_string()
                        } else if path
                            .parent()
                            .map(|p| {
                                p.file_name()
                                    .map(|n| n.to_string_lossy() == "outcomes")
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false)
                        {
                            "outcome".to_string()
                        } else {
                            "unknown".to_string()
                        };

                        *records_by_type.entry(record_type).or_insert(0) += record_count;
                    }
                }
            }
        }

        Ok(())
    }
}
