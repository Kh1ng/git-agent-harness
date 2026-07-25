use super::*;

#[test]
fn ledger_summary_json_outputs_machine_readable_counts() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    fs::write(&ledger_path, "").unwrap();
    let out = bin()
        .args([
            "ledger",
            "summary",
            "--since",
            "7d",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_LEDGER_PATH", ledger_path.to_str().unwrap())
        .assert()
        .success();
    let parsed: Value =
        serde_json::from_slice(&out.get_output().stdout).expect("summary output must be JSON");
    assert_eq!(parsed["entries"], 0);
    assert!(parsed["by_mode"].is_object());
}

#[test]
fn ledger_work_no_matching_entry_reports_empty_result() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    fs::write(&ledger_path, "").unwrap();
    bin()
        .args([
            "ledger",
            "work",
            "TICKET-999",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_LEDGER_PATH", ledger_path.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "No ledger entries found for work item 'TICKET-999'.",
        ));
}

#[test]
fn ledger_clear_attempts_dry_run_stays_pretended_stateful() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    fs::write(&ledger_path, "").unwrap();
    bin()
        .args([
            "ledger",
            "clear-attempts",
            "--profile",
            "real",
            "--dry-run",
            "TICKET-999",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_LEDGER_PATH", ledger_path.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Dry run: would append tombstone entry for work_id 'TICKET-999':",
        ));
}
