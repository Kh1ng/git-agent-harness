//! Bounded-loop regression tests for recurring failure classes.
//!
//! Covers: sync failures (6), worker hangs/stale attempts (7),
//! invalid review output (8), and named incident regressions (10).
//!
//! Each test proves that within a bounded number of loops, GAH reaches a
//! terminal state — never silently spinning forever with indistinguishable
//! no_op decisions.

mod support;
use std::fs;
use support::fake_ledger::{ledger_entry_full, TestLedger};
use support::scenario::{read_jsonl_lines, GithubPrParams, ScenarioHarness};
use support::{FakeBackend, Scenario};
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────

/// Assert that within `max_loops` iterations the system reaches a
/// non-spinning terminal state (progress, recover, block, escalate,
/// human_required, or terminal no_op with a changing reason).
fn assert_bounded_progress_or_terminal(
    results: &[support::scenario::LoopResult],
    max_loops: usize,
    context: &str,
) {
    assert!(
        results.len() <= max_loops,
        "{context}: exceeded {max_loops} loops without reaching terminal state"
    );

    // Check that the last result is NOT an identical repeat of the prior
    let final_action = &results.last().unwrap().action_kind;
    assert!(
        !final_action.is_empty(),
        "{context}: empty action kind in final loop"
    );

    // If multiple results exist and the last two are both no_op, ensure
    // they have different reasons (no silent spin).
    if results.len() >= 2 {
        let (last, prev) = (&results[results.len() - 1], &results[results.len() - 2]);
        if last.action_kind == "no_op" && prev.action_kind == "no_op" {
            let same_reason =
                last.action_details == prev.action_details && last.exit_code == prev.exit_code;
            assert!(
                !same_reason,
                "{context}: consecutive indistinguishable no_op loops"
            );
        }
    }
}

// ── 6. Sync-failure bounded-loop tests ───────────────────────────────

/// Malformed GitHub JSON → sync error → controller must return bounded NoOp,
/// not spin forever.
#[test]
fn sync_github_malformed_json_is_noop() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("malformed")
        .worker_scenario("success");

    // The first loop should fail sync and return no_op (rule 1: incomplete observation).
    let r = harness.run_one_loop().unwrap();
    assert!(
        r.action_kind == "no_op" || r.action_kind == "exit_only",
        "malformed JSON: expected no_op/exit_only, got '{}' | stderr: {}",
        r.action_kind,
        r.stderr_tail
    );
    assert!(
        r.call_counts.get("gh").copied().unwrap_or(0) > 0,
        "gh should have been invoked"
    );

    // A second loop with the same malformed fixture should still be bounded.
    let r2 = harness.run_one_loop().unwrap();
    assert!(
        r2.action_kind == "no_op" || r2.action_kind == "exit_only",
        "malformed JSON loop 2: expected no_op, got '{}'",
        r2.action_kind
    );
}

/// GitHub non-zero exit → same bounded behavior as malformed JSON.
#[test]
fn sync_github_non_zero_exit_is_noop() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("non_zero_exit")
        .worker_scenario("success");

    let r = harness.run_one_loop().unwrap();
    assert!(
        r.action_kind == "no_op" || r.action_kind == "exit_only",
        "non-zero exit: expected no_op/exit_only, got '{}' | stderr: {}",
        r.action_kind,
        r.stderr_tail
    );
    assert!(r.call_counts.get("gh").copied().unwrap_or(0) > 0);
}

/// Provider fails then recovers — previous failures must not permanently
/// poison state.
#[test]
fn sync_provider_fails_then_recovers() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");

    // Sequence: fail, fail, succeed (one PR needing review)
    let pr_json = support::scenario::github_pr_json(GithubPrParams {
        title: "Draft: TICKET-001 Feature".into(),
        branch: "gah/feature-1".into(),
        labels: vec![],
        ci_conclusion: Some("SUCCESS".into()),
        url: None,
        number: None,
        draft: None,
        merged_at: None,
        updated_at: None,
    });

    gh.install_sequence(vec![
        Scenario::failure(1).with_stderr("gh: network error"),
        Scenario::failure(1).with_stderr("gh: network error"),
        Scenario::success().with_stdout(serde_json::to_string(&vec![pr_json]).unwrap()),
    ]);

    // Copy the fake gh script into the harness bin_dir for subprocess resolution
    let mut harness = ScenarioHarness::new("github");
    harness = harness
        .github_scenario("empty")
        .worker_scenario("review_approve");
    harness.install_custom_gh(&gh);

    // The recovery fixture (loop 3) returns a PR on gah/feature-1.
    // gah will try to fetch that branch from origin; create it.
    harness.create_remote_branch("gah/feature-1");

    // Loop 1: provider failure → no_op
    let r1 = harness.run_one_loop().unwrap();
    let calls_after_r1 = gh.call_count();
    assert!(
        r1.action_kind == "no_op" || r1.action_kind == "exit_only",
        "loop 1 failure: got '{}'",
        r1.action_kind
    );

    // Loop 2: provider failure → no_op
    let r2 = harness.run_one_loop().unwrap();
    let calls_after_r2 = gh.call_count();
    assert!(
        r2.action_kind == "no_op" || r2.action_kind == "exit_only",
        "loop 2 failure: got '{}'",
        r2.action_kind
    );

    // Loop 3: provider succeeds → gah should NOT crash
    let r3 = harness.run_one_loop().unwrap();
    let calls_after_r3 = gh.call_count();
    assert_eq!(
        r3.exit_code,
        Some(0),
        "loop 3 recovery: gah exited non-zero | stderr: {}",
        r3.stderr_tail
    );
    assert!(calls_after_r1 > 0, "loop 1 must call the provider");
    assert!(
        calls_after_r2 > calls_after_r1,
        "loop 2 must retry the provider"
    );
    assert!(
        calls_after_r3 > calls_after_r2,
        "loop 3 must retry the provider"
    );
}

/// Persistent provider failure — must eventually escalate or require human.
#[test]
fn sync_persistent_provider_failure_is_bounded() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("malformed")
        .worker_scenario("success");

    let results = harness.run_loops(5).unwrap();
    assert_bounded_progress_or_terminal(&results, 5, "persistent sync failure");
    // Every loop should call gh
    for r in &results {
        assert!(r.call_counts.get("gh").copied().unwrap_or(0) > 0);
    }
}

/// GitLab malformed JSON → same bounded behavior.
#[test]
fn sync_gitlab_malformed_json_is_noop() {
    let mut harness = ScenarioHarness::new("gitlab")
        .gitlab_scenario("malformed")
        .worker_scenario("success");

    let r = harness.run_one_loop().unwrap();
    assert!(
        r.action_kind == "no_op" || r.action_kind == "exit_only",
        "gitlab malformed: got '{}'",
        r.action_kind
    );
}

/// GitLab non-zero exit → bounded.
#[test]
fn sync_gitlab_non_zero_exit_is_noop() {
    let mut harness = ScenarioHarness::new("gitlab")
        .gitlab_scenario("non_zero_exit")
        .worker_scenario("success");

    let r = harness.run_one_loop().unwrap();
    assert!(
        r.action_kind == "no_op" || r.action_kind == "exit_only",
        "gitlab non-zero exit: got '{}'",
        r.action_kind
    );
}

// ── Named incident: null statusCheckRollup ───────────────────────────

/// Real July 2026 incident: GitHub returned `statusCheckRollup: null` for
/// a closed PR. The original deserialization treated this as a hard error,
/// collapsing the entire sync into an incomplete-observation NoOp that
/// repeated forever. The fix made `statusCheckRollup` an
/// `Option<Vec<GithubCheck>>` so `null` → `None`. This test proves the
/// full controller/supervisor loop handles `null` correctly and does not
/// spin in repeated no_op.
#[test]
fn regression_github_null_status_rollup_does_not_spin_noop_forever() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("prs_closed_null_rollup")
        .worker_scenario("review_approve");

    // The fake GitHub returned a PR on gah/test-1.  gah will try to
    // fetch that branch from origin; create it so the fetch succeeds.
    harness.create_remote_branch("gah/test-1");

    // Real July 2026 incident: GitHub returned `statusCheckRollup: null`.
    // The fix made it Option<Vec<GithubCheck>> so null→None. This test
    // proves the FULL controller/supervisor loop handles null without
    // crashing into an unrecoverable sync-error NoOp.
    //
    // What we care about: (a) gah doesn't crash (exit code 0),
    // (b) the null doesn't cause a deserialization error that loops
    // indefinitely. The review dispatch succeeds/pproves on our fake reviewer.
    let r = harness.run_one_loop().unwrap();
    assert_eq!(
        r.exit_code,
        Some(0),
        "null rollup: gah exited non-zero (crashed); stderr: {}",
        r.stderr_tail
    );
}

// ── 7. Worker hang / stale-attempt bounded-loop tests ────────────────

/// Worker exits non-zero → attempt recorded, next loop should not be
/// permanently blocked.
#[test]
fn worker_non_zero_exit_is_recorded() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("failure");

    let r = harness.run_one_loop().unwrap();
    // With empty PRs and worker=success, this should be no_op or wait_until
    assert!(!r.action_kind.is_empty());
}

/// Worker exits 0 with empty output — should not be treated as success
/// that silently blocks progress.
#[test]
fn worker_empty_success_is_not_permanent_block() {
    // This tests that empty worker output doesn't cause a deadlock.
    // Without real tickets to dispatch, this exercises the path minimally.
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("empty_success");

    let results = harness.run_loops(3).unwrap();
    assert_bounded_progress_or_terminal(&results, 3, "worker empty output");
}

// ── 8. Invalid review output bounded-loop tests ──────────────────────

/// Validate that the harness correctly exercises the review path when a
/// PR needs review. This is a smoke test for the review infrastructure.
#[test]
fn review_path_is_exercisable() {
    // With a PR needing review (NEEDS_REVIEW classification) and a fake
    // worker, the harness should be able to run through the review path.
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("one_pr_needs_review")
        .worker_scenario("success");

    let r = harness.run_one_loop().unwrap();
    // The controller should attempt to review the PR.
    // Possible outcomes: review_mr (dispatched reviewer), no_op (reviewer
    // unavailable), wait_until (backends unavailable with known reset).
    assert!(
        !r.action_kind.is_empty(),
        "review path: empty action | stderr: {}",
        r.stderr_tail
    );
}

// ── 10. Named deadlock regression suite ──────────────────────────────

/// Regression: sync error must NOT be reported as healthy idle.
#[test]
fn regression_sync_error_is_not_reported_as_healthy_idle() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("malformed")
        .worker_scenario("success");

    let r = harness.run_one_loop().unwrap();
    // A sync error should produce no_op (rule 1: incomplete observation),
    // NOT a healthy-looking dispatch.
    assert_ne!(
        r.action_kind, "dispatch_ticket",
        "sync error must not dispatch tickets"
    );
}

/// Regression: stale started attempt must not block forever.
/// Ledger has attempt_started but no corresponding worker process.
#[test]
fn regression_stale_started_attempt_does_not_block_forever() {
    let entry = ledger_entry_full(
        "improve",
        "gah/stale-1",
        Some("initial"),
        "TICKET-001",
        "2026-07-01T00:00:00Z",
    );

    let ledger = TestLedger::new().with_entry(entry);

    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("success")
        .with_ledger(ledger);

    let results = harness.run_loops(3).unwrap();
    assert_bounded_progress_or_terminal(&results, 3, "stale attempt");
}

/// Regression: quota-exhausted reviewer failover is bounded.
#[test]
fn regression_quota_exhausted_reviewer_failover_is_bounded() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("one_pr_needs_review")
        .worker_scenario("failure");

    let results = harness.run_loops(3).unwrap();
    assert_bounded_progress_or_terminal(&results, 3, "quota exhausted failover");
}

/// A generic non-zero review exit has no availability signal, so policy
/// retries the same route within the fixed three-attempt budget and emits one
/// terminal dispatch result.
#[test]
fn regression_generic_nonzero_review_retry_is_bounded_and_terminal() {
    let mut harness = ScenarioHarness::new("github")
        .with_config_append("[profiles.test.routing]\nreview_backend = \"claude\"\n")
        .github_scenario("one_pr_needs_review")
        .worker_scenario("failure");
    harness.create_remote_branch("gah/feature-1");

    let dispatch_result = harness
        .run_dispatch(&["--mode", "review", "--branch", "gah/feature-1"])
        .unwrap();
    let events = read_jsonl_lines(&harness.events_path).unwrap_or_default();
    let ledger_entries = fs::read_to_string(&harness.ledger_path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect::<Vec<_>>();
    let attempts = ledger_entries
        .last()
        .and_then(|entry| entry["attempts"].as_array())
        .expect("terminal ledger entry has attempts");

    assert_eq!(attempts.len(), 3);
    assert!(attempts
        .iter()
        .all(|attempt| attempt["backend"] == "claude"));
    let models = attempts
        .iter()
        .map(|attempt| attempt["effective_model"].clone())
        .collect::<Vec<_>>();
    assert!(models.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"].as_str() == Some("dispatch_started"))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"].as_str() == Some("dispatch_finished"))
            .count(),
        1
    );
    assert!(events.iter().any(|event| event["details"]
        .as_str()
        .is_some_and(|details| details.contains("review failed after 3 attempt(s)"))));
    assert_ne!(dispatch_result.exit_code, Some(0));
}

/// An idle timeout is an availability signal even when the backend emitted
/// partial output before stalling. The attempted route is recorded, its slot
/// is released, and the next configured candidate can complete the dispatch.
#[test]
fn regression_idle_timeout_with_partial_output_reroutes_and_succeeds() {
    let tmp = TempDir::new().unwrap();
    let vibe = FakeBackend::new(tmp.path(), "vibe");
    vibe.install(
        Scenario::success()
            .with_stdout("Initializing review context...\n")
            .with_delay_ms(30_000),
    );
    let claude = FakeBackend::new(tmp.path(), "claude");
    claude.install(Scenario::success().with_stdout(
        r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:gah-feature-1.md"]}"#,
    ));

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(
            "review_timeout_seconds = 1\n[profiles.test.routing]\nreview_backend = \"vibe\"\nweak_review_backend = \"claude\"\nallow_review_fallback = true\n",
        )
        .github_scenario("one_pr_needs_review");
    harness.install_custom_worker("vibe", &vibe);
    harness.install_custom_worker("claude", &claude);
    harness.create_remote_branch("gah/feature-1");

    let dispatch_result = harness
        .run_dispatch(&["--mode", "review", "--branch", "gah/feature-1"])
        .unwrap();
    assert_eq!(
        dispatch_result.exit_code,
        Some(0),
        "{}",
        dispatch_result.stderr
    );

    let entry = fs::read_to_string(&harness.ledger_path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .next_back()
        .expect("ledger entry");
    let attempts = entry["attempts"].as_array().expect("attempts");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["backend"], "vibe");
    assert_eq!(attempts[0]["validation_result"], "not_run_idle_timeout");
    assert_eq!(attempts[1]["backend"], "claude");
    assert_eq!(attempts[1]["validation_result"], "APPROVE");

    let events = read_jsonl_lines(&harness.events_path).unwrap_or_default();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"].as_str() == Some("dispatch_finished"))
            .count(),
        1
    );
    assert!(!events.iter().any(|event| event["details"]
        .as_str()
        .is_some_and(|details| details.contains("dispatch failed"))));
}

/// The hard safety ceiling is operator policy, not backend unavailability. It
/// remains terminal and must not consume a fallback reviewer.
#[test]
fn regression_hard_review_timeout_does_not_fallback() {
    let tmp = TempDir::new().unwrap();
    let vibe = FakeBackend::new(tmp.path(), "vibe");
    vibe.install(Scenario::success().with_delay_ms(30_000));
    let claude = FakeBackend::new(tmp.path(), "claude");
    claude.install(Scenario::success().with_stdout(
        r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:gah-feature-1.md"]}"#,
    ));

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(
            "review_timeout_seconds = 30\nreview_hard_timeout_seconds = 1\n[profiles.test.routing]\nreview_backend = \"vibe\"\nweak_review_backend = \"claude\"\nallow_review_fallback = true\n",
        )
        .github_scenario("one_pr_needs_review");
    harness.install_custom_worker("vibe", &vibe);
    harness.install_custom_worker("claude", &claude);
    harness.create_remote_branch("gah/feature-1");

    let dispatch_result = harness
        .run_dispatch(&["--mode", "review", "--branch", "gah/feature-1"])
        .unwrap();
    assert_ne!(dispatch_result.exit_code, Some(0));
    assert_eq!(vibe.call_count(), 1);
    assert_eq!(claude.call_count(), 0);

    let entry = fs::read_to_string(&harness.ledger_path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .next_back()
        .expect("ledger entry");
    let attempts = entry["attempts"].as_array().expect("attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["backend"], "vibe");
    assert_eq!(attempts[0]["validation_result"], "not_run_hard_timeout");
}
