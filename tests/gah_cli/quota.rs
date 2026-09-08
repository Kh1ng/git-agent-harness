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
fn quota_list_json_keeps_missing_store_empty() {
    let tmp = test_tempdir();
    bin()
        .args(["quota", "list", "--json", "--store-path"])
        .arg(tmp.path().join("missing.jsonl"))
        .assert()
        .success()
        .stdout("[]\n");
}

#[test]
fn quota_list_json_preserves_valid_records_around_malformed_lines() {
    let tmp = test_tempdir();
    let store = tmp.path().join("quota.jsonl");
    fs::write(
        &store,
        "{\"backend\":\"codex\",\"quota_used_percent\":25.0}\nmalformed\n{\"backend\":\"claude\"}\n",
    )
    .unwrap();
    let output = bin()
        .args(["quota", "list", "--json", "--store-path"])
        .arg(store)
        .assert()
        .success();
    let records: Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(
        records,
        serde_json::json!([
            { "backend": "codex", "quota_used_percent": 25.0 },
            { "backend": "claude" }
        ])
    );
}

#[test]
fn quota_list_reports_store_read_failures_without_empty_success_output() {
    let tmp = test_tempdir();
    let invalid_utf8 = tmp.path().join("invalid-utf8.jsonl");
    fs::write(&invalid_utf8, [0xff]).unwrap();
    let mut paths = vec![tmp.path().to_path_buf(), invalid_utf8.clone()];
    // `Path::exists()` reports false for ENOTDIR, just as it does for some
    // permission errors. Reading this path must preserve the actual failure.
    #[cfg(unix)]
    paths.push(invalid_utf8.join("child.jsonl"));
    for path in paths {
        for json in [false, true] {
            let mut command = bin();
            command.args(["quota", "list", "--store-path"]).arg(&path);
            if json {
                command.arg("--json");
            }
            command
                .assert()
                .failure()
                .stdout("")
                .stderr(predicate::str::contains("read quota store"));
        }
    }
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
