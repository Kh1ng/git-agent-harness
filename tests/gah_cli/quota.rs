use super::*;

#[test]
fn quota_list_json_reads_existing_store_records() {
    let tmp = test_tempdir();
    let store_path = tmp.path().join("quota-observations.jsonl");
    fs::write(
        &store_path,
        r#"{"backend":"codex","model":"gpt-5","quota_window":"weekly","quota_used_percent":25.0,"quota_remaining_percent":75.0,"quota_reset_at":"2026-07-20T00:00:00Z","observed_at":"2026-07-19T00:00:00Z","usage_source":"codex_status"}
"#,
    )
    .unwrap();

    let out = bin()
        .args([
            "quota",
            "list",
            "--json",
            "--store-path",
            store_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let parsed: Value =
        serde_json::from_slice(&out.get_output().stdout).expect("quota list output must be JSON");
    let records = parsed.as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["backend"], "codex");
}

#[test]
fn quota_refresh_rejects_quota_pool_without_instance() {
    bin()
        .args([
            "quota",
            "refresh",
            "--backend",
            "codex",
            "--quota-pool",
            "team-shared-quota",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--quota-pool requires --backend-instance for an unambiguous quota observation",
        ));
}
