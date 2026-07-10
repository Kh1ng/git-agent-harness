//! Telemetry Module Tests
//!
//! Comprehensive tests for the telemetry export functionality.

use super::exporter::*;
use super::extractor::*;
use super::records::*;
use crate::ledger::LedgerEntry;
use tempfile::tempdir;

#[cfg(test)]
mod telemetry_tests {
    use super::*;

    #[test]
    fn test_partition_key_parsing() {
        let key = PartitionKey::from_date_str("2026-07-10T12:34:56Z").unwrap();
        assert_eq!(key.year, 2026);
        assert_eq!(key.month, 7);
        assert_eq!(key.day, 10);
        assert_eq!(key.to_path_string(), "2026/07/10");
        assert_eq!(key.to_date_string(), "2026-07-10");
    }

    #[test]
    fn test_partition_key_from_date_only() {
        let key = PartitionKey::from_date_str("2026-07-10").unwrap();
        assert_eq!(key.year, 2026);
        assert_eq!(key.month, 7);
        assert_eq!(key.day, 10);
        assert_eq!(key.to_date_string(), "2026-07-10");
    }

    #[test]
    fn test_generate_record_ids() {
        let id1 = generate_attempt_usage_id(
            "2026-07-10T12:00:00Z",
            Some("work-123"),
            1,
            "backend-a",
            Some("model-x"),
        );
        let id2 = generate_attempt_usage_id(
            "2026-07-10T12:00:00Z",
            Some("work-123"),
            1,
            "backend-a",
            Some("model-x"),
        );

        // Same inputs should produce same ID
        assert_eq!(id1, id2);

        // Different attempt number should produce different ID
        let id3 = generate_attempt_usage_id(
            "2026-07-10T12:00:00Z",
            Some("work-123"),
            2,
            "backend-a",
            Some("model-x"),
        );
        assert_ne!(id1, id3);

        // Different work ID should produce different ID
        let id4 = generate_attempt_usage_id(
            "2026-07-10T12:00:00Z",
            Some("work-456"),
            1,
            "backend-a",
            Some("model-x"),
        );
        assert_ne!(id1, id4);
    }

    #[test]
    fn test_schema_version_in_record() {
        let base = TelemetryRecord {
            schema_version: SCHEMA_VERSION,
            record_id: "test-123".to_string(),
            exported_at: "2026-07-10T12:00:00Z".to_string(),
            observed_at: "2026-07-10T12:00:00Z".to_string(),
        };

        let record = AttemptUsageRecord {
            base: base.clone(),
            profile: "test".to_string(),
            repo_id: "test".to_string(),
            repo: "test".to_string(),
            provider: "test".to_string(),
            work_id: None,
            target_summary: None,
            mode: "test".to_string(),
            attempt_number: 0,
            backend: "test".to_string(),
            effective_backend: "test".to_string(),
            requested_backend: "test".to_string(),
            effective_model: None,
            requested_model: None,
            exit_code: None,
            duration_seconds: None,
            validation_result: None,
            failure_class: None,
            failure_stage: None,
            fallback_used: false,
            human_required: false,
            routing_reason: None,
            usage_source: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: None,
            requests_count: None,
            estimated_cost_usd: None,
            actual_cost_usd: None,
            quota_window: None,
            quota_used_percent: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
        };

        let exported = ExportedTelemetryRecord::AttemptUsage(record);
        let json = serde_json::to_string(&exported).unwrap();
        // println!("JSON: {}", json); // Debug output
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Check that schema_version is present (inside data due to enum tag)
        assert_eq!(parsed["record_type"], "attempt_usage");
        assert_eq!(parsed["data"]["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn test_telemetry_record_serialization() {
        let base = TelemetryRecord {
            schema_version: SCHEMA_VERSION,
            record_id: "test-task-123".to_string(),
            exported_at: "2026-07-10T12:00:00Z".to_string(),
            observed_at: "2026-07-10T12:00:00Z".to_string(),
        };

        let record = TaskOutcomeRecord {
            base,
            profile: "test-profile".to_string(),
            repo_id: "test-repo".to_string(),
            repo: "test/repo".to_string(),
            provider: "github".to_string(),
            work_id: "work-123".to_string(),
            target_summary: Some("test target".to_string()),
            mode: "fix".to_string(),
            branch: Some("main".to_string()),
            dispatch_reason: None,
            attempts_started: 1,
            attempts_completed: 1,
            duration_seconds: Some(100.0),
            backend_exit_code: Some(0),
            validation_result: Some("pass".to_string()),
            review_verdict: None,
            review_confidence: None,
            reviewer_backend: None,
            reviewer_model: None,
            commit_attempted: true,
            commit_created: true,
            push_attempted: true,
            push_succeeded: true,
            mr_attempted: false,
            mr_created: false,
            mr_url: None,
            files_changed: Some(5),
            insertions: Some(100),
            deletions: Some(50),
            failure_class: None,
            failure_stage: None,
            error_summary: None,
            usage_source: None,
            input_tokens: Some(1000),
            output_tokens: Some(500),
            cache_read_tokens: Some(100),
            cache_write_tokens: Some(50),
            total_tokens: Some(1500),
            requests_count: Some(10),
            estimated_cost_usd: Some(0.1),
            actual_cost_usd: Some(0.15),
            final_outcome: Some("SUCCESS".to_string()),
            merge_status: None,
        };

        let exported = ExportedTelemetryRecord::TaskOutcome(record);
        let json = serde_json::to_string(&exported).unwrap();

        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("record_type").is_some());
        assert_eq!(parsed["record_type"], "task_outcome");

        // Check that the work_id is preserved
        assert_eq!(parsed["data"]["work_id"], "work-123");
        assert_eq!(parsed["data"]["mode"], "fix");
    }

    #[test]
    fn test_extract_telemetry_records_basic() {
        let entry = create_test_ledger_entry();
        let exported_at = "2026-07-10T13:00:00Z";
        let records = extract_telemetry_records(&entry, exported_at);

        // Should have at least task outcome record
        assert!(!records.is_empty());

        // Check that schema version is set correctly
        for record in &records {
            match record {
                ExportedTelemetryRecord::AttemptUsage(r) => {
                    assert_eq!(r.base.schema_version, SCHEMA_VERSION);
                }
                ExportedTelemetryRecord::QuotaObservation(r) => {
                    assert_eq!(r.base.schema_version, SCHEMA_VERSION);
                }
                ExportedTelemetryRecord::TaskOutcome(r) => {
                    assert_eq!(r.base.schema_version, SCHEMA_VERSION);
                }
                ExportedTelemetryRecord::ReviewOutcome(r) => {
                    assert_eq!(r.base.schema_version, SCHEMA_VERSION);
                }
            }
        }
    }

    #[test]
    fn test_unknown_values_remain_unknown() {
        let entry = create_test_ledger_entry();
        // Modify the entry to have unknown values
        let mut entry = entry;
        entry.requested_model = None;
        entry.effective_model = None;
        entry.duration_seconds = None;
        entry.backend_exit_code = None;
        entry.validation_result = None;
        entry.usage.input_tokens = None;
        entry.usage.output_tokens = None;
        entry.usage.estimated_cost_usd = None;
        entry.usage.actual_cost_usd = None;

        let exported_at = "2026-07-10T13:00:00Z";
        let records = extract_telemetry_records(&entry, exported_at);

        for record in &records {
            if let ExportedTelemetryRecord::TaskOutcome(r) = record {
                // Check that unknown values remain None
                assert!(r.duration_seconds.is_none());
                assert!(r.backend_exit_code.is_none());
                assert!(r.validation_result.is_none());
                assert!(r.input_tokens.is_none());
                assert!(r.output_tokens.is_none());
                assert!(r.estimated_cost_usd.is_none());
                assert!(r.actual_cost_usd.is_none());
            }
        }
    }

    #[test]
    fn test_no_double_counting_of_usage() {
        use crate::ledger::{AttemptRecord, LedgerUsage};

        let _attempt = AttemptRecord {
            attempt_number: 1,
            backend: "test-backend".to_string(),
            effective_model: Some("test-model".to_string()),
            exit_code: Some(0),
            validation_result: Some("pass".to_string()),
            failure_class: None,
            failure_stage: None,
            duration_seconds: Some(50.0),
            diff_path: None,
            usage: LedgerUsage {
                usage_source: Some("attempt".to_string()),
                observed_at: Some("2026-07-10T12:30:00Z".to_string()),
                input_tokens: Some(500),
                output_tokens: Some(250),
                cache_read_tokens: Some(50),
                cache_write_tokens: Some(25),
                total_tokens: Some(750),
                requests_count: Some(5),
                estimated_cost_usd: Some(0.05),
                actual_cost_usd: Some(0.06),
                quota_window: None,
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: None,
            },
        };

        let mut entry = create_test_ledger_entry();
        // Add an attempt with different usage
        let attempt = AttemptRecord {
            attempt_number: 1,
            backend: "test-backend".to_string(),
            effective_model: Some("test-model".to_string()),
            exit_code: Some(0),
            validation_result: Some("pass".to_string()),
            failure_class: None,
            failure_stage: None,
            duration_seconds: Some(50.0),
            diff_path: None,
            usage: LedgerUsage {
                usage_source: Some("attempt".to_string()),
                observed_at: Some("2026-07-10T12:30:00Z".to_string()),
                input_tokens: Some(500),
                output_tokens: Some(250),
                cache_read_tokens: Some(50),
                cache_write_tokens: Some(25),
                total_tokens: Some(750),
                requests_count: Some(5),
                estimated_cost_usd: Some(0.05),
                actual_cost_usd: Some(0.06),
                quota_window: None,
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: None,
            },
        };
        entry.attempts = vec![attempt];
        // Update entry-level usage to be different from attempt
        entry.usage = LedgerUsage {
            usage_source: Some("entry".to_string()),
            observed_at: Some("2026-07-10T12:00:00Z".to_string()),
            input_tokens: Some(1000), // Different from attempt
            output_tokens: Some(500),
            cache_read_tokens: Some(100),
            cache_write_tokens: Some(50),
            total_tokens: Some(1500),
            requests_count: Some(10),
            estimated_cost_usd: Some(0.10),
            actual_cost_usd: Some(0.12),
            quota_window: None,
            quota_used_percent: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
        };

        let exported_at = "2026-07-10T13:00:00Z";
        let records = extract_telemetry_records(&entry, exported_at);

        // Count attempt usage records vs task outcome records
        let attempt_usage_count = records
            .iter()
            .filter(|r| matches!(r, ExportedTelemetryRecord::AttemptUsage(_)))
            .count();
        let task_outcome_count = records
            .iter()
            .filter(|r| matches!(r, ExportedTelemetryRecord::TaskOutcome(_)))
            .count();

        // Should have 1 attempt usage record and 1 task outcome record
        assert!(attempt_usage_count >= 1);
        assert!(task_outcome_count >= 1);

        // Check that the task outcome record doesn't include the attempt's specific usage
        for record in &records {
            if let ExportedTelemetryRecord::TaskOutcome(task_record) = record {
                // Task outcome should have the entry-level usage, not the attempt-level
                assert_eq!(task_record.input_tokens, Some(1000));
                assert_ne!(task_record.input_tokens, Some(500)); // Not the attempt's tokens
            }
        }
    }

    #[test]
    fn test_export_idempotency() {
        let entry = create_test_ledger_entry();
        let exported_at = "2026-07-10T13:00:00Z";
        let records1 = extract_telemetry_records(&entry, exported_at);
        let records2 = extract_telemetry_records(&entry, exported_at);

        // Same entry should produce same records with same IDs
        assert_eq!(records1.len(), records2.len());

        for (r1, r2) in records1.iter().zip(records2.iter()) {
            assert_eq!(r1.get_id(), r2.get_id());
        }
    }

    #[test]
    fn test_final_outcome_determination() {
        // Test APPROVE_STRONG
        let mut entry = create_test_ledger_entry();
        entry.review_verdict = Some("APPROVE_STRONG".to_string());
        let outcome = determine_final_outcome(&entry);
        assert_eq!(outcome, Some("APPROVE_STRONG".to_string()));

        // Test APPROVE_WEAK
        entry.review_verdict = Some("APPROVE_WEAK".to_string());
        let outcome = determine_final_outcome(&entry);
        assert_eq!(outcome, Some("APPROVE_WEAK".to_string()));

        // Test NEEDS_FIX
        entry.review_verdict = Some("NEEDS_FIX".to_string());
        let outcome = determine_final_outcome(&entry);
        assert_eq!(outcome, Some("NEEDS_FIX".to_string()));

        // Test validation passed
        entry.review_verdict = None;
        entry.validation_result = Some("pass".to_string());
        entry.commit_created = false;
        entry.push_succeeded = false;
        let outcome = determine_final_outcome(&entry);
        assert_eq!(outcome, Some("VALIDATION_PASSED".to_string()));

        // Test success
        entry.validation_result = None;
        entry.backend_exit_code = Some(0);
        let outcome = determine_final_outcome(&entry);
        assert_eq!(outcome, Some("SUCCESS".to_string()));
    }

    #[test]
    fn test_telemetry_exporter_basic() {
        let temp_dir = tempdir().unwrap();
        let telemetry_path = temp_dir.path().join("telemetry");

        let config = TelemetryConfig {
            telemetry_repo_path: telemetry_path.clone(),
            format: ExportFormat::Jsonl,
            generate_manifests: false,
            commit_batch_size: None,
        };

        let mut exporter = TelemetryExporter::new(config).unwrap();

        // Test with empty entries
        let entries: Vec<LedgerEntry> = vec![];
        exporter.export_from_entries(&entries).unwrap();

        assert_eq!(exporter.records_exported(), 0);

        // Test with one entry
        let entry = create_test_ledger_entry();
        exporter.export_from_entries(&[entry]).unwrap();

        // Should have exported some records
        assert!(exporter.records_exported() > 0);

        // Verify repository structure was created
        assert!(telemetry_path.join("raw").join("usage").exists());
        assert!(telemetry_path.join("raw").join("quota").exists());
        assert!(telemetry_path.join("raw").join("outcomes").exists());
        assert!(telemetry_path.join("exports").exists());
        assert!(telemetry_path.join("manifests").exists());
        assert!(telemetry_path.join("schemas").exists());
    }

    #[test]
    fn test_deduplication() {
        let temp_dir = tempdir().unwrap();
        let telemetry_path = temp_dir.path().join("telemetry");

        let config = TelemetryConfig {
            telemetry_repo_path: telemetry_path.clone(),
            format: ExportFormat::Jsonl,
            generate_manifests: false,
            commit_batch_size: None,
        };

        let mut exporter = TelemetryExporter::new(config).unwrap();

        // Create test entries
        let entry = create_test_ledger_entry();

        // First export
        exporter
            .export_from_entries(std::slice::from_ref(&entry))
            .unwrap();
        let first_export_count = exporter.records_exported();
        assert!(first_export_count > 0);

        // Second export of the same entry should not export duplicates
        exporter.export_from_entries(&[entry]).unwrap();
        let second_export_count = exporter.records_exported();

        // Should be the same count (no new records exported)
        assert_eq!(first_export_count, second_export_count);
    }

    #[test]
    fn test_manifest_generation() {
        let temp_dir = tempdir().unwrap();
        let telemetry_path = temp_dir.path().join("telemetry");

        let config = TelemetryConfig {
            telemetry_repo_path: telemetry_path.clone(),
            format: ExportFormat::Jsonl,
            generate_manifests: true,
            commit_batch_size: None,
        };

        let mut exporter = TelemetryExporter::new(config).unwrap();

        // Export some entries
        let entry = create_test_ledger_entry();
        exporter.export_from_entries(&[entry]).unwrap();

        // Check that manifests directory was created and has files
        let manifests_dir = telemetry_path.join("manifests");
        assert!(manifests_dir.exists());

        // Count manifest files
        let manifest_files: Vec<_> = std::fs::read_dir(&manifests_dir)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                if entry
                    .path()
                    .extension()
                    .map(|e| e == "json")
                    .unwrap_or(false)
                {
                    Some(entry.path())
                } else {
                    None
                }
            })
            .collect();

        // Should have at least one manifest file
        assert!(!manifest_files.is_empty());
    }

    // Helper function to create a test ledger entry
    fn create_test_ledger_entry() -> LedgerEntry {
        LedgerEntry {
            timestamp: "2026-07-10T12:00:00Z".to_string(),
            session_id: None,
            profile: "test-profile".to_string(),
            display_name: "Test Profile".to_string(),
            repo_id: "test-repo".to_string(),
            repo: "test/repo".to_string(),
            local_path: "/tmp/test/repo".to_string(),
            provider: "github".to_string(),
            backend: "test-backend".to_string(),
            requested_backend: "test-backend".to_string(),
            effective_backend: "test-backend".to_string(),
            requested_model: Some("test-model".to_string()),
            effective_model: Some("test-model".to_string()),
            routing_reason: None,
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
            mode: "fix".to_string(),
            target_summary: Some("test target".to_string()),
            work_id: Some("work-123".to_string()),
            source_issue_number: None,
            work_title: None,
            branch: Some("main".to_string()),
            session_dir: None,
            duration_seconds: Some(100.0),
            backend_exit_code: Some(0),
            validation_result: Some("pass".to_string()),
            review_verdict: None,
            review_confidence: None,
            reviewer_backend: None,
            reviewer_model: None,
            commit_attempted: true,
            commit_created: true,
            push_attempted: true,
            push_succeeded: true,
            mr_attempted: false,
            mr_created: false,
            mr_url: None,
            files_changed: Some(5),
            insertions: Some(100),
            deletions: Some(50),
            error_summary: None,
            failure_class: None,
            failure_stage: None,
            attempts_started: 1,
            attempts_completed: 1,
            attempts: vec![],
            dispatch_reason: None,
            usage: crate::ledger::LedgerUsage::default(),
        }
    }
}
