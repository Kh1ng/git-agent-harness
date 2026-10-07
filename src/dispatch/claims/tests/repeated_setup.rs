use super::*;

fn failure(summary: &str) -> LedgerEntry {
    let mut e = LedgerEntry::new(
        "test",
        &profile(Path::new("/tmp")),
        "auto",
        "fix",
        "#1462",
        None,
        None,
    );
    e.work_id = Some("#1462".into());
    e.failure_class = Some("harness_error".into());
    e.failure_stage = Some("dispatch".into());
    e.error_summary = Some(summary.into());
    e
}

fn lookup(entries: &[LedgerEntry]) -> TicketHistoryLookup {
    ledger_lookup_for_ticket(
        Some("#1462"),
        &profile(Path::new("/tmp")),
        &[],
        &crate::ledger::index_entries_by_work_id(entries),
    )
    .unwrap()
}

#[test]
fn signatures_normalize_numbers_paths_and_whitespace() {
    let disk = failure("insufficient free space on temporary filesystem (/tmp): 9 GiB available; require at least 20 GiB");
    let other = failure("insufficient free space on temporary filesystem (/tmp): 8 GiB available; require at least 21 GiB");
    assert_eq!(
        setup_failure_signature(&disk),
        setup_failure_signature(&other)
    );
    let path = failure("refusing to replace existing checkpoint worktree path /tmp/worktree-123");
    let other_path =
        failure("refusing  to replace existing checkpoint worktree path /other/worktree-456");
    assert_eq!(
        setup_failure_signature(&path),
        setup_failure_signature(&other_path)
    );
    assert_ne!(
        setup_failure_signature(&disk),
        setup_failure_signature(&path)
    );
    assert_eq!(
        setup_failure_signature(&failure("(/tmp/a): 9 GiB")),
        setup_failure_signature(&failure("(/tmp/b): 8 GiB"))
    );
    for (summary, expected) in [
        ("(/tmp/a): 9 GiB", "(<path>): # GiB"),
        ("'/home/x/y.rs',", "'<path>',"),
        ("/tmp", "<path>"),
        ("backend/model", "backend/model"),
        ("and/or 123", "and/or #"),
        ("a1/b2 ./tmp _/tmp -/tmp", "a#/b# ./tmp _/tmp -/tmp"),
    ] {
        assert_eq!(setup_failure_signature(&failure(summary)).2, expected);
    }
}

#[test]
fn only_three_identical_setup_attempts_gate() {
    let e = failure("setup failed 9 /tmp/run-1");
    assert!(!lookup(&[e.clone(), e.clone()]).4);
    let entries = vec![e.clone(); REPEATED_SETUP_FAILURE_LIMIT];
    let result = lookup(&entries);
    assert!(result.4);
    assert_eq!(result.6.as_deref(), Some("repeated_setup_failure"));
    assert_eq!(
        AvailableTicket::setup_failure_reason_for_ticket(
            "#1462",
            &profile(Path::new("/tmp")),
            &crate::ledger::index_entries_by_work_id(&entries)
        )
        .as_deref(),
        Some("the same setup failure happened 3 times in a row: setup failed 9 /tmp/run-1")
    );
    for class in [
        None,
        Some("backend_error"),
        Some("unknown"),
        Some("agent_failure"),
        Some("environment_error"),
    ] {
        let mut middle = e.clone();
        middle.failure_class = class.map(str::to_string);
        assert!(!lookup(&[e.clone(), middle, e.clone()]).4);
    }
    for field in ["summary", "stage"] {
        let mut middle = e.clone();
        if field == "summary" {
            middle.error_summary = Some("different failure".into());
        } else {
            middle.failure_stage = Some("preflight".into());
        }
        assert!(!lookup(&[e.clone(), middle, e.clone()]).4);
    }
    let mut environment = e.clone();
    environment.failure_class = Some("environment_error".into());
    assert!(lookup(&vec![environment; REPEATED_SETUP_FAILURE_LIMIT]).4);
}

#[test]
fn controls_are_ignored_reset_clears_and_existing_gate_wins() {
    let e = failure("setup failed");
    for mode in [
        "claim",
        "paid_route_approval_grant",
        "paid_route_approval_revoke",
        "review_hold",
        "review_hold_release",
        "fix",
    ] {
        let mut control = e.clone();
        control.mode = mode.into();
        control.failure_class = None;
        if mode == "fix" {
            control.validation_result = Some("deferred_capacity".into());
        }
        let result = lookup(&[e.clone(), control, e.clone(), e.clone()]);
        assert_eq!(result.0, REPEATED_SETUP_FAILURE_LIMIT);
        assert!(result.4, "{mode}");
    }
    let mut reset = e.clone();
    reset.mode = "clear_attempts".into();
    assert!(!lookup(&[e.clone(), e.clone(), e.clone(), reset, e.clone()]).4);
    let mut gate = e.clone();
    gate.human_required = true;
    gate.human_required_reason_code = Some("terminal_harness_failure".into());
    let entries = [gate, e.clone(), e.clone(), e];
    assert_eq!(
        lookup(&entries).6.as_deref(),
        Some("terminal_harness_failure")
    );
    assert!(AvailableTicket::setup_failure_reason_for_ticket(
        "#1462",
        &profile(Path::new("/tmp")),
        &crate::ledger::index_entries_by_work_id(&entries)
    )
    .is_none());
}

#[test]
fn interrupted_execution_is_ignored_without_resetting_setup_streak() {
    let setup = failure("setup failed");
    let mut shutdown = setup.clone();
    shutdown.validation_result = Some("cancelled_shutdown".into());
    let mut abandoned = setup.clone();
    abandoned.mode = "abandoned".into();
    abandoned.validation_result = Some("not_run_abandoned".into());
    abandoned.failure_stage = Some(crate::ledger::FailureStage::AgentRun.as_str().into());
    abandoned.error_summary =
        Some("dispatch abandoned before terminal telemetry was persisted".into());
    for ignored in [shutdown, abandoned] {
        assert!(!lookup(&vec![ignored.clone(); 3]).4);
        assert!(lookup(&[setup.clone(), setup.clone(), ignored, setup.clone()]).4);
    }
}

#[test]
fn identical_non_setup_failure_classes_do_not_gate() {
    for class in ["backend_error", "unknown"] {
        let mut entry = failure("same failure");
        entry.failure_class = Some(class.into());
        assert!(!lookup(&vec![entry; 3]).4, "{class}");
    }
}

/// Summaries copied from real ledger rows: each is recorded as
/// `harness_error` at the dispatch stage, and each clears without a human.
#[test]
fn transient_dispatch_refusals_never_gate() {
    let setup = failure("insufficient free space on temporary filesystem (/tmp): 9 GiB available");
    for summary in [
        "node admission deferred: node CPU reserve would be crossed (load 11.20 + 2.00 > 10.80)",
        "node admission deferred: node memory PSI is saturated (12.50% full avg10)",
        "fallback capacity deferred after 1 backend attempt(s): node admission deferred",
        "no eligible backend available for preferred codex/gpt-5; skipped 3 candidates",
        "shutdown requested before node admission",
    ] {
        let refusal = failure(summary);
        assert!(!lookup(&vec![refusal.clone(); 3]).4, "{summary}");
        assert!(
            lookup(&[setup.clone(), setup.clone(), refusal, setup.clone()]).4,
            "a refusal must not reset the streak: {summary}"
        );
    }
}

/// Setup worked on the third attempt (the agent launched and then failed),
/// so the setup failures are not three in a row.
#[test]
fn agent_failure_between_setup_failures_resets_the_streak() {
    let setup = failure("insufficient free space on temporary filesystem (/tmp): 9 GiB available");
    for class in ["backend_error", "harness_error"] {
        let mut crashed = failure("backend exited with status 1");
        crashed.failure_class = Some(class.into());
        crashed.failure_stage = Some(crate::ledger::FailureStage::AgentRun.as_str().into());
        assert!(
            !lookup(&[setup.clone(), setup.clone(), crashed, setup.clone()]).4,
            "{class}"
        );
    }
}
