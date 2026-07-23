use super::*;

#[test]
fn telemetry_status_reports_missing_repository() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("telemetry");

    bin()
        .args([
            "telemetry",
            "status",
            "--telemetry-repo-path",
            repo.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            format!("Telemetry repository not found at: {}", repo.display()).as_str(),
        ));
}

#[test]
fn telemetry_aggregate_json_reports_zero_for_empty_ledger() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    fs::write(&ledger_path, "").unwrap();
    let out = bin()
        .args([
            "telemetry",
            "aggregate",
            "--dimensions",
            "backend",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_LEDGER_PATH", ledger_path.to_str().unwrap())
        .assert()
        .success();

    let payload: Value = serde_json::from_slice(&out.get_output().stdout)
        .expect("telemetry aggregate output must be JSON");
    assert_eq!(payload["total_entries"], 0);
    assert_eq!(payload["total_attempts"], 0);
    assert_eq!(payload["aggregated_data"].as_array().unwrap().len(), 0);
}
