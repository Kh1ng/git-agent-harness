//! Hermetic integration tests for Issue #230: automatic post-attempt
//! telemetry export and its operator-visible export health.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

use support::fake_ledger::{ledger_entry_full, TestLedger};
use support::scenario::ScenarioHarness;

fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}

fn install_successful_openhands(harness: &ScenarioHarness) {
    write_exec(
        &harness.bin_dir.join("openhands"),
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nprintf 'input_tokens: 10\\noutput_tokens: 5\\n'\n",
    );
}

/// Automatic post-attempt export writes into `<artifact_root>/telemetry` by
/// default (Issue #230, `GahConfig::defaults::telemetry_export_path`) --
/// `ScenarioHarness`'s generated config points `artifact_root` at the
/// harness's own temp artifacts dir.
fn telemetry_root(harness: &ScenarioHarness) -> PathBuf {
    harness.artifacts_dir.join("telemetry")
}

/// Collect every exported task-outcome record's `work_id`, walking all
/// partition JSONL files under `raw/outcomes`.
fn exported_task_outcome_work_ids(telemetry_root: &Path) -> Vec<String> {
    let mut ids = Vec::new();
    let outcomes_dir = telemetry_root.join("raw").join("outcomes");
    if !outcomes_dir.exists() {
        return ids;
    }
    let mut stack = vec![outcomes_dir];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let content = fs::read_to_string(&path).unwrap();
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                if value.get("record_type").and_then(|v| v.as_str()) == Some("task_outcome") {
                    if let Some(work_id) = value["data"]["work_id"].as_str() {
                        ids.push(work_id.to_string());
                    }
                }
            }
        }
    }
    ids
}

fn failed_entry(ts: &str, work_id: &str) -> serde_json::Value {
    let mut entry = ledger_entry_full("fix", &format!("gah/{work_id}"), None, work_id, ts);
    let obj = entry.as_object_mut().unwrap();
    obj.insert(
        "failure_class".into(),
        serde_json::Value::String("ImplementationFailure".into()),
    );
    obj.insert(
        "failure_stage".into(),
        serde_json::Value::String("implementation".into()),
    );
    obj.insert("backend_exit_code".into(), serde_json::json!(1));
    entry
}

fn timed_out_entry(ts: &str, work_id: &str) -> serde_json::Value {
    let mut entry = ledger_entry_full("fix", &format!("gah/{work_id}"), None, work_id, ts);
    let obj = entry.as_object_mut().unwrap();
    obj.insert(
        "validation_result".into(),
        serde_json::Value::String("not_run_idle_timeout".into()),
    );
    obj.insert(
        "failure_class".into(),
        serde_json::Value::String("BackendTimeout".into()),
    );
    entry
}

fn cancelled_entry(ts: &str, work_id: &str) -> serde_json::Value {
    let mut entry = ledger_entry_full("fix", &format!("gah/{work_id}"), None, work_id, ts);
    let obj = entry.as_object_mut().unwrap();
    obj.insert(
        "validation_result".into(),
        serde_json::Value::String("cancelled_shutdown".into()),
    );
    obj.insert(
        "failure_class".into(),
        serde_json::Value::String("Cancelled".into()),
    );
    entry
}

/// A hermetic mix of pre-existing failed/timed-out/cancelled terminal
/// attempts plus one live successful dispatch: the live dispatch's
/// post-attempt export hook re-scans the *authoritative* ledger, so all
/// four terminal outcome kinds must appear in the export -- each exactly
/// once -- after a single terminal dispatch.
#[test]
fn every_terminal_attempt_kind_is_exported_exactly_once() {
    let ledger = TestLedger::new()
        .with_entry(failed_entry("2026-01-01T00:00:01Z", "TICKET-FAILED"))
        .with_entry(timed_out_entry("2026-01-01T00:00:02Z", "TICKET-TIMEDOUT"))
        .with_entry(cancelled_entry("2026-01-01T00:00:03Z", "TICKET-CANCELLED"));

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(
            "[profiles.test.publishing]\nallow_pull_request_creation = false\nallow_commit_message_generation = false\n",
        )
        .with_ledger(ledger);
    install_successful_openhands(&harness);

    let result = harness
        .run_dispatch(&[
            "--mode",
            "fix",
            "--backend",
            "openhands",
            "--target",
            "add a successful change",
        ])
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "stderr was {}", result.stderr);

    let ledger_entries = TestLedger::read_from(&harness.ledger_path).unwrap();
    assert!(
        ledger_entries.len() >= 4,
        "3 seeded terminal attempts plus the live dispatch's own, got {}",
        ledger_entries.len()
    );

    let exported_ids = exported_task_outcome_work_ids(&telemetry_root(&harness));
    for expected in ["TICKET-FAILED", "TICKET-TIMEDOUT", "TICKET-CANCELLED"] {
        let count = exported_ids
            .iter()
            .filter(|id| id.as_str() == expected)
            .count();
        assert_eq!(
            count, 1,
            "{expected} must be exported exactly once, got {count} (all exported ids: {exported_ids:?})"
        );
    }
    assert!(
        exported_ids.len() >= 4,
        "the live dispatch's own successful attempt is exported too: {exported_ids:?}"
    );

    let status = harness.run_status_json().unwrap();
    let export_health = &status["export_health"];
    assert_eq!(export_health["status"], "healthy", "{export_health}");
    assert_eq!(export_health["retry_pending"], false);
    assert!(export_health["record_count"].as_u64().unwrap() > 0);
}

/// A forced exporter failure (the raw output directory is blocked by a
/// regular file where a directory belongs) must never touch the ledger,
/// must surface as a non-healthy export status with a retained/classified
/// error, and must resolve to healthy once the blocker is cleared and the
/// next scheduled export retries.
#[test]
fn forced_export_failure_preserves_ledger_and_recovers_on_retry() {
    let mut harness = ScenarioHarness::new("github").with_config_append(
        "[profiles.test.publishing]\nallow_pull_request_creation = false\nallow_commit_message_generation = false\n",
    );
    install_successful_openhands(&harness);

    let repo = telemetry_root(&harness);
    fs::create_dir_all(&repo).unwrap();
    fs::write(
        repo.join("raw"),
        b"blocking regular file where a directory belongs",
    )
    .unwrap();

    let first = harness
        .run_dispatch(&[
            "--mode",
            "fix",
            "--backend",
            "openhands",
            "--target",
            "first change",
        ])
        .unwrap();
    assert_eq!(
        first.exit_code,
        Some(0),
        "dispatch outcome must succeed even though export fails: {}",
        first.stderr
    );

    let ledger_after_failure = TestLedger::read_from(&harness.ledger_path).unwrap();
    assert_eq!(
        ledger_after_failure.len(),
        1,
        "the terminal attempt's ledger entry must be intact despite the export failure"
    );

    // `ScenarioHarness::run_dispatch`/`run_status_json` rewrite ledger.jsonl
    // from the harness's own fixed `TestLedger` snapshot on every call (a
    // harness convenience for hermetic seeding), so the real subprocess's
    // freshly appended entry must be fed back into that snapshot before any
    // further harness call, or it would be silently dropped on the next
    // rewrite -- this is a test-harness quirk, not ledger corruption.
    harness = harness.with_ledger(
        TestLedger::new().with_entry(ledger_after_failure.into_iter().next().unwrap()),
    );

    let status = harness.run_status_json().unwrap();
    let health = &status["export_health"];
    assert_ne!(
        health["status"], "healthy",
        "export health must reflect the forced failure: {health}"
    );
    assert_eq!(health["retry_pending"], true, "{health}");
    assert!(health["last_error"].is_string(), "{health}");
    assert!(health["last_error_class"].is_string(), "{health}");

    // Clear the blocker; the next scheduled export (the next terminal
    // dispatch) must retry and succeed.
    fs::remove_file(repo.join("raw")).unwrap();

    let second = harness
        .run_dispatch(&[
            "--mode",
            "fix",
            "--backend",
            "openhands",
            "--target",
            "second change",
        ])
        .unwrap();
    assert_eq!(second.exit_code, Some(0), "stderr was {}", second.stderr);

    let ledger_after_retry = TestLedger::read_from(&harness.ledger_path).unwrap();
    assert_eq!(
        ledger_after_retry.len(),
        2,
        "both terminal attempts remain durably in the ledger"
    );

    let status2 = harness.run_status_json().unwrap();
    let health2 = &status2["export_health"];
    assert_eq!(
        health2["status"], "healthy",
        "export recovers once the blocker is gone: {health2}"
    );
    assert_eq!(health2["retry_pending"], false, "{health2}");
    assert!(health2["record_count"].as_u64().unwrap() > 0);
}

/// Two terminal attempts scheduling an export at the same instant must
/// serialize on the export health lock rather than corrupt or duplicate
/// the shared output -- both records land in one coherent export.
#[test]
fn concurrent_terminal_attempts_produce_one_coherent_export() {
    let ledger = TestLedger::new()
        .with_entry(failed_entry("2026-01-01T00:00:01Z", "TICKET-CONCURRENT-A"))
        .with_entry(cancelled_entry(
            "2026-01-01T00:00:02Z",
            "TICKET-CONCURRENT-B",
        ));

    let mut harness = ScenarioHarness::new("github").with_ledger(ledger);
    // Cheaply write the config file and set up the harness's process-wide
    // env vars (GAH_CONFIG/GAH_LEDGER_PATH/...) without a real dispatch, so
    // the in-process library calls below resolve the same paths.
    harness.run_status_json().unwrap();

    let cfg = Arc::new(
        git_agent_harness::config::load(Some(harness.config_path.to_str().unwrap())).unwrap(),
    );

    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let cfg = Arc::clone(&cfg);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                git_agent_harness::telemetry::schedule_export_after_terminal_attempt(&cfg);
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let exported_ids = exported_task_outcome_work_ids(&telemetry_root(&harness));
    for expected in ["TICKET-CONCURRENT-A", "TICKET-CONCURRENT-B"] {
        let count = exported_ids
            .iter()
            .filter(|id| id.as_str() == expected)
            .count();
        assert_eq!(
            count, 1,
            "{expected} exported exactly once despite concurrent schedulers: {exported_ids:?}"
        );
    }

    let health = git_agent_harness::telemetry::health::read_view(&telemetry_root(&harness));
    assert_eq!(health.status, "healthy");
    assert!(!health.retry_pending);
}
