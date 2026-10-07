use super::{load, parse_external_observation};

#[test]
fn legacy_null_remaining_percent_survives_store_and_external_loading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota.jsonl");
    let mut value = serde_json::json!({
        "backend": "codex",
        "backend_instance": "codex-primary",
        "observed_at": "2026-10-02T23:00:00Z",
        "checked_at": "2026-10-02T23:00:00Z",
        "usage_source": "cli_router",
        "quota_used_percent": 25,
        "quota_remaining_percent": null
    });
    for (used, remaining, expected) in [
        (Some(25.0), None, Some(75.0)),
        (Some(25.0), Some(60.0), Some(60.0)),
        (None, None, None),
    ] {
        value["quota_used_percent"] = serde_json::json!(used);
        value["quota_remaining_percent"] = serde_json::json!(remaining);
        let input = value.to_string();
        std::fs::write(&path, format!("{input}\n")).unwrap();
        let records = load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].quota_remaining_percent, expected);
        let external = parse_external_observation(&input).unwrap();
        assert_eq!(external.quota_remaining_percent, expected);
    }

    // A used percent that is not a percentage is not a reading. A stored row
    // keeps loading, with the balance unknown rather than negative; a new
    // submission is rejected even when a valid remaining percent rides along.
    for (used, remaining) in [(500.0, None), (-5.0, None), (500.0, Some(40.0))] {
        value["quota_used_percent"] = serde_json::json!(used);
        value["quota_remaining_percent"] = serde_json::json!(remaining);
        let input = value.to_string();
        std::fs::write(&path, format!("{input}\n")).unwrap();
        let records = load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].quota_remaining_percent, remaining);
        let error = parse_external_observation(&input).unwrap_err();
        assert!(error.to_string().contains("between 0 and 100"), "{error}");
    }
}
