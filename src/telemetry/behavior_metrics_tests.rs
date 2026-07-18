//! Issue #119: provenance-aware per-attempt behavior metrics tests.
//!
//! Covers structured backends (exact counts), partial backends, and
//! unsupported backends (unknown distinct from zero), plus provenance
//! exposure in exported records and aggregated reports.

use super::tests::telemetry_tests::create_test_ledger_entry;
use crate::ledger::{
    AttemptBehaviorMetrics, AttemptRecord, BehaviorMetric, BehaviorMetricQuality, LedgerEntry,
    LedgerUsage,
};
use crate::telemetry::extractor::{
    extract_attempt_usage_records, parse_structured_behavior_events,
};
use crate::telemetry::records::{ExportedTelemetryRecord, SCHEMA_VERSION};
use crate::telemetry::{
    aggregate_by_dimension, AggregatedBehaviorMetric, AggregationDimension, AggregationParams,
    TelemetryReport,
};

#[cfg(test)]
#[allow(clippy::module_inception)]
mod behavior_metrics_tests {
    use super::*;

    fn attempt_with_behavior(
        attempt_number: u32,
        backend: &str,
        metrics: AttemptBehaviorMetrics,
    ) -> AttemptRecord {
        AttemptRecord {
            attempt_number,
            backend: backend.to_string(),
            effective_model: Some("model-x".to_string()),
            exit_code: Some(0),
            validation_result: Some("pass".to_string()),
            failure_class: None,
            failure_stage: None,
            duration_seconds: Some(1.0),
            diff_path: None,
            cli_version: None,
            usage: LedgerUsage {
                behavior_metrics: Some(metrics),
                ..LedgerUsage::default()
            },
        }
    }

    fn structured_metrics(
        tool_calls: u64,
        shell_calls: u64,
        file_edits: u64,
        test_runs: u64,
    ) -> AttemptBehaviorMetrics {
        AttemptBehaviorMetrics {
            tool_calls: Some(BehaviorMetric {
                count: Some(tool_calls),
                quality: BehaviorMetricQuality::StructuredEventDerived,
                unknown_reason: None,
            }),
            shell_calls: Some(BehaviorMetric {
                count: Some(shell_calls),
                quality: BehaviorMetricQuality::StructuredEventDerived,
                unknown_reason: None,
            }),
            file_edits: Some(BehaviorMetric {
                count: Some(file_edits),
                quality: BehaviorMetricQuality::StructuredEventDerived,
                unknown_reason: None,
            }),
            test_runs: Some(BehaviorMetric {
                count: Some(test_runs),
                quality: BehaviorMetricQuality::StructuredEventDerived,
                unknown_reason: None,
            }),
        }
    }

    #[test]
    fn behavior_metrics_extracted_with_provenance() {
        let mut entry = create_test_ledger_entry();
        entry.attempts = vec![attempt_with_behavior(
            1,
            "codex",
            structured_metrics(5, 2, 3, 1),
        )];

        let records = extract_attempt_usage_records(&entry, "2026-07-10T00:00:00Z");
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.tool_calls.as_ref().unwrap().count, Some(5));
        assert_eq!(
            r.tool_calls.as_ref().unwrap().quality,
            BehaviorMetricQuality::StructuredEventDerived
        );
        assert_eq!(r.shell_calls.as_ref().unwrap().count, Some(2));
        assert_eq!(r.file_edits.as_ref().unwrap().count, Some(3));
        assert_eq!(r.test_runs.as_ref().unwrap().count, Some(1));
    }

    #[test]
    fn behavior_metric_unknown_is_distinct_from_zero() {
        let mut entry = create_test_ledger_entry();
        // Unsupported backend: no behavior data at all.
        entry.attempts = vec![attempt_with_behavior(
            1,
            "legacy",
            AttemptBehaviorMetrics::default(),
        )];

        let records = extract_attempt_usage_records(&entry, "2026-07-10T00:00:00Z");
        let r = &records[0];
        // Unknown must be None, never Some(0).
        assert!(r.tool_calls.is_none());
        assert!(r.shell_calls.is_none());
        assert!(r.file_edits.is_none());
        assert!(r.test_runs.is_none());
    }

    #[test]
    fn structured_event_parser_only_parses_documented_event() {
        // Mix of documented event, arbitrary prose, and a malformed line.
        let events = "\
{\"type\":\"gah.behavior_summary\",\"tool_calls\":4,\"shell_calls\":1,\"file_edits\":2,\"test_runs\":3}
the agent ran some commands and edited a bunch of files
{\"type\":\"other.event\",\"tool_calls\":999}
{\"type\":\"gah.behavior_summary\",\"not_a_count\":\"oops\"}";

        let parsed =
            parse_structured_behavior_events(events).expect("should find documented event");
        assert_eq!(parsed.tool_calls.as_ref().unwrap().count, Some(4));
        assert_eq!(
            parsed.tool_calls.as_ref().unwrap().quality,
            BehaviorMetricQuality::StructuredEventDerived
        );
        assert_eq!(parsed.shell_calls.as_ref().unwrap().count, Some(1));
        assert_eq!(parsed.file_edits.as_ref().unwrap().count, Some(2));
        assert_eq!(parsed.test_runs.as_ref().unwrap().count, Some(3));
        // The non-documented event's bogus count must not leak in.
        assert_ne!(parsed.tool_calls.as_ref().unwrap().count, Some(999));
    }

    #[test]
    fn structured_event_parser_returns_none_without_documented_event() {
        let events = "the agent edited files and ran tests\n{\"type\":\"other.event\"}";
        assert!(parse_structured_behavior_events(events).is_none());
    }

    #[test]
    fn aggregate_sums_known_counts_and_keeps_unknown_separate() {
        let mut entry = create_test_ledger_entry();
        // One structured backend attempt (exact counts) and one unsupported
        // backend attempt (unknown). Both attempts preserved independently.
        entry.attempts = vec![
            attempt_with_behavior(1, "codex", structured_metrics(5, 2, 3, 1)),
            attempt_with_behavior(2, "legacy", AttemptBehaviorMetrics::default()),
        ];

        let params = AggregationParams {
            dimensions: vec![AggregationDimension::Backend],
            since: None,
            until: None,
            profile: None,
            include_failed_attempts: true,
            include_retried_attempts: true,
            project: None,
            ticket: None,
            execution_type: None,
            backend_instance: None,
            provider: None,
            model: None,
            account: None,
        };

        let aggregated = aggregate_by_dimension(&[entry], AggregationDimension::Backend, &params);
        let report = TelemetryReport {
            report_type: String::new(),
            generated_at: String::new(),
            time_range: None,
            profile: None,
            total_entries: 0,
            total_attempts: 0,
            successful_attempts: 0,
            failed_attempts: 0,
            total_cost_usd: 0.0,
            quota_backed_cost_usd: 0.0,
            api_cost_usd: 0.0,
            aggregated_data: aggregated,
        };
        // Two backend dimension buckets (codex, legacy) each with 1 attempt.
        let codex = report
            .aggregated_data
            .iter()
            .find(|d| d.dimension_value == "codex")
            .expect("codex bucket");
        assert_eq!(codex.attempts, 1);
        assert_eq!(codex.tool_calls.total, 5);
        assert_eq!(codex.tool_calls.known_attempts, 1);
        assert_eq!(codex.tool_calls.unknown_attempts, 0);
        assert_eq!(codex.tool_calls.quality, "structured_event_derived");

        let legacy = report
            .aggregated_data
            .iter()
            .find(|d| d.dimension_value == "legacy")
            .expect("legacy bucket");
        assert_eq!(legacy.attempts, 1);
        // Unknown must not be summed as zero.
        assert_eq!(legacy.tool_calls.total, 0);
        assert_eq!(legacy.tool_calls.known_attempts, 0);
        assert_eq!(legacy.tool_calls.unknown_attempts, 1);
    }

    #[test]
    fn failed_and_fallback_attempts_preserved_independently() {
        let mut entry = create_test_ledger_entry();
        let mut failed = attempt_with_behavior(1, "codex", structured_metrics(1, 0, 0, 0));
        failed.exit_code = Some(1);
        failed.validation_result = Some("fail".to_string());
        failed.failure_class = Some("BackendError".to_string());
        let mut fallback = attempt_with_behavior(2, "codex", structured_metrics(2, 1, 1, 0));
        fallback.usage = LedgerUsage {
            behavior_metrics: Some(structured_metrics(2, 1, 1, 0)),
            ..LedgerUsage::default()
        };
        entry.attempts = vec![failed, fallback];

        let params = AggregationParams {
            dimensions: vec![AggregationDimension::Backend],
            since: None,
            until: None,
            profile: None,
            include_failed_attempts: true,
            include_retried_attempts: true,
            project: None,
            ticket: None,
            execution_type: None,
            backend_instance: None,
            provider: None,
            model: None,
            account: None,
        };

        let aggregated = aggregate_by_dimension(&[entry], AggregationDimension::Backend, &params);
        let report = TelemetryReport {
            report_type: String::new(),
            generated_at: String::new(),
            time_range: None,
            profile: None,
            total_entries: 0,
            total_attempts: 0,
            successful_attempts: 0,
            failed_attempts: 0,
            total_cost_usd: 0.0,
            quota_backed_cost_usd: 0.0,
            api_cost_usd: 0.0,
            aggregated_data: aggregated,
        };
        let codex = report
            .aggregated_data
            .iter()
            .find(|d| d.dimension_value == "codex")
            .expect("codex bucket");
        // Both attempts counted separately, not collapsed.
        assert_eq!(codex.attempts, 2);
        assert_eq!(codex.failed_attempts, 1);
        assert_eq!(codex.tool_calls.total, 3);
        assert_eq!(codex.tool_calls.known_attempts, 2);
    }

    #[test]
    fn telemetry_record_serializes_behavior_metrics_with_provenance() {
        let mut entry = create_test_ledger_entry();
        entry.attempts = vec![attempt_with_behavior(
            1,
            "codex",
            structured_metrics(5, 2, 3, 1),
        )];
        let records = extract_attempt_usage_records(&entry, "2026-07-10T00:00:00Z");
        let exported = ExportedTelemetryRecord::AttemptUsage(Box::new(records[0].clone()));
        let json = serde_json::to_string(&exported).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["data"]["tool_calls"]["count"], 5);
        assert_eq!(
            parsed["data"]["tool_calls"]["quality"],
            "structured_event_derived"
        );
        assert_eq!(parsed["data"]["file_edits"]["count"], 3);
    }

    #[test]
    fn legacy_ledger_entry_without_behavior_metrics_deserializes_unknown() {
        // Historical JSONL line written before behavior_metrics existed.
        let raw = r#"{
            "schema_version": 3,
            "timestamp": "2026-07-10T00:00:00Z",
            "profile": "test-profile",
            "display_name": "Test Profile",
            "repo_id": "test-repo",
            "repo": "test/repo",
            "local_path": "/tmp",
            "provider": "github",
            "backend": "codex",
            "requested_backend": "codex",
            "effective_backend": "codex",
            "mode": "fix",
            "commit_attempted": false,
            "commit_created": false,
            "push_attempted": false,
            "push_succeeded": false,
            "mr_attempted": false,
            "mr_created": false,
            "fallback_used": false,
            "human_required": false,
            "attempts": [{"attempt_number":1,"backend":"codex","usage":{}}],
            "usage": {}
        }"#;
        let legacy: LedgerEntry = serde_json::from_str(raw).unwrap();
        assert!(legacy.attempts[0].usage.behavior_metrics.is_none());
        let records = extract_attempt_usage_records(&legacy, "2026-07-10T00:00:00Z");
        assert!(records[0].tool_calls.is_none());
    }

    #[test]
    fn aggregated_behavior_metric_defaults_are_zeroed() {
        // Guard that a fresh aggregate never reports phantom known counts.
        let m = AggregatedBehaviorMetric::default();
        assert_eq!(m.total, 0);
        assert_eq!(m.known_attempts, 0);
        assert_eq!(m.unknown_attempts, 0);
        assert_eq!(m.quality, "");
        let _ = SCHEMA_VERSION;
    }
}
