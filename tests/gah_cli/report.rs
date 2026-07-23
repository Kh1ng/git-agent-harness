use super::*;

#[test]
fn report_json_is_supported_with_empty_ledger() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    fs::write(&ledger_path, "").unwrap();
    let out = bin()
        .args([
            "report",
            "--json",
            "--since",
            "365d",
            "--group-by",
            "backend",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_LEDGER_PATH", ledger_path.to_str().unwrap())
        .assert()
        .success();

    let payload: Value =
        serde_json::from_slice(&out.get_output().stdout).expect("report output must be JSON");
    assert_eq!(payload["total_entries"], 0);
    assert_eq!(payload["comparisons"].as_array().unwrap().len(), 0);
}

#[test]
fn report_series_json_emits_empty_series_for_empty_ledger() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    fs::write(&ledger_path, "").unwrap();
    let out = bin()
        .args([
            "report",
            "--json",
            "--series",
            "--bucket",
            "daily",
            "--since",
            "365d",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_LEDGER_PATH", ledger_path.to_str().unwrap())
        .assert()
        .success();

    let payload: Value = serde_json::from_slice(&out.get_output().stdout)
        .expect("report series output must be JSON");
    assert_eq!(payload["bucket"], "daily");
    assert_eq!(payload["series"].as_array().unwrap().len(), 0);
}
