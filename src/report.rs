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
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: Option<u64>,
    requests_count: Option<u64>,
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
    } = args;

    // Use the existing ledger summary functionality but with grouping
    let cfg = config::load(config_path.as_deref())?;
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
            let success_rate = if group.entries > 0 {
                group.validation_pass as f64 / group.entries as f64
            } else {
                0.0
            };

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
                cache_read_tokens: group.cache_read_tokens,
                cache_write_tokens: group.cache_write_tokens,
                total_tokens: group.total_tokens,
                requests_count: group.requests_count,
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
            Some("passed") | Some("APPROVE_STRONG") | Some("APPROVE_WEAK")
        ) {
            point.validation_pass += 1;
        }
        point.total_tokens += entry.usage.total_tokens.unwrap_or(0);
        add_optional(&mut point.actual_cost_usd, entry.usage.actual_cost_usd);
        add_optional(
            &mut point.estimated_cost_usd,
            entry.usage.estimated_cost_usd,
        );
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
                "  Usage: input={} output={} cache_read={} cache_write={} total={} requests={}",
                group
                    .input_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                group
                    .output_tokens
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
            validation_pass: 8,
            review_verdict_distribution: BTreeMap::from([("APPROVE_STRONG".to_string(), 2)]),
            total_cost_usd: Some(1.50),
            actual_cost_usd: Some(1.50),
            estimated_cost_usd: None,
            average_cost_usd: Some(0.15),
            average_duration_seconds: Some(120.5),
            cost_per_approve_strong: Some(0.75),
            input_tokens: Some(700),
            output_tokens: Some(300),
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(1000),
            requests_count: Some(10),
            quota_observations: vec![],
        });
        grouped_by_backend.push(GroupSummary {
            group_key: "codex".to_string(),
            entries: 5,
            attempts: 3,
            validation_pass: 4,
            review_verdict_distribution: BTreeMap::new(),
            total_cost_usd: Some(2.00),
            actual_cost_usd: Some(2.00),
            estimated_cost_usd: None,
            average_cost_usd: Some(0.40),
            average_duration_seconds: Some(180.0),
            cost_per_approve_strong: None,
            input_tokens: Some(300),
            output_tokens: Some(200),
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(500),
            requests_count: Some(10),
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
            average_duration_seconds: Some(150.0),
            usage_input_tokens: Some(1000),
            usage_output_tokens: Some(500),
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
            validation_pass: 2,
            review_verdict_distribution: BTreeMap::new(),
            total_cost_usd: None,
            actual_cost_usd: None,
            estimated_cost_usd: None,
            average_cost_usd: None,
            average_duration_seconds: None,
            cost_per_approve_strong: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: None,
            requests_count: None,
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
            validation_pass: 0,
            review_verdict_distribution: BTreeMap::new(),
            total_cost_usd: None,
            actual_cost_usd: None,
            estimated_cost_usd: None,
            average_cost_usd: None,
            average_duration_seconds: None,
            cost_per_approve_strong: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: None,
            requests_count: None,
            quota_observations: vec![],
        }];

        let mut data = mock_summary_data();
        data.grouped_by_backend = Some(grouped_data);

        let report_data =
            transform_to_report_format(None, &data, GroupBy::Backend, "7d", None).unwrap();

        assert_eq!(report_data.comparisons[0].success_rate, 0.0);
    }
}
