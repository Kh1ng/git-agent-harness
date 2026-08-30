mod support;

use std::fs;
use std::process::Command;
use support::fake_ledger::{ledger_entry_full, TestLedger};
use support::scenario::ScenarioHarness;

#[test]
fn controller_retry_sends_prior_validation_tail_and_records_retry_reason() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("success");
    let backend_config = format!(
        "validation_commands = [\"true\"]\ncodex_path = \"{}\"\n[profiles.test.routing]\nimprove_backend = \"codex\"\n",
        harness.bin_dir.join("codex").display()
    );
    harness = harness.with_config_append(&backend_config);
    let ticket_path = harness
        .local_repo_dir
        .join("docs/tickets/TICKET-243-prior-attempts.md");
    fs::create_dir_all(ticket_path.parent().unwrap()).unwrap();
    fs::write(
        &ticket_path,
        "# TICKET-243: Preserve retry evidence\n\nGoal: exercise retry context.\n\nRecommended backend: codex\n",
    )
    .unwrap();
    for args in [
        vec!["add", "docs/tickets/TICKET-243-prior-attempts.md"],
        vec!["commit", "-m", "add retry fixture"],
        vec!["push", "-q", "origin", "main"],
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(&harness.local_repo_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git fixture setup failed: {output:?}"
        );
    }

    let prior_session = harness.artifacts_dir.join("sessions/prior-dispatch");
    let prior_attempt = prior_session.join("attempt-1");
    fs::create_dir_all(&prior_attempt).unwrap();
    fs::write(
        prior_attempt.join("validation-failure.txt"),
        "$ cargo test retry_context\nerror: retained integration validation tail\n",
    )
    .unwrap();
    let mut prior = ledger_entry_full(
        "fix",
        "gah/test-prior",
        Some("initial"),
        "TICKET-243",
        &chrono::Utc::now().to_rfc3339(),
    );
    prior["session_dir"] = serde_json::json!(prior_session.display().to_string());
    prior["failure_class"] = serde_json::json!("backend_error");
    prior["failure_stage"] = serde_json::json!("post_validation");
    prior["validation_result"] = serde_json::json!("failed");
    prior["error_summary"] = serde_json::json!("prior dispatch failed validation");
    prior["attempts"] = serde_json::json!([{
        "attempt_number": 1,
        "backend": "codex",
        "effective_model": null,
        "exit_code": 1,
        "validation_result": "failed",
        "failure_class": "backend_error",
        "failure_stage": "post_validation",
        "duration_seconds": 1.0,
        "diff_path": null,
        "cli_version": null,
        "checkpoint_branch": null,
        "checkpoint_sha": null,
        "usage": {}
    }]);
    harness = harness.with_ledger(TestLedger::new().with_entry(prior));
    let availability_path = harness
        .artifacts_dir
        .parent()
        .unwrap()
        .join("xdg-state/gah/availability.json");
    fs::create_dir_all(availability_path.parent().unwrap()).unwrap();
    fs::write(
        &availability_path,
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "records": [{
                "backend": "codex",
                "status": "available",
                "reason": "unknown",
                "observed_at": chrono::Utc::now().to_rfc3339(),
                "source": "manual"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let snapshot = harness.run_status_json().unwrap();
    assert!(
        snapshot["available_tickets"]
            .as_array()
            .is_some_and(|tickets| !tickets.is_empty()),
        "retry fixture was not observable: {snapshot}"
    );

    let result = harness.run_one_loop().unwrap();
    let prompt = harness.worker_argv_for_call("codex", 1).join("\n");
    assert!(
        prompt.contains("## Prior attempts"),
        "retry prompt omitted prior-attempt section; snapshot={} action={} details={} stderr={} calls={:?} events={:?} ledger={:?}",
        snapshot,
        result.action_kind,
        result.action_details,
        result.stderr_tail,
        result.call_counts,
        result.events,
        result.ledger_entries,
    );
    assert!(prompt.contains("retained integration validation tail"));

    let redispatch = result
        .ledger_entries
        .iter()
        .rev()
        .find(|entry| entry["mode"] != "claim" && entry["session_id"] != "prior-dispatch")
        .expect("retry completion ledger entry");
    assert_eq!(redispatch["dispatch_reason"], "retry");
}
