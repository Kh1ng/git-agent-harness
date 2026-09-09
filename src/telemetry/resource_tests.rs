use super::*;
use crate::ledger::{AttemptRecord, ProcessResources};

fn params() -> AggregationParams {
    AggregationParams {
        dimensions: vec![],
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
    }
}

#[test]
fn resource_reports_preserve_unknowns_and_aggregate_every_dimension() {
    let mut entry = super::tests::telemetry_tests::create_test_ledger_entry();
    entry.attempts = vec![
        AttemptRecord {
            attempt_number: 1,
            backend: "claude".into(),
            effective_model: Some("fixture".into()),
            resources: ProcessResources {
                cpu_seconds: Some(1.25),
                peak_rss_bytes: Some(4096),
                source: "linux_procfs_sampled_lower_bound".into(),
                unknown_reason: None,
            },
            exit_code: Some(7),
            ..Default::default()
        },
        AttemptRecord {
            attempt_number: 2,
            backend: "claude".into(),
            effective_model: Some("fixture".into()),
            resources: ProcessResources {
                cpu_seconds: Some(2.5),
                peak_rss_bytes: Some(2048),
                source: "linux_procfs_sampled_lower_bound".into(),
                unknown_reason: None,
            },
            exit_code: Some(-2),
            ..Default::default()
        },
        AttemptRecord {
            attempt_number: 3,
            backend: "claude".into(),
            effective_model: Some("fixture".into()),
            resources: ProcessResources::unknown("unsupported_platform"),
            ..Default::default()
        },
    ];
    for dimension in [
        AggregationDimension::Project,
        AggregationDimension::Ticket,
        AggregationDimension::ExecutionType,
        AggregationDimension::BackendInstance,
        AggregationDimension::Model,
        AggregationDimension::Date,
        AggregationDimension::DateRange,
    ] {
        let groups = aggregate_by_dimension(&[entry.clone()], dimension, &params());
        let resources = &groups[0].resources;
        assert_eq!(resources.cpu_seconds, Some(3.75));
        assert_eq!(resources.peak_rss_bytes, Some(4096));
        assert_eq!(resources.cpu_known_attempts, 2);
        assert_eq!(resources.rss_known_attempts, 2);
        assert_eq!(resources.unknown_attempts, 1);
        assert_eq!(resources.unknown_reasons["unsupported_platform"], 1);
    }
    let outcomes =
        aggregate_by_dimension(&[entry.clone()], AggregationDimension::Outcome, &params());
    assert_eq!(
        outcomes
            .iter()
            .map(|g| g.dimension_value.as_str())
            .collect::<Vec<_>>(),
        ["cancelled", "failed", "unknown"]
    );
    assert_eq!(outcomes[2].resources.cpu_seconds, None);
    assert_eq!(outcomes[2].resources.peak_rss_bytes, None);
    let exported = extractor::extract_attempt_usage_records(&entry, "2026-09-09T00:00:00Z");
    assert_eq!(exported[0].resources, entry.attempts[0].resources);
    assert_eq!(exported[2].resources.cpu_seconds, None);
    let mut historical = serde_json::to_value(&entry).unwrap();
    historical["attempts"][0]
        .as_object_mut()
        .unwrap()
        .remove("resources");
    let historical: LedgerEntry = serde_json::from_value(historical).unwrap();
    assert_eq!(
        historical.attempts[0].resources,
        ProcessResources::default()
    );
    let mut bounded = params();
    bounded.since = Some("2100-01-01".into());
    assert!(filter_entries_for_aggregation(&[entry], &bounded)
        .unwrap()
        .is_empty());
}
