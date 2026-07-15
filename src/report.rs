/// TICKET-098: Unified model/backend usage comparison report
/// This module provides the `gah report` command for backend/model comparison.
///
/// DESIGN DECISION: Retired external model-usage.jsonl tracking in favor of ledger's
/// richer schema. The ledger already contains all the usage fields (input/output/cache
/// tokens, request counts, estimated/actual cost, quota info) that model-usage.jsonl lacked,
/// and is the canonical source of truth for GAH's own retry-cap/status logic.
use crate::config;
use crate::ledger::summary::{build_summary, SummaryData};
use crate::ledger::{self, GroupBy as LedgerGroupBy};
use anyhow::Result;
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::BTreeMap;

/// Report command parameters
pub struct ReportArgs {
    pub since: String,
    pub profile: Option<String>,
    pub config_path: Option<String>,
    pub group_by: LedgerGroupBy,
    pub json: bool,
    /// When set, emit a time-bucketed series (one row per bucket) instead
    /// of the single aggregate-per-backend/model report. Additive flag: the
    /// non-series aggregate behavior is unchanged when this is false.
    pub series: bool,
    /// Bucket granularity for `--series`. Currently only `daily` is
    /// supported; each bucket is keyed by calendar date (UTC).
    pub bucket: String,
}

/// Unified report data structure
#[derive(Debug, Serialize)]
struct ReportData {
    ledger_path: String,
    total_entries: usize,
    since: String,
    profile: Option<String>,
    group_by: String,
    comparisons: Vec<BackendModelComparison>,
    trend: Vec<TrendPoint>,
}

#[derive(Debug, Serialize)]
struct TrendPoint {
    date: String,
    entries: usize,
    validation_pass: usize,
    total_tokens: u64,
    actual_cost_usd: Option<f64>,
    estimated_cost_usd: Option<f64>,
}

/// Comparison data for a single backend or model
#[derive(Debug, Serialize)]
struct BackendModelComparison {
    backend_or_model: String,
    is_model: bool,
    entries: usize,
    attempts: usize,
    validation_pass: usize,
    success_rate: f64,
    total_cost_usd: Option<f64>,
    actual_cost_usd: Option<f64>,
    estimated_cost_usd: Option<f64>,
    average_cost_usd: Option<f64>,
    average_duration_seconds: Option<f64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: Option<u64>,
    requests_count: Option<u64>,
    tokens_per_success: Option<f64>,
    requests_per_success: Option<f64>,
    quota_observations: Vec<crate::ledger::summary::GroupQuotaObservation>,
    review_verdict_distribution: Vec<(String, usize)>,
}

/// Run the report command
pub fn run(args: ReportArgs) -> Result<()> {
    let ReportArgs {
        since,
        profile,
        config_path,
        group_by,
        json,
        series,
        bucket,
    } = args;

    // Use the existing ledger summary functionality but with grouping
    let cfg = config::load(config_path.as_deref())?;

    if series {
        let series_data = build_series(&cfg, &since, profile.as_deref(), &bucket)?;
        if json {
            println!("{}", serde_json::to_string(&series_data)?);
        } else {
            display_series(&series_data)?;
        }
        return Ok(());
    }

    let data = build_summary(&cfg, &since, profile.as_deref(), group_by)?;

    if json {
        // For JSON output, transform the existing summary data into our report format
        let report_data =
            transform_to_report_format(Some(&cfg), &data, group_by, &since, profile.as_deref())?;
        println!("{}", serde_json::to_string(&report_data)?);
        return Ok(());
    }

    // For plain text output, use the existing grouped display but with report-focused formatting
    display_report(&data, group_by, &since, profile.as_deref())?;

    Ok(())
}

/// One time-bucketed row of the `--series` report: a per-bucket aggregate
/// of usage/cost/success over ledger entries whose timestamp falls in the
/// bucket.
#[derive(Debug, Serialize)]
pub struct ReportSeriesPoint {
    /// Bucket key. For `daily` this is the calendar date (`YYYY-MM-DD`), UTC.
    pub date: String,
    pub total_tokens: u64,
    pub actual_cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    /// Fraction 0..1 (entries with a validation pass / total entries).
    pub success_rate: f64,
    pub entries: usize,
    pub validation_pass: usize,
}

/// Top-level payload for `gah report --series --json`.
#[derive(Debug, Serialize)]
pub struct ReportSeriesData {
    pub ledger_path: String,
    pub since: String,
    pub bucket: String,
    pub profile: Option<String>,
    pub series: Vec<ReportSeriesPoint>,
}

/// Build a time-bucketed series of usage/cost/success over the ledger.
///
/// Reuses the same ledger-scanning logic as the existing aggregate report
/// (read entries, apply `--since`/`--profile` filters, bucket by calendar
/// date for `daily`, aggregate token/cost/validation fields from the
/// per-attempt usage records), just grouping into a series of buckets
/// instead of collapsing to one row per backend/model.
pub fn build_series(
    cfg: &config::GahConfig,
    since: &str,
    profile: Option<&str>,
    bucket: &str,
) -> Result<ReportSeriesData> {
    if bucket != "daily" {
        anyhow::bail!(
            "unsupported --bucket value '{}'; only 'daily' is supported",
            bucket
        );
    }

    let cutoff = ledger::summary::parse_since(since)?;
    let mut entries = ledger::read_entries(cfg)?;
    if let Some(profile) = profile {
        entries.retain(|entry| entry.profile == profile);
    }
    entries.retain(|entry| entry.timestamp >= cutoff);

    let mut points: BTreeMap<String, ReportSeriesPoint> = BTreeMap::new();
    for entry in &entries {
        // Bucket key is the calendar date prefix (UTC). Entries without a
        // usable date prefix are skipped rather than silently dropped into
        // a wrong bucket.
        let Some(date) = entry.timestamp.get(..10) else {
            continue;
        };
        let point = points
            .entry(date.to_string())
            .or_insert_with(|| ReportSeriesPoint {
                date: date.to_string(),
                total_tokens: 0,
                actual_cost_usd: None,
                estimated_cost_usd: None,
                success_rate: 0.0,
                entries: 0,
                validation_pass: 0,
            });
        point.entries += 1;
        if matches!(
            entry.validation_result.as_deref(),
            Some("passed") | Some("APPROVE")
        ) {
            point.validation_pass += 1;
        }

        // Token/cost telemetry lives on the per-attempt usage records, not
        // the top-level entry usage (which is only populated for some
        // backends). Aggregate from attempts, falling back to the
        // top-level entry usage when there are no attempts.
        let mut tokens: u64 = 0;
        let mut actual: Option<f64> = None;
        let mut estimated: Option<f64> = None;
        let mut saw_usage = false;
        for attempt in &entry.attempts {
            if let Some(t) = attempt.usage.total_tokens {
                tokens += t;
                saw_usage = true;
            }
            add_optional(&mut actual, attempt.usage.actual_cost_usd);
            add_optional(&mut estimated, attempt.usage.estimated_cost_usd);
        }
        if !saw_usage {
            tokens += entry.usage.total_tokens.unwrap_or(0);
            add_optional(&mut actual, entry.usage.actual_cost_usd);
            add_optional(&mut estimated, entry.usage.estimated_cost_usd);
        }
        point.total_tokens += tokens;
        add_optional(&mut point.actual_cost_usd, actual);
        add_optional(&mut point.estimated_cost_usd, estimated);
    }

    let mut series: Vec<ReportSeriesPoint> = points.into_values().collect();
    for point in &mut series {
        point.success_rate = if point.entries > 0 {
            point.validation_pass as f64 / point.entries as f64
        } else {
            0.0
        };
    }

    Ok(ReportSeriesData {
        ledger_path: cfg.defaults.ledger_path().to_string_lossy().to_string(),
        since: since.to_string(),
        bucket: bucket.to_string(),
        profile: profile.map(String::from),
        series,
    })
}

/// Plain-text rendering for `gah report --series`.
fn display_series(data: &ReportSeriesData) -> Result<()> {
    println!("Time-bucketed usage series (bucket: {}):", data.bucket);
    println!("Ledger: {}", data.ledger_path);
    println!("Window: last {}", data.since);
    if let Some(profile) = &data.profile {
        println!("Profile: {}", profile);
    }
    println!();

    if data.series.is_empty() {
        println!("  No entries in this window.");
        return Ok(());
    }

    println!(
        "{:<12} {:>10} {:>12} {:>12} {:>8}",
        "date", "tokens", "actual$", "est$", "success"
    );
    for point in &data.series {
        println!(
            "{:<12} {:>10} {:>12} {:>12} {:>7.1}%",
            point.date,
            point.total_tokens,
            point
                .actual_cost_usd
                .map(|c| format!("{c:.4}"))
                .unwrap_or_else(|| "n/a".to_string()),
            point
                .estimated_cost_usd
                .map(|c| format!("{c:.4}"))
                .unwrap_or_else(|| "n/a".to_string()),
            point.success_rate * 100.0
        );
    }

    Ok(())
}

/// Transform summary data to report format
fn transform_to_report_format(
    cfg: Option<&config::GahConfig>,
    data: &SummaryData,
    group_by: LedgerGroupBy,
    since: &str,
    profile: Option<&str>,
) -> Result<ReportData> {
    let grouped_data = if group_by == LedgerGroupBy::Backend {
        &data.grouped_by_backend
    } else {
        &data.grouped_by_model
    };

    let is_model = group_by == LedgerGroupBy::Model;
    let group_label = if is_model { "Model" } else { "Backend" };

    let mut comparisons = Vec::new();

    if let Some(groups) = grouped_data {
        for group in groups {
            let success_rate = group.success_rate.unwrap_or(0.0);

            let review_verdicts: Vec<(String, usize)> = group
                .review_verdict_distribution
                .iter()
                .map(|(v, c)| (v.clone(), *c))
                .collect();

            comparisons.push(BackendModelComparison {
                backend_or_model: group.group_key.clone(),
                is_model,
                entries: group.entries,
                attempts: group.attempts,
                validation_pass: group.validation_pass,
                success_rate,
                total_cost_usd: group.total_cost_usd,
                actual_cost_usd: group.actual_cost_usd,
                estimated_cost_usd: group.estimated_cost_usd,
                average_cost_usd: group.average_cost_usd,
                average_duration_seconds: group.average_duration_seconds,
                input_tokens: group.input_tokens,
                output_tokens: group.output_tokens,
                reasoning_tokens: group.reasoning_tokens,
                cache_read_tokens: group.cache_read_tokens,
                cache_write_tokens: group.cache_write_tokens,
                total_tokens: group.total_tokens,
                requests_count: group.requests_count,
                tokens_per_success: group.tokens_per_success,
                requests_per_success: group.requests_per_success,
                quota_observations: group.quota_observations.clone(),
                review_verdict_distribution: review_verdicts,
            });
        }
    }

    // Sort by entries descending for better readability
    comparisons.sort_by_key(|b| Reverse(b.entries));

    let trend = cfg
        .map(|cfg| build_trend(cfg, since, profile))
        .transpose()?
        .unwrap_or_default();

    Ok(ReportData {
        ledger_path: data.ledger_path.clone(),
        total_entries: data.entries,
        since: since.to_string(),
        profile: profile.map(String::from),
        group_by: group_label.to_string(),
        comparisons,
        trend,
    })
}

fn build_trend(
    cfg: &config::GahConfig,
    since: &str,
    profile: Option<&str>,
) -> Result<Vec<TrendPoint>> {
    let cutoff = ledger::summary::parse_since(since)?;
    let mut entries = ledger::read_entries(cfg)?;
    if let Some(profile) = profile {
        entries.retain(|entry| entry.profile == profile);
    }
    entries.retain(|entry| entry.timestamp >= cutoff);

    let mut points: BTreeMap<String, TrendPoint> = BTreeMap::new();
    for entry in &entries {
        let Some(date) = entry.timestamp.get(..10) else {
            continue;
        };
        let point = points
            .entry(date.to_string())
            .or_insert_with(|| TrendPoint {
                date: date.to_string(),
                entries: 0,
                validation_pass: 0,
                total_tokens: 0,
                actual_cost_usd: None,
                estimated_cost_usd: None,
            });
        point.entries += 1;
        if matches!(
            entry.validation_result.as_deref(),
            Some("passed") | Some("APPROVE")
        ) {
            point.validation_pass += 1;
        }
        // Token/cost telemetry lives on the per-attempt usage records, not
        // the top-level entry usage (which is only populated for some
        // backends). Aggregate from attempts, falling back to the
        // top-level entry usage when there are no attempts.
        let mut tokens: u64 = 0;
        let mut actual: Option<f64> = None;
        let mut estimated: Option<f64> = None;
        let mut saw_usage = false;
        for attempt in &entry.attempts {
            if let Some(t) = attempt.usage.total_tokens {
                tokens += t;
                saw_usage = true;
            }
            add_optional(&mut actual, attempt.usage.actual_cost_usd);
            add_optional(&mut estimated, attempt.usage.estimated_cost_usd);
        }
        if !saw_usage {
            tokens += entry.usage.total_tokens.unwrap_or(0);
            add_optional(&mut actual, entry.usage.actual_cost_usd);
            add_optional(&mut estimated, entry.usage.estimated_cost_usd);
        }
        point.total_tokens += tokens;
        add_optional(&mut point.actual_cost_usd, actual);
        add_optional(&mut point.estimated_cost_usd, estimated);
    }
    Ok(points.into_values().collect())
}

fn add_optional(total: &mut Option<f64>, value: Option<f64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0.0) + value);
    }
}

/// Display report in plain text format
fn display_report(
    data: &SummaryData,
    group_by: LedgerGroupBy,
    since: &str,
    profile: Option<&str>,
) -> Result<()> {
    let group_label = if group_by == LedgerGroupBy::Backend {
        "Backend"
    } else {
        "Model"
    };

    println!("Total entries: {}", data.entries);
    println!("Time range: last {}", since);
    if let Some(profile) = profile {
        println!("Profile: {}", profile);
    }
    println!();

    let grouped_data = if group_by == LedgerGroupBy::Backend {
        &data.grouped_by_backend
    } else {
        &data.grouped_by_model
    };

    println!("{} Comparison Report:", group_label);
    println!("{}", "=".repeat(50));

    if let Some(groups) = grouped_data {
        // Sort by entries descending
        let mut sorted_groups = groups.to_vec();
        sorted_groups.sort_by_key(|b| Reverse(b.entries));

        for group in sorted_groups {
            let success_rate = if group.entries > 0 {
                (group.validation_pass as f64 / group.entries as f64) * 100.0
            } else {
                0.0
            };

            println!("\n{}:", group.group_key);
            println!("  Entries: {} ({} attempts)", group.entries, group.attempts);
            println!(
                "  Success rate: {:.1}% (validation pass: {}/{})",
                success_rate, group.validation_pass, group.entries
            );

            // Show cost info
            if let Some(total_cost) = group.total_cost_usd {
                if let Some(avg_cost) = group.average_cost_usd {
                    println!(
                        "  Total cost: ${:.4}, Avg cost: ${:.4}",
                        total_cost, avg_cost
                    );
                } else {
                    println!("  Total cost: ${:.4}", total_cost);
                }
            }
            if let Some(actual_cost) = group.actual_cost_usd {
                println!("  Actual cost: ${:.4}", actual_cost);
            }
            if let Some(estimated_cost) = group.estimated_cost_usd {
                println!("  Estimated cost: ${:.4}", estimated_cost);
            }

            // Show duration
            if let Some(avg_duration) = group.average_duration_seconds {
                println!("  Avg duration: {:.1}s", avg_duration);
            }
            println!(
                "  Usage: input={} output={} reasoning={} cache_read={} cache_write={} total={} requests={}",
                group
                    .input_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                group
                    .output_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                group
                    .reasoning_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                group
                    .cache_read_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                group
                    .cache_write_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                group
                    .total_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                group
                    .requests_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
            );

            // Show review verdict distribution (compact)
            if !group.review_verdict_distribution.is_empty() {
                let verdicts: Vec<String> = group
                    .review_verdict_distribution
                    .iter()
                    .map(|(v, c)| format!("{} {} ", v, c))
                    .collect();
                println!("  Review verdicts: {}", verdicts.join(", "));
            }
            if !group.quota_observations.is_empty() {
                for quota in &group.quota_observations {
                    println!(
                        "  Quota [{}{}]: used={} remaining={} reset={} observed={}",
                        quota.backend,
                        quota
                            .quota_window
                            .as_deref()
                            .map(|w| format!(":{w}"))
                            .unwrap_or_default(),
                        quota
                            .quota_used_percent
                            .map(|n| format!("{n:.1}%"))
                            .unwrap_or_else(|| "unknown".to_string()),
                        quota
                            .quota_remaining_percent
                            .map(|n| format!("{n:.1}%"))
                            .unwrap_or_else(|| "unknown".to_string()),
                        quota.quota_reset_at.as_deref().unwrap_or("unknown"),
                        quota.observed_at.as_deref().unwrap_or("unknown"),
                    );
                }
            }
        }
    } else {
        println!("  No {} data available", group_label.to_lowercase());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::summary::{GroupSummary, SummaryData};
    use crate::ledger::GroupBy;
    use std::collections::BTreeMap;

    #[allow(clippy::vec_init_then_push)]
    fn mock_summary_data() -> SummaryData {
        let mut grouped_by_backend = Vec::new();
        grouped_by_backend.push(GroupSummary {
            group_key: "agy".to_string(),
            entries: 10,
            attempts: 5,
            attempts_started: Some(5),
            attempts_completed: Some(5),
            attempts_started_unknown: 0,
            attempts_completed_unknown: 0,
            validation_pass: 8,
            success_rate: Some(0.8),
            review_verdict_distribution: BTreeMap::from([("APPROVE".to_string(), 2)]),
            total_cost_usd: Some(1.50),
            actual_cost_usd: Some(1.50),
            estimated_cost_usd: None,
            average_cost_usd: Some(0.15),
            average_duration_seconds: Some(120.5),
            cost_per_approve_strong: Some(0.75),
            input_tokens: Some(700),
            output_tokens: Some(300),
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(1000),
            requests_count: Some(10),
            tokens_per_success: Some(125.0),
            requests_per_success: Some(1.25),
            quota_observations: vec![],
        });
        grouped_by_backend.push(GroupSummary {
            group_key: "codex".to_string(),
            entries: 5,
            attempts: 3,
            attempts_started: Some(3),
            attempts_completed: Some(3),
            attempts_started_unknown: 0,
            attempts_completed_unknown: 0,
            validation_pass: 4,
            success_rate: Some(0.8),
            review_verdict_distribution: BTreeMap::new(),
            total_cost_usd: Some(2.00),
            actual_cost_usd: Some(2.00),
            estimated_cost_usd: None,
            average_cost_usd: Some(0.40),
            average_duration_seconds: Some(180.0),
            cost_per_approve_strong: None,
            input_tokens: Some(300),
            output_tokens: Some(200),
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(500),
            requests_count: Some(10),
            tokens_per_success: Some(125.0),
            requests_per_success: Some(2.5),
            quota_observations: vec![],
        });

        SummaryData {
            ledger_path: "/tmp/ledger.jsonl".to_string(),
            entries: 15,
            success: 12,
            failed: 3,
            by_mode: BTreeMap::new(),
            by_requested_backend: BTreeMap::new(),
            by_backend: BTreeMap::new(),
            by_model: BTreeMap::new(),
            by_failure_class: BTreeMap::new(),
            fallback_count: 0,
            validation_pass: 12,
            push_success: 10,
            mr_count: 5,
            human_required_count: 1,
            attempts_started: Some(8),
            attempts_completed: Some(8),
            attempts_started_unknown: 0,
            attempts_completed_unknown: 0,
            average_duration_seconds: Some(150.0),
            usage_input_tokens: Some(1000),
            usage_output_tokens: Some(500),
            usage_reasoning_tokens: None,
            usage_cache_read_tokens: None,
            usage_cache_write_tokens: None,
            usage_total_tokens: Some(1500),
            usage_requests_count: Some(20),
            estimated_cost_usd: Some(3.50),
            actual_cost_usd: Some(3.50),
            last_run: Some("2026-07-07 10:00:00 UTC agy improve".to_string()),
            grouped_by_backend: Some(grouped_by_backend),
            grouped_by_model: None,
        }
    }

    #[test]
    fn test_transform_to_report_format_backend() {
        let data = mock_summary_data();
        let report_data =
            transform_to_report_format(None, &data, GroupBy::Backend, "7d", None).unwrap();

        assert_eq!(report_data.total_entries, 15);
        assert_eq!(report_data.since, "7d");
        assert_eq!(report_data.group_by, "Backend");
        assert_eq!(report_data.comparisons.len(), 2);

        // Check first backend (agy should be first due to sorting by entries)
        assert_eq!(report_data.comparisons[0].backend_or_model, "agy");
        assert_eq!(report_data.comparisons[0].entries, 10);
        assert_eq!(report_data.comparisons[0].attempts, 5);
        assert_eq!(report_data.comparisons[0].validation_pass, 8);
        assert!((report_data.comparisons[0].success_rate - 0.8).abs() < 0.001);
        assert_eq!(
            report_data.comparisons[0].average_duration_seconds,
            Some(120.5)
        );
        assert!(!report_data.comparisons[0].is_model);

        // Check second backend (codex)
        assert_eq!(report_data.comparisons[1].backend_or_model, "codex");
        assert_eq!(report_data.comparisons[1].entries, 5);
        assert!((report_data.comparisons[1].success_rate - 0.8).abs() < 0.001);
        assert!(!report_data.comparisons[1].is_model);
    }

    #[test]
    fn test_transform_to_report_format_model() {
        let mut data = mock_summary_data();
        // Swap to model grouping
        data.grouped_by_model = data.grouped_by_backend.take();

        let report_data =
            transform_to_report_format(None, &data, GroupBy::Model, "30d", Some("test-profile"))
                .unwrap();

        assert_eq!(report_data.total_entries, 15);
        assert_eq!(report_data.since, "30d");
        assert_eq!(report_data.profile, Some("test-profile".to_string()));
        assert_eq!(report_data.group_by, "Model");
        assert_eq!(report_data.comparisons.len(), 2);

        // All should be marked as models
        assert!(report_data.comparisons[0].is_model);
        assert!(report_data.comparisons[1].is_model);
    }

    #[test]
    fn test_empty_grouped_data() {
        let mut data = mock_summary_data();
        data.grouped_by_backend = None;

        let report_data =
            transform_to_report_format(None, &data, GroupBy::Backend, "7d", None).unwrap();

        assert_eq!(report_data.comparisons.len(), 0);
    }

    #[test]
    fn test_success_rate_calculation() {
        let grouped_data = vec![GroupSummary {
            group_key: "test".to_string(),
            entries: 4,
            attempts: 2,
            attempts_started: None,
            attempts_completed: None,
            attempts_started_unknown: 0,
            attempts_completed_unknown: 0,
            validation_pass: 2,
            success_rate: Some(0.5),
            review_verdict_distribution: BTreeMap::new(),
            total_cost_usd: None,
            actual_cost_usd: None,
            estimated_cost_usd: None,
            average_cost_usd: None,
            average_duration_seconds: None,
            cost_per_approve_strong: None,
            input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: None,
            requests_count: None,
            tokens_per_success: None,
            requests_per_success: None,
            quota_observations: vec![],
        }];

        let mut data = mock_summary_data();
        data.grouped_by_backend = Some(grouped_data);

        let report_data =
            transform_to_report_format(None, &data, GroupBy::Backend, "7d", None).unwrap();

        assert!((report_data.comparisons[0].success_rate - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_zero_entries_success_rate() {
        let grouped_data = vec![GroupSummary {
            group_key: "test".to_string(),
            entries: 0,
            attempts: 0,
            attempts_started: None,
            attempts_completed: None,
            attempts_started_unknown: 0,
            attempts_completed_unknown: 0,
            validation_pass: 0,
            success_rate: None,
            review_verdict_distribution: BTreeMap::new(),
            total_cost_usd: None,
            actual_cost_usd: None,
            estimated_cost_usd: None,
            average_cost_usd: None,
            average_duration_seconds: None,
            cost_per_approve_strong: None,
            input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: None,
            requests_count: None,
            tokens_per_success: None,
            requests_per_success: None,
            quota_observations: vec![],
        }];

        let mut data = mock_summary_data();
        data.grouped_by_backend = Some(grouped_data);

        let report_data =
            transform_to_report_format(None, &data, GroupBy::Backend, "7d", None).unwrap();

        assert_eq!(report_data.comparisons[0].success_rate, 0.0);
    }

    /// Build a `LedgerEntry` with a fixed timestamp/profile/validation and a
    /// single attempt carrying the given usage. Uses `LedgerEntry::new` for
    /// the defaults, then overrides the fields the series logic reads.
    fn entry(
        profile_name: &str,
        timestamp: &str,
        validation_result: Option<&str>,
        total_tokens: u64,
        actual_cost_usd: f64,
        estimated_cost_usd: f64,
    ) -> crate::ledger::LedgerEntry {
        let profile: crate::config::Profile = serde_json::from_str(
            r#"{"display_name":"r","repo_id":"r","provider":"github","repo":"o/r","local_path":"/tmp","artifact_root":"/tmp","default_target_branch":"main"}"#,
        )
        .expect("valid profile json");
        let mut e = crate::ledger::LedgerEntry::new(
            profile_name,
            &profile,
            "agy",
            "improve",
            "target",
            None,
            None,
        );
        e.timestamp = timestamp.to_string();
        e.profile = profile_name.to_string();
        e.validation_result = validation_result.map(String::from);
        e.attempts = vec![crate::ledger::AttemptRecord {
            attempt_number: 1,
            backend: "agy".to_string(),
            effective_model: None,
            exit_code: None,
            validation_result: None,
            failure_class: None,
            failure_stage: None,
            duration_seconds: None,
            diff_path: None,
            cli_version: None,
            usage: crate::ledger::LedgerUsage {
                total_tokens: Some(total_tokens),
                actual_cost_usd: Some(actual_cost_usd),
                estimated_cost_usd: Some(estimated_cost_usd),
                ..Default::default()
            },
        }];
        e
    }

    /// Write `entries` to a temp ledger file and return a config that points
    /// at it. The temp dir is returned so the caller keeps it alive for the
    /// duration of the assertions.
    fn config_with_ledger(
        entries: &[crate::ledger::LedgerEntry],
    ) -> (tempfile::TempDir, crate::config::GahConfig) {
        let tmp = tempfile::tempdir().unwrap();
        let ledger_path = tmp.path().join("ledger.jsonl");
        let artifact_root = tmp.path().to_string_lossy().into_owned();
        let mut text = String::new();
        for entry in entries {
            text.push_str(&serde_json::to_string(entry).unwrap());
            text.push('\n');
        }
        std::fs::write(&ledger_path, text).unwrap();
        let cfg = crate::config::GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
                artifact_root,
                worktree_base: String::new(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        (tmp, cfg)
    }

    #[test]
    fn test_build_series_buckets_by_day() {
        // Two days, two entries each. Day 1: both pass. Day 2: one pass one fail.
        let d1 = "2026-07-01";
        let d2 = "2026-07-02";
        let entries = vec![
            entry(
                "gah",
                &format!("{d1}T10:00:00Z"),
                Some("passed"),
                100,
                1.0,
                0.9,
            ),
            entry(
                "gah",
                &format!("{d1}T12:00:00Z"),
                Some("APPROVE"),
                200,
                2.0,
                1.8,
            ),
            entry(
                "gah",
                &format!("{d2}T09:00:00Z"),
                Some("passed"),
                300,
                3.0,
                2.7,
            ),
            entry(
                "gah",
                &format!("{d2}T15:00:00Z"),
                Some("failed"),
                400,
                4.0,
                3.6,
            ),
        ];

        let (_tmp, cfg) = config_with_ledger(&entries);
        let data = build_series(&cfg, "30d", None, "daily").unwrap();

        assert_eq!(data.bucket, "daily");
        assert_eq!(data.series.len(), 2);

        let day1 = data.series.iter().find(|p| p.date == d1).unwrap();
        assert_eq!(day1.entries, 2);
        assert_eq!(day1.validation_pass, 2);
        assert!((day1.success_rate - 1.0).abs() < 1e-9);
        assert_eq!(day1.total_tokens, 300);
        assert!((day1.actual_cost_usd.unwrap() - 3.0).abs() < 1e-9);
        assert!((day1.estimated_cost_usd.unwrap() - 2.7).abs() < 1e-9);

        let day2 = data.series.iter().find(|p| p.date == d2).unwrap();
        assert_eq!(day2.entries, 2);
        assert_eq!(day2.validation_pass, 1);
        assert!((day2.success_rate - 0.5).abs() < 1e-9);
        assert_eq!(day2.total_tokens, 700);
        assert!((day2.actual_cost_usd.unwrap() - 7.0).abs() < 1e-9);
    }

    #[test]
    fn test_build_series_filters_by_profile() {
        let entries = vec![
            entry("gah", "2026-07-01T10:00:00Z", Some("passed"), 100, 1.0, 0.9),
            entry(
                "other",
                "2026-07-01T11:00:00Z",
                Some("passed"),
                999,
                9.0,
                8.0,
            ),
        ];

        let (_tmp, cfg) = config_with_ledger(&entries);
        let data = build_series(&cfg, "30d", Some("gah"), "daily").unwrap();

        assert_eq!(data.series.len(), 1);
        assert_eq!(data.series[0].total_tokens, 100);
        assert_eq!(data.profile.as_deref(), Some("gah"));
    }

    #[test]
    fn test_build_series_rejects_unknown_bucket() {
        let (_tmp, cfg) = config_with_ledger(&[]);
        let err = build_series(&cfg, "30d", None, "hourly").unwrap_err();
        assert!(err.to_string().contains("unsupported --bucket"));
    }

    #[test]
    fn test_build_series_empty_when_no_entries() {
        let (_tmp, cfg) = config_with_ledger(&[]);
        let data = build_series(&cfg, "30d", None, "daily").unwrap();
        assert!(data.series.is_empty());
    }
}
