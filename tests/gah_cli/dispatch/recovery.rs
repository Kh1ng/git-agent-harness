use crate::*;

#[test]
fn direct_dispatch_reconciles_an_abandoned_prior_run() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = []\n");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");
    fs::write(
        &events_path,
        r##"{"timestamp":"2026-01-01T00:00:00Z","event_type":"dispatch_started","profile":"real","work_id":"#1112","run_id":"orphaned-run","details":"dispatch: #1112"}
"##,
    )
    .unwrap();

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "codex",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "noop",
            "--dry-run",
            "--skip-validation-gate",
        ])
        .env("HOME", &home)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();

    let events = fs::read_to_string(events_path).unwrap();
    assert!(events.lines().any(|line| {
        let event: Value = serde_json::from_str(line).unwrap();
        event["run_id"] == "orphaned-run"
            && event["event_type"] == "dispatch_finished"
            && event["details"]
                .as_str()
                .is_some_and(|details| details.contains("abandoned"))
    }));
}
