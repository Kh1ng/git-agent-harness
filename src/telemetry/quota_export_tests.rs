//! #1341: quota observation export from the durable account quota store.
//!
//! Telemetry export derives quota records from the account quota store,
//! not from per-attempt ledger usage. These tests pin the exported shape,
//! per-window record granularity, cross-export idempotency, and the
//! absence of secret-bearing fields.

use super::exporter::*;
use super::extractor::*;
use super::records::*;
use tempfile::tempdir;

/// #1341: build a store record with all identity fields so the tests
/// cover the exact exported shape.
#[allow(clippy::too_many_arguments)]
fn store_record(
    backend: &str,
    backend_instance: Option<&str>,
    credential_id: Option<&str>,
    window: Option<&str>,
    remaining_percent: Option<f64>,
    observed_at: Option<&str>,
    checked_at: Option<&str>,
    check_error: Option<&str>,
) -> crate::quota_store::QuotaObservationRecord {
    crate::quota_store::QuotaObservationRecord {
        backend: backend.to_string(),
        backend_instance: backend_instance.map(str::to_string),
        credential_id: credential_id.map(str::to_string),
        model: None,
        quota_pool: None,
        quota_window: window.map(str::to_string),
        quota_remaining_percent: remaining_percent,
        quota_reset_at: None,
        observed_at: observed_at.map(str::to_string),
        checked_at: checked_at.map(str::to_string),
        check_error: check_error.map(str::to_string),
        usage_source: Some("claude_native".to_string()),
        mistral_admin: None,
        account_usage: None,
    }
}

/// #1341 acceptance: a store with two windows for one identity exports
/// two records, and re-exporting is idempotent.
#[test]
fn store_quota_observations_export_two_windows_and_reexport_is_idempotent() {
    let store = vec![
        store_record(
            "claude",
            Some("claude"),
            None,
            Some("five-hour"),
            Some(40.0),
            Some("2026-10-04T08:00:00Z"),
            Some("2026-10-04T08:00:00Z"),
            None,
        ),
        store_record(
            "claude",
            Some("claude"),
            None,
            Some("weekly"),
            Some(90.0),
            Some("2026-10-04T08:00:00Z"),
            Some("2026-10-04T08:00:00Z"),
            None,
        ),
    ];

    let records = extract_quota_observation_records(&store, "2026-10-04T09:00:00Z");
    assert_eq!(records.len(), 2, "each window is its own record");
    let ids: std::collections::BTreeSet<_> = records
        .iter()
        .map(|record| record.base.record_id.clone())
        .collect();
    assert_eq!(ids.len(), 2, "record ids must not collide across windows");
    for record in &records {
        assert_eq!(record.base.schema_version, SCHEMA_VERSION);
        assert_eq!(record.backend, "claude");
        assert_eq!(record.backend_instance.as_deref(), Some("claude"));
    }

    let temp_dir = tempdir().unwrap();
    let telemetry_path = temp_dir.path().join("telemetry");
    let mut exporter = TelemetryExporter::new(TelemetryConfig {
        telemetry_repo_path: telemetry_path.clone(),
        format: ExportFormat::Jsonl,
        generate_manifests: true,
        commit_batch_size: None,
    })
    .unwrap();
    exporter.load_exported_ids().unwrap();
    exporter.export_store_quota_observations(&store).unwrap();
    assert_eq!(exporter.records_exported(), 2);

    // A fresh exporter reading the same repo re-exports nothing: ids in
    // the store records are deterministic across exports.
    let mut reexporter = TelemetryExporter::new(TelemetryConfig {
        telemetry_repo_path: telemetry_path.clone(),
        format: ExportFormat::Jsonl,
        generate_manifests: true,
        commit_batch_size: None,
    })
    .unwrap();
    reexporter.load_exported_ids().unwrap();
    reexporter.export_store_quota_observations(&store).unwrap();
    assert_eq!(
        reexporter.records_exported(),
        0,
        "re-exporting the same store must not duplicate records"
    );

    // The quota partition carries exactly the two records.
    let quota_dir = telemetry_path.join("raw").join("quota");
    let mut pending = vec![quota_dir];
    let mut lines = 0usize;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                lines += std::fs::read_to_string(&path)
                    .unwrap()
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count();
            }
        }
    }
    assert_eq!(lines, 2);
}

/// #1341 acceptance: no secret-bearing fields are exported. The exported
/// quota record exposes source identity labels and readings only.
#[test]
fn store_quota_observations_export_no_secret_bearing_fields() {
    let store = vec![store_record(
        "claude",
        Some("claude"),
        Some("cred-main"),
        Some("weekly"),
        Some(72.5),
        None,
        Some("2026-10-04T08:00:00Z"),
        Some("auth_required: Claude OAuth login expired; run claude auth login"),
    )];

    let records = extract_quota_observation_records(&store, "2026-10-04T09:00:00Z");
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.base.observed_at, "2026-10-04T08:00:00Z",
        "checked_at stands in when the provider reported no observation time"
    );
    assert_eq!(record.checked_at.as_deref(), Some("2026-10-04T08:00:00Z"));
    assert_eq!(record.credential_id.as_deref(), Some("cred-main"));

    let json = serde_json::to_value(record).unwrap();
    let object = json.as_object().unwrap();
    let serialized = serde_json::to_string(&json).unwrap();
    for forbidden in ["mistral_admin", "account_usage", "token", "api_key"] {
        assert!(
            !serialized.contains(forbidden),
            "exported quota record must not contain {forbidden}"
        );
    }
    // The identity and reading fields survive, and the exported tag is
    // the quota observation type.
    assert_eq!(object["backend"], "claude");
    assert_eq!(object["quota_remaining_percent"], 72.5);
    let wrapped = serde_json::to_value(ExportedTelemetryRecord::QuotaObservation(Box::new(
        record.clone(),
    )))
    .unwrap();
    assert_eq!(wrapped["record_type"], "quota_observation");
}

/// #1341: a failed check exports with its redacted error so consumers
/// can see why a source stopped reporting.
#[test]
fn store_quota_check_error_exports() {
    let store = vec![store_record(
        "vibe",
        Some("vibe"),
        None,
        None,
        None,
        None,
        Some("2026-10-04T08:00:00Z"),
        Some("auth_required: MISTRAL_ADMIN_API_KEY is not configured"),
    )];
    let records = extract_quota_observation_records(&store, "2026-10-04T09:00:00Z");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].check_error.as_deref(),
        Some("auth_required: MISTRAL_ADMIN_API_KEY is not configured")
    );
}
