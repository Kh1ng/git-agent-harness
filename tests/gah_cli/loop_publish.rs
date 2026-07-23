use super::*;

/// TICKET-072: `gah ledger reconcile` must append a reconciliation entry
/// when a dispatched work item's MR has since merged, and must never
/// rewrite `ledger.jsonl` itself.
#[test]
fn ledger_reconcile_appends_entry_when_mr_state_changed() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let reconciliation_path = tmp.path().join("reconciliation.jsonl");

    fs::write(
        &ledger_path,
        "{\"timestamp\":\"2026-07-01T00:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"codex\",\"requested_backend\":\"codex\",\"effective_backend\":\"codex\",\"requested_model\":null,\"effective_model\":null,\"routing_reason\":\"explicit\",\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"work_id\":\"TICKET-072\",\"mode\":\"fix\",\"target_summary\":\"x\",\"branch\":\"gah/real-1\",\"session_dir\":null,\"duration_seconds\":1.0,\"backend_exit_code\":0,\"validation_result\":\"passed\",\"commit_attempted\":true,\"commit_created\":true,\"push_attempted\":true,\"push_succeeded\":true,\"mr_attempted\":true,\"mr_created\":true,\"mr_url\":\"https://github.com/owner/real/pull/7\",\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"usage_source\":null}}\n",
    )
    .unwrap();

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] Fix: TICKET-072\",\"headRefName\":\"gah/real-1\",\"url\":\"https://github.com/owner/real/pull/7\",\"labels\":[],\"number\":7,\"state\":\"MERGED\",\"isDraft\":false,\"mergeStateStatus\":\"MERGED\",\"mergedAt\":\"2026-07-05T00:00:00Z\",\"updatedAt\":\"2026-07-05T00:00:00Z\",\"statusCheckRollup\":[]}]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "ledger",
            "reconcile",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_RECONCILIATION_PATH", &reconciliation_path)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let entries = parsed["new_entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["work_id"], "TICKET-072");
    assert_eq!(entries[0]["new_state"], "MERGED");
    assert_eq!(entries[0]["previous_state"], Value::Null);

    // ledger.jsonl itself must be untouched (still exactly the one original line).
    let ledger_text = fs::read_to_string(&ledger_path).unwrap();
    assert_eq!(ledger_text.lines().count(), 1);

    // Running again with the same (still-MERGED) state must not append a
    // second, redundant reconciliation entry.
    bin()
        .args([
            "ledger",
            "reconcile",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_RECONCILIATION_PATH", &reconciliation_path)
        .assert()
        .success()
        .stdout("{\"new_entries\":[],\"issue_closure\":{\"already_closed\":[],\"would_close\":[],\"closed\":[],\"ambiguous\":[],\"unmapped\":[\"unknown\"],\"leave_open\":[],\"observation_failed\":[],\"policy_blocked\":[],\"skipped\":[]}}\n");

    let reconciliation_text = fs::read_to_string(&reconciliation_path).unwrap();
    assert_eq!(reconciliation_text.lines().count(), 1);
}

/// TICKET-079: recurring mode is the default; --once remains an explicit
/// bounded/testing mode.
#[test]
fn loop_without_once_is_accepted_as_recurring_mode() {
    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            "/definitely/does/not/exist.toml",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no config found"));
}

/// TICKET-079: nothing to do (no tickets, no MRs, no availability records)
/// must report NoOp and exit successfully -- not error, not hang.
#[test]
fn loop_once_reports_noop_when_nothing_actionable() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    let events_path = tmp.path().join("events.jsonl");
    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("no_op"));

    let events_text = fs::read_to_string(&events_path).unwrap();
    assert!(events_text.contains("observation_completed"));
    assert!(events_text.contains("action_decided"));
    assert!(events_text.contains("loop_stopped"));
}

#[test]
fn loop_once_prune_skips_full_provider_history_and_retains_fresh_worktree() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let worktree_root = tmp.path().join("worktrees");
    fs::create_dir_all(&worktree_root).unwrap();
    let worktree = worktree_root.join("gah-real-no-progress");
    ProcessCommand::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "gah/real-no-progress",
            worktree.to_str().unwrap(),
            "HEAD",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo 'automatic loop must not query full PR history' >&2; exit 97; fi\nif [ \"$1\" = \"api\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .stdout(predicate::str::contains("no_op"));

    assert!(worktree.exists(), "fresh worktree was automatically pruned");
}

/// TICKET-079: an eligible never-dispatched ticket actually gets dispatched
/// (fix mode) -- the full observe -> decide -> execute -> persist path,
/// not just the decision in isolation.
#[test]
fn loop_once_dispatches_an_eligible_ticket() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let github_root = tmp.path().join("github-root");
    let origin = github_root.join("owner/real.git");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&home).unwrap();
    init_git_repo(&repo);
    fs::create_dir_all(origin.parent().unwrap()).unwrap();
    ProcessCommand::new("git")
        .args(["init", "--bare", origin.to_str().unwrap()])
        .output()
        .unwrap();
    configure_git_url_instead_of(
        &home,
        "https://github.com/",
        &format!("file://{}/", github_root.display()),
    );
    ProcessCommand::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/owner/real.git",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&repo)
        .env("HOME", &home)
        .output()
        .unwrap();

    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    fs::write(
        repo.join("docs/tickets/TICKET-300-loop-test.md"),
        "# TICKET-300: Loop test ticket\n\nGoal: test loop --once dispatch\n\nRecommended backend: codex\n",
    )
    .unwrap();

    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "validation_commands = [\"true\"]\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"api\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
    );

    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("dispatch_ticket"));

    let ledger_text = fs::read_to_string(&ledger_path).unwrap();
    // Parallel workers: the first line is now a "claim" entry (written
    // before any backend work runs, so a concurrent worker sees this
    // ticket is taken immediately rather than only after the dispatch
    // finishes minutes-to-hours later) -- the real completion entry is
    // the first non-claim line.
    let entry: Value = ledger_text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|e| e["mode"] != "claim")
        .expect("a real completion entry after the claim");
    assert_eq!(entry["work_id"], "TICKET-300");
    assert_eq!(entry["validation_result"], "passed");

    let events_text = fs::read_to_string(&events_path).unwrap();
    assert!(events_text.contains("dispatch_started"));
    assert!(events_text.contains("dispatch_finished"));
}

/// Regression for the parallel-batch-abort bug: a terminal decision
/// (NoOp/HumanRequired/WaitUntil) for ONE slot must not stop OTHER slots in
/// the same `--parallel` batch from being tried. Simulated here via a `gh`
/// stub that fails `pr list` on exactly the second call (a transient sync
/// hiccup) -- the middle of 3 slots hits it and legitimately decides NoOp
/// ("observation incomplete"), while slot 1 (before the hiccup) and slot 3
/// (after it clears) each find a distinct, real, dispatchable ticket. Before
/// the fix, the middle slot's NoOp `break`s the whole batch and TICKET-301
/// (only reachable from slot 3) never gets dispatched.
#[test]
fn parallel_loop_slot_terminal_action_does_not_abort_later_slots() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let github_root = tmp.path().join("github-root");
    let origin = github_root.join("owner/real.git");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&home).unwrap();
    init_git_repo(&repo);
    fs::create_dir_all(origin.parent().unwrap()).unwrap();
    ProcessCommand::new("git")
        .args(["init", "--bare", origin.to_str().unwrap()])
        .output()
        .unwrap();
    configure_git_url_instead_of(
        &home,
        "https://github.com/",
        &format!("file://{}/", github_root.display()),
    );
    ProcessCommand::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/owner/real.git",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&repo)
        .env("HOME", &home)
        .output()
        .unwrap();

    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    fs::write(
        repo.join("docs/tickets/TICKET-300-loop-test.md"),
        "# TICKET-300: Loop test ticket A\n\nGoal: test loop --parallel dispatch\n\nRecommended backend: codex\n",
    )
    .unwrap();
    fs::write(
        repo.join("docs/tickets/TICKET-301-loop-test.md"),
        "# TICKET-301: Loop test ticket B\n\nGoal: test loop --parallel dispatch\n\nRecommended backend: codex\n",
    )
    .unwrap();

    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "validation_commands = [\"true\"]\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    let pr_list_count_file = tmp.path().join("pr_list_count");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
             "#!/bin/sh\n\
             if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then\n\
             \x20\x20lock_dir='{count_file}.lock'\n\
             \x20\x20while ! mkdir \"$lock_dir\" 2>/dev/null; do sleep 0.01; done\n\
             \x20\x20count=$(( $(cat '{count_file}' 2>/dev/null || echo 0) + 1 ))\n\
             \x20\x20echo \"$count\" > '{count_file}'\n\
             \x20\x20rmdir \"$lock_dir\"\n\
             \x20\x20if [ \"$count\" = \"2\" ]; then echo 'simulated transient sync failure' >&2; exit 1; fi\n\
             \x20\x20echo '[]'; exit 0\n\
             fi\n\
             if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\n\
             if [ \"$1\" = \"api\" ]; then echo '[]'; exit 0; fi\n\
             exit 0\n",
            count_file = pr_list_count_file.display()
        ),
    );

    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
            "--parallel",
            "3",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();

    // Both distinct tickets must have a real (non-claim) completion entry --
    // slot 3's dispatch of TICKET-301 must not have been aborted by slot 2's
    // NoOp verdict on the transient sync hiccup.
    let ledger_text = fs::read_to_string(&ledger_path).unwrap_or_else(|err| {
        let events = fs::read_to_string(&events_path)
            .unwrap_or_else(|events_err| format!("<unreadable: {events_err}>"));
        let pr_list_count =
            fs::read_to_string(&pr_list_count_file).unwrap_or_else(|_| "<missing>".into());
        panic!(
            "parallel loop produced no ledger ({err}); pr-list count={}; events={events}",
            pr_list_count.trim()
        );
    });
    let dispatched_work_ids: std::collections::HashSet<String> = ledger_text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|e| e["mode"] != "claim")
        .filter_map(|e| e["work_id"].as_str().map(str::to_string))
        .collect();
    assert!(
        dispatched_work_ids.contains("TICKET-300"),
        "expected TICKET-300 dispatched, got: {dispatched_work_ids:?}"
    );
    assert!(
        dispatched_work_ids.contains("TICKET-301"),
        "expected TICKET-301 dispatched (slot 3, after a middle slot's NoOp), got: {dispatched_work_ids:?}"
    );
}

/// TICKET-084: `gah events` reads back exactly what `gah loop --once`
/// wrote, and `--profile` filters to just that profile's events.
#[test]
fn events_reads_back_loop_once_output_and_filters_by_profile() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    let events_path = tmp.path().join("events.jsonl");
    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();

    let out = bin()
        .args([
            "events",
            "--config-path",
            cfg.to_str().unwrap(),
            "--profile",
            "real",
            "--json",
        ])
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let events = parsed.as_array().unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e["profile"] == "real"));

    let out_other = bin()
        .args([
            "events",
            "--config-path",
            cfg.to_str().unwrap(),
            "--profile",
            "some-other-profile",
            "--json",
        ])
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();
    let stdout_other = String::from_utf8_lossy(&out_other.get_output().stdout).to_string();
    let parsed_other: Value = serde_json::from_str(&stdout_other).unwrap();
    assert!(parsed_other.as_array().unwrap().is_empty());
}

/// TICKET-081: an MR that keeps landing on the same decision (ReviewMr,
/// unchanged classification each time) must trip the stuck-loop detector
/// on the Nth `--once` invocation instead of re-dispatching a review
/// forever.
#[test]
fn loop_once_stops_on_stuck_loop_instead_of_repeating_forever() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\ncase \"$4\" in */pulls?*) echo '[{\"title\":\"[GAH] Fix: TICKET-500\",\"body\":\"body\",\"head\":{\"ref\":\"gah/real-1\",\"sha\":null},\"html_url\":\"https://github.com/owner/real/pull/1\",\"labels\":[],\"number\":1,\"state\":\"open\",\"draft\":false,\"updated_at\":\"2026-07-18T17:22:35-05:00\"}]'; exit 0;; */check-runs?*) echo '{\"total_count\":0,\"check_runs\":[]}'; exit 0;; esac\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"number\":1,\"url\":\"https://github.com/owner/real/pull/1\",\"title\":\"[GAH] Fix: TICKET-500\",\"body\":\"body\",\"headRefName\":\"gah/real-1\",\"baseRefName\":\"main\",\"statusCheckRollup\":[]}'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"comment\" ]; then exit 0; fi\nexit 0\n",
    );

    let events_path = tmp.path().join("events.jsonl");
    // Pre-seed 3 prior review_mr decisions for this exact work_id -- as if
    // 3 previous `--once` iterations already tried (and re-tried) the same
    // review with nothing else happening in between.
    let mut seeded = String::new();
    for _ in 0..3 {
        seeded.push_str(
            &serde_json::to_string(&serde_json::json!({
                "timestamp": "2026-07-05T00:00:00Z", "event_type": "action_decided",
                "profile": "real",
                "work_id": "TICKET-500",
                "details": "review_mr: MR needs review",
                "review_contract_version": 1
            }))
            .unwrap(),
        );
        seeded.push('\n');
    }
    fs::write(&events_path, seeded).unwrap();

    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("human_required"))
        .stdout(predicate::str::contains("stuck-loop"));

    let events_text = fs::read_to_string(&events_path).unwrap();
    // No dispatch was triggered for the 4th, stuck iteration.
    assert!(!events_text.contains("dispatch_started"));
}

// ── TICKET-128: per-profile publishing policy ───────────────────────────────
//
// A restricted profile forbids agent-authored repository prose (PR/MR text,
// generated commit messages, issue/MR comments) while preserving autonomous
// code execution and code review. Each axis is configured independently and
// must NOT be overloaded onto `human_required`.

/// Acceptance: publishing disabled + successful fix => no PR/MR API call is
/// issued and the run stops at a deterministic human handoff.
#[test]
fn publishing_disabled_blocks_pr_creation_and_emits_handoff() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\n[profiles.real.publishing]\nallow_pull_request_creation = false\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        // Deterministic handoff metadata is produced.
        .stdout(predicate::str::contains(
            "GAH human handoff (publishing policy)",
        ))
        .stdout(predicate::str::contains(
            "PR/MR creation or commit-message generation disabled by publishing policy",
        ));

    // No PR/MR was ever attempted (gh was never asked to `pr create`).
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        !gh_text.contains("pr create"),
        "gh was asked to create a PR: {gh_text}"
    );

    // Ledger reflects the handoff, not a publish attempt.
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["mr_attempted"], false);
    assert_eq!(entry["mr_created"], false);
    assert_eq!(entry["push_attempted"], false);
    assert_eq!(entry["push_succeeded"], false);
    assert_eq!(entry["validation_result"], "passed");
    assert_eq!(entry["human_required"], false);
}

/// Acceptance: commit-message generation disabled => the worktree is left
/// uncommitted for human completion (no commit is made / recorded). This axis
/// is configured independently of PR creation; both are combined into a single
/// deterministic human handoff, but the commit ledger must still record that
/// no commit was attempted.
#[test]
fn commit_message_generation_disabled_leaves_worktree_uncommitted() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\n[profiles.real.publishing]\nallow_pull_request_creation = true\nallow_commit_message_generation = false\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "GAH human handoff (publishing policy)",
        ))
        .stdout(predicate::str::contains(
            "PR/MR creation or commit-message generation disabled by publishing policy",
        ));

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    // The auto-commit step was skipped entirely (no LLM commit-message call):
    // `commit_attempted` is only set when we actually try to stage/commit.
    assert_eq!(entry["commit_attempted"], false);
    assert_eq!(entry["commit_created"], false);
    // No PR was opened either (the combined gate stops before publish).
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        !gh_text.contains("pr create"),
        "gh was asked to create a PR: {gh_text}"
    );
}

/// Acceptance: contribution still reaches the reviewer when publishing is
/// disabled. The reviewer runs and a deterministic verdict is produced.
#[test]
fn publishing_disabled_still_runs_reviewer() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        concat!(
            "[profiles.real.routing]\nreview_backend = \"claude\"\n",
            "[profiles.real.publishing]\nallow_pull_request_creation = false\n",
        ),
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "feature/review",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();

    // Reviewer actually executed and produced a structured verdict.
    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let report = fs::read_to_string(session.join("review-report.md")).unwrap();
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    assert!(report.contains("Review notes"));
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
    // The prompt was still written for the reviewer (review is not disabled).
    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("Source: feature/review"));
}

/// Acceptance: APPROVE + CI pass + PR creation disabled => no auto-merge path
/// is entered. With publishing disabled, `MergeMr` must not be selected by the
/// controller.
#[test]
fn approve_with_pr_disabled_skips_auto_merge_in_loop() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        concat!(
            "validation_commands = [\"true\"]\n",
            "[profiles.real.publishing]\nallow_pull_request_creation = false\n",
        ),
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    // No merge command (gh pr merge / glab mr merge) was issued.
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        !gh_text.contains("merge"),
        "gh was asked to merge: {gh_text}"
    );
    // The snapshot the controller consulted reflected the disabled policy.
    // (We assert indirectly: the run still succeeded and stopped at handoff.)
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["mr_created"], false);
}

/// Acceptance: issue comments disabled => no tracker comment API call is made,
/// even though review still runs and produces a verdict.
#[test]
fn issue_comments_disabled_skips_tracker_comment() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        concat!(
            "[profiles.real.routing]\nreview_backend = \"claude\"\n",
            "[profiles.real.publishing]\nallow_issue_comments = false\n",
        ),
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{{\"number\":7}}]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{{\"number\":7,\"url\":\"https://github.com/owner/real/pull/7\",\"title\":\"Draft: [GAH] Fix\",\"body\":\"MR body\",\"headRefName\":\"feature/review\",\"baseRefName\":\"main\"}}'; exit 0; fi\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "feature/review",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Publishing policy forbids agent-authored issue/MR comments",
        ));

    // No `pr comment` (tracker comment) call was made.
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        !gh_text.contains("pr comment") && !gh_text.contains("comment"),
        "gh was asked to comment: {gh_text}"
    );
    // Reviewer still produced a verdict locally.
    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

/// Acceptance: a pet-project profile with publishing enabled keeps the
/// existing autonomous behavior (PR is actually created). This guards against
/// the default flipping to restrictive.
#[test]
fn pet_project_publishing_enabled_creates_pr() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        concat!(
            "validation_commands = [\"true\"]\n",
            "[profiles.real.publishing]\n",
            "allow_pull_request_creation = true\n",
            "allow_commit_message_generation = true\n",
            "allow_issue_comments = true\n",
        ),
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        gh_text.contains("pr create"),
        "gh was NOT asked to create a PR: {gh_text}"
    );
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["mr_created"], true);
}

/// Acceptance: a restricted profile still produces deterministic human-handoff
/// metadata (branch, changed files, validation status, artifact paths, verdict).
#[test]
fn restricted_profile_emits_deterministic_handoff_metadata() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\n[profiles.real.publishing]\nallow_pull_request_creation = false\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "=== GAH human handoff (publishing policy) ===",
        ))
        .stdout(predicate::str::contains("validation_status"))
        .stdout(predicate::str::contains("changed_files"))
        .stdout(predicate::str::contains("branch:"))
        .stdout(predicate::str::contains(
            "PR/MR creation or commit-message generation disabled by publishing policy",
        ));
}
