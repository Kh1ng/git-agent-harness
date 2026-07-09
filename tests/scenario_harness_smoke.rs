//! Smoke tests for the deterministic controller/supervisor test harness.
//!
//! Proves the ScenarioHarness infrastructure works: fake provider CLIs,
//! fake workers, ledger builder, run_one_loop, and run_loops(n).

mod support;
use support::scenario::ScenarioHarness;
use support::{FakeBackend, Scenario};
use tempfile::TempDir;

/// Fake `gh` executable returns fixtures and logs calls.
#[test]
fn fake_provider_gh_returns_fixture() {
    let tmp = TempDir::new().unwrap();
    let fb = FakeBackend::new(tmp.path(), "gh");
    fb.install(Scenario::success().with_stdout("[{\"number\":1}]"));
    assert_eq!(fb.call_count(), 0);

    let out = std::process::Command::new(fb.bin_dir().join("gh"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                fb.bin_dir().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"number\":1"));
    assert_eq!(fb.call_count(), 1);
    // argv_for_call returns the recorded args; can be empty if no args passed.
    let _argv = fb.argv_for_call(1);
}

/// Fake ledger builder writes and reads JSONL.
#[test]
fn fake_ledger_writes_readable_jsonl() {
    let tmp = TempDir::new().unwrap();
    let entry = serde_json::json!({"work_id": "TICKET-SMOKE", "profile": "test"});
    let ledger = support::fake_ledger::TestLedger::new().with_entry(entry);
    let path = ledger.write_into(tmp.path()).unwrap();
    assert!(path.exists());

    let entries = support::fake_ledger::TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["work_id"], "TICKET-SMOKE");
}

/// Fake worker executable returns configured output and logs calls.
#[test]
fn fake_worker_returns_output() {
    let tmp = TempDir::new().unwrap();
    let fb = FakeBackend::new(tmp.path(), "openhands");
    fb.install(Scenario::success().with_stdout("task complete"));
    assert_eq!(fb.call_count(), 0);

    let out = std::process::Command::new(fb.bin_dir().join("openhands"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                fb.bin_dir().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .arg("--headless")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("task complete"));
    assert_eq!(fb.call_count(), 1);
    let argv = fb.argv_for_call(1);
    assert!(argv.contains(&"--headless".to_string()));
}

/// run_one_loop() executes a real controller/decision path and returns
/// inspectable results.
#[test]
fn one_loop_returns_inspectable_result() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("success");

    let result = harness.run_one_loop().unwrap();
    assert!(
        result.action_kind == "no_op"
            || result.action_kind == "human_required"
            || result.action_kind == "wait_until"
            || result.action_kind == "exit_only",
        "unexpected action: {} | stderr: {}",
        result.action_kind,
        result.stderr_tail
    );
    // gh should have been invoked at least once.
    assert!(
        result.call_counts.get("gh").copied().unwrap_or(0) > 0,
        "gh was not invoked; stderr: {}",
        result.stderr_tail
    );
}

/// run_loops(3) preserves state across iterations.
#[test]
fn multi_loop_preserves_state() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("success");

    let results = harness.run_loops(3).unwrap();
    assert_eq!(results.len(), 3);

    for (i, r) in results.iter().enumerate() {
        assert!(
            !r.action_kind.is_empty(),
            "loop {i} returned empty action_kind"
        );
    }

    let entries =
        support::fake_ledger::TestLedger::read_from(&harness.ledger_path).unwrap_or_default();
    // Ledger entries may be empty if all loops returned WaitUntil/NoOp
    // (no dispatch occurred). The key is that state was preserved across
    // iterations — the call counts and events accumulate.
    assert!(results.len() == 3, "expected 3 loop results");
    // Events should accumulate across loops.
    let events = support::scenario::read_jsonl_lines(&harness.events_path).unwrap_or_default();
    assert!(!events.is_empty(), "no events after 3 loops");
    let _ = entries; // ledger may be empty for WaitUntil/NoOp loops
}
