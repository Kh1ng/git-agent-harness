use super::*;

#[test]
fn quota_snapshot_signals_the_v3_source_check_contract() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");
    let output = bin()
        .args([
            "quota",
            "snapshot",
            "--profile",
            "real",
            "--json",
            "--config-path",
        ])
        .arg(cfg)
        .assert()
        .success();
    let snapshot: Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(snapshot["schema_version"], 3);
}

#[test]
fn native_claude_refresh_cannot_attribute_default_login_to_another_account() {
    let tmp = test_tempdir();
    let path = tmp.path().join("quota.jsonl");
    for arguments in [
        ["--backend-instance", "claude-second"],
        ["--model", "other-model"],
        ["--command", "other-login"],
    ] {
        bin()
            .args(["quota", "refresh", "--backend", "claude", "--store-path"])
            .arg(&path)
            .args(arguments)
            .assert()
            .failure()
            .stderr(predicate::str::contains("current native OAuth login"));
        assert!(!path.exists());
    }
}

#[test]
fn quota_record_persists_validated_account_observation_from_stdin() {
    let tmp = test_tempdir();
    let path = tmp.path().join("quota.jsonl");
    bin().args(["quota", "record", "--store-path"]).arg(&path)
        .write_stdin(r#"{"backend":"agy","backend_instance":"agy-1","quota_remaining_percent":42,"observed_at":"2026-10-02T23:00:00Z","checked_at":"2026-10-02T23:00:00Z","usage_source":"cli_router"}"#)
        .assert().success();
    let recorded: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(recorded["backend_instance"], "agy-1");
    assert_eq!(recorded["quota_remaining_percent"], 42.0);
    bin().args(["quota", "record", "--store-path"]).arg(&path)
        .write_stdin(r#"{"backend":"agy","backend_instance":"agy-1","quota_remaining_percent":142,"checked_at":"2026-10-02T23:00:00Z","usage_source":"cli_router"}"#)
        .assert().failure();
    assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
}

/// `gah quota record` accepts a collector that still reports used percent,
/// stores remaining only, and rejects a used value that is not a percentage
/// even when a valid remaining value is sent with it.
#[test]
fn quota_record_normalizes_used_percent_and_rejects_one_out_of_range() {
    let tmp = test_tempdir();
    let path = tmp.path().join("quota.jsonl");
    let payload = |percents: &str| {
        format!(
            r#"{{"backend":"agy","backend_instance":"agy-1",{percents},"observed_at":"2026-10-02T23:00:00Z","checked_at":"2026-10-02T23:00:00Z","usage_source":"cli_router"}}"#
        )
    };
    bin()
        .args(["quota", "record", "--store-path"])
        .arg(&path)
        .write_stdin(payload(r#""quota_used_percent":25"#))
        .assert()
        .success();
    let recorded: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(recorded["quota_remaining_percent"], 75.0);
    assert!(recorded.get("quota_used_percent").is_none());

    for percents in [
        r#""quota_used_percent":500"#,
        r#""quota_used_percent":500,"quota_remaining_percent":40"#,
    ] {
        bin()
            .args(["quota", "record", "--store-path"])
            .arg(&path)
            .write_stdin(payload(percents))
            .assert()
            .failure()
            .stderr(predicate::str::contains("between 0 and 100"));
    }
    assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
}

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
            { "backend": "codex", "quota_remaining_percent": 75.0 },
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
