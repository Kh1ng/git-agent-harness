use super::*;

#[test]
fn dispatch_fix_validation_never_passes_records_no_push_no_mr() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"false\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "0",
            // TICKET-111: this test's baseline deliberately fails ("false")
            // to reach post-attempt validation-exhaustion behavior, not to
            // test baseline-stop policy itself.
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("validation failed after"));

    // No MR was ever attempted.
    assert!(!gh_log.exists() || !fs::read_to_string(&gh_log).unwrap().contains("pr create"));

    // The push never happened: the branch GAH created does not exist on origin.
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["push_attempted"], false);
    assert_eq!(entry["push_succeeded"], false);
    assert_eq!(entry["mr_attempted"], false);
    assert_eq!(entry["mr_created"], false);
    assert!(entry["error_summary"]
        .as_str()
        .unwrap()
        .contains("validation failed"));
    let branch = entry["branch"].as_str().unwrap();
    assert!(!branch_exists_on_bare_origin(
        &repo.parent().unwrap().join("github-root"),
        branch
    ));
    // TICKET-172: validation failure must leave the generated patch on the
    // local dispatch branch for recovery, even though no push/MR occurred.
    let recovered = ProcessCommand::new("git")
        .args(["show", &format!("{branch}:README.md")])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(recovered.status.success(), "WIP branch {branch} was lost");
    assert!(
        String::from_utf8_lossy(&recovered.stdout).contains("agent edit"),
        "terminal validation failure should retain the agent's patch"
    );
}

/// TICKET-237: OpenCode can report provider rate limits only in its own
/// internal log while returning exit 0 and leaving no diff. That must be
/// classified as a backend availability failure, not agent_no_progress, and
/// the bounded retry must select the configured fallback in the same dispatch.
#[test]
fn dispatch_fix_opencode_internal_rate_limit_marks_unavailable_and_reroutes() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let config = fs::read_to_string(&cfg).unwrap().replace(
        "improve_backend = \"codex\"",
        "improve_backend = \"opencode\"\nimprove_candidates = [{ backend = \"opencode\", model = \"opencode/hy3-free\" }, { backend = \"codex\", model = \"gpt-5.4-mini\" }]",
    );
    fs::write(&cfg, config).unwrap();

    let ledger_path = tmp.path().join("ledger.jsonl");
    let availability_path = tmp.path().join("availability.json");
    let data_home = tmp.path().join("xdg-data");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "opencode",
        "#!/bin/sh\nmkdir -p \"$XDG_DATA_HOME/opencode/log\"\nprintf '%s\\n' 'timestamp=now level=ERROR message=\"AI_APICallError: Rate limit exceeded. Please try again later.\"' >> \"$XDG_DATA_HOME/opencode/log/opencode.log\"\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'fallback edit\\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nprintf 'https://github.com/owner/real/pull/1\\n'\n",
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
            "--retries",
            "1",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &data_home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Backend unavailable after no-progress result; retrying next attempt with codex/gpt-5.4-mini instead of opencode/opencode/hy3-free",
        ));

    let availability: Value =
        serde_json::from_str(&fs::read_to_string(&availability_path).unwrap()).unwrap();
    let records = availability["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["backend"], "opencode");
    assert_eq!(records[0]["model"], "opencode/hy3-free");
    assert_eq!(records[0]["reason"], "rate_limited");

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "codex");
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["backend"], "opencode");
    assert_eq!(attempts[0]["failure_class"], "backend_error");
    assert_eq!(
        attempts[0]["validation_result"],
        "not_run_backend_unavailable"
    );
    assert_eq!(attempts[1]["backend"], "codex");
    assert_eq!(attempts[1]["validation_result"], "passed");
}

#[test]
fn dispatch_reroute_continues_partial_tree_after_billing_exhaustion() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let config = fs::read_to_string(&cfg).unwrap().replace(
        "improve_backend = \"codex\"",
        "improve_backend = \"opencode\"\nimprove_candidates = [{ backend = \"opencode\", model = \"opencode/hy3-free\" }, { backend = \"codex\", model = \"gpt-5.4-mini\" }]",
    );
    fs::write(&cfg, config).unwrap();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let availability_path = tmp.path().join("availability.json");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "opencode",
        "#!/bin/sh\nprintf 'opencode-partial-progress\\n' >> README.md\nprintf 'Forbidden: Sorry, your account balance is insufficient\\n' >&2\nexit 1\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\ngrep -q 'opencode-partial-progress' README.md || exit 19\nprintf 'codex-completed-progress\\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; fi\nexit 0\n",
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
            "continue rerouted work",
            "--retries",
            "1",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Backend unavailable; retrying next attempt with codex/gpt-5.4-mini instead of opencode/opencode/hy3-free (QuotaExhausted)",
        ));

    let entry: Value = serde_json::from_str(
        fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(entry["attempts"][0]["backend"], "opencode");
    assert_eq!(entry["attempts"][1]["backend"], "codex");
    let branch = entry["branch"].as_str().unwrap();
    let readme = ProcessCommand::new("git")
        .args(["show", &format!("{branch}:README.md")])
        .current_dir(repo)
        .output()
        .unwrap();
    let readme = String::from_utf8_lossy(&readme.stdout);
    assert!(readme.contains("opencode-partial-progress"));
    assert!(readme.contains("codex-completed-progress"));

    let availability: Value =
        serde_json::from_str(&fs::read_to_string(availability_path).unwrap()).unwrap();
    assert_eq!(availability["records"][0]["reason"], "quota_exhausted");
}

/// TICKET-250: no-progress uses the same bounded retry policy as other
/// recoverable agent failures. Every failed no-change attempt remains visible
/// in the ledger, and only the final one marks the dispatch terminally failed.
#[test]
fn dispatch_fix_retries_no_change_before_terminal_failure() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let invocation_log = tmp.path().join("codex-invocations.log");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nprintf 'attempt\\n' >> \"{}\"\nexit 0\n",
            invocation_log.display()
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
            "--retries",
            "1",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "attempt 2 but produced no worktree changes",
        ));

    assert_eq!(
        fs::read_to_string(&invocation_log).unwrap().lines().count(),
        2
    );
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "agent_no_progress");
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().all(|attempt| {
        attempt["failure_class"] == "agent_no_progress"
            && attempt["validation_result"] == "not_run_no_changes"
    }));
}

/// TICKET-172: retry cleanup must not destroy a failed attempt's patch. The
/// retry starts clean, so its final WIP belongs on the dispatch branch while
/// the previous attempt remains reachable from a dedicated local checkpoint.
#[test]
fn dispatch_fix_validation_retry_retains_each_failed_wip_tree() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        r#"validation_commands = ["sh -c 'if grep -q \"first attempt\" README.md; then echo first; false; elif grep -q \"second attempt\" README.md; then echo second; false; else echo baseline; false; fi'"]
"#,
    );
    let ledger_path = tmp.path().join("ledger.jsonl");
    let fake_bin = tmp.path().join("bin");
    let invocation_marker = tmp.path().join("agent-ran-once");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nif test -f '{marker}'; then printf 'second attempt\\n' >> README.md; else touch '{marker}'; printf 'first attempt\\n' >> README.md; fi\nexit 0\n",
            marker = invocation_marker.display(),
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
            "--retries",
            "1",
            "--skip-validation-gate",
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "validation failed after 2 attempt",
        ));

    let entry: Value = serde_json::from_str(
        fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let dispatch_branch = entry["branch"].as_str().unwrap();
    let dispatch_tree = ProcessCommand::new("git")
        .args(["show", &format!("{dispatch_branch}:README.md")])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(dispatch_tree.status.success());
    assert!(String::from_utf8_lossy(&dispatch_tree.stdout).contains("second attempt"));

    let checkpoints = ProcessCommand::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/gah-wip",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    let checkpoint = String::from_utf8_lossy(&checkpoints.stdout)
        .lines()
        .next()
        .expect("first failed retry should leave a WIP checkpoint")
        .to_string();
    let checkpoint_tree = ProcessCommand::new("git")
        .args(["show", &format!("{checkpoint}:README.md")])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(checkpoint_tree.status.success());
    assert!(String::from_utf8_lossy(&checkpoint_tree.stdout).contains("first attempt"));
}

/// TICKET-064, test 1: a one-shot success (no validation failures at all)
/// must record exactly one attempt, started and completed.
#[test]
fn dispatch_fix_one_shot_success_records_one_attempt() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
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

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["attempts_started"], 1);
    assert_eq!(entry["attempts_completed"], 1);
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["attempt_number"], 1);
    assert_eq!(attempts[0]["validation_result"], "passed");
    assert_eq!(attempts[0]["failure_class"], Value::Null);
}

/// TICKET-073: a config change (here, the very first run, since nothing is
/// recorded yet) must trigger exactly one fresh-worktree self-check, record
/// the new hash + last_verified_ok=true, and a *second* dispatch with an
/// unchanged validation_commands list must take the fast path (hash compare
/// only) — no "[validation-gate] commands changed" message, no second worktree
/// spin-up.
#[test]
fn dispatch_runs_validation_gate_once_per_config_change_then_skips() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let state_path = tmp.path().join("validation_check.json");

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
        "#!/bin/sh\nprintf 'https://github.com/owner/real/pull/1\\n'\n",
    );

    let run = || -> assert_cmd::assert::Assert {
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
                "noop",
                "--retries",
                "0",
            ])
            .env("PATH", prepend_path(&fake_bin))
            .env("HOME", &home)
            .env("GITHUB_TOKEN", "token")
            .env("GAH_VALIDATION_CHECK_PATH", &state_path)
            .assert()
    };

    // First dispatch: nothing recorded yet → gate runs, passes, records.
    // The gate logs to stdout.
    let first = run();
    first
        .success()
        .stdout(predicate::str::contains(
            "[validation-gate] commands changed",
        ))
        .stdout(predicate::str::contains("Baseline validation on pristine worktree").not());

    // State now records last_verified_ok = true for profile "real".
    let state_text = fs::read_to_string(&state_path).unwrap();
    assert!(
        state_text.contains("\"last_verified_ok\": true") && state_text.contains("\"real\""),
        "gate should have recorded a passing check: {}",
        state_text
    );

    // Second dispatch: config unchanged → fast path, no gate re-run message.
    // Sleep 1s so the dispatch worktree branch timestamp differs from the
    // first run (the previous worktree is cleaned up but its branch ref
    // lingers until pruned) and the two runs don't collide.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let second = run();
    second
        .success()
        .stdout(predicate::str::contains("[validation-gate] commands changed").not())
        .stdout(predicate::str::contains("Baseline validation on pristine worktree").not());
}

/// TICKET-073: --skip-validation-gate deliberately bypasses the gate even when
/// validation_commands is broken, recording nothing new and letting dispatch
/// proceed (so an operator who has acknowledged a known-broken gate can still
/// dispatch real work).
#[test]
fn dispatch_skip_validation_gate_bypasses_gate() {
    let tmp = test_tempdir();
    // validation_commands passes baseline; we are only testing that the
    // --skip-validation-gate opt-out suppresses the gate self-check entirely.
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let state_path = tmp.path().join("validation_check.json");

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
        "#!/bin/sh\nprintf 'https://github.com/owner/real/pull/1\\n'\n",
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
            "noop",
            "--retries",
            "0",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_VALIDATION_CHECK_PATH", &state_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "[validation-gate] skipped by explicit --skip-validation-gate",
        ));

    // Bypass means no check was recorded for this profile.
    assert!(
        !state_path.exists()
            || !fs::read_to_string(&state_path)
                .unwrap()
                .contains("\"real\""),
        "skipping the gate must not record a check for the profile"
    );
}

/// TICKET-101: usage the backend reports on stdout for a given attempt is
/// captured onto that specific attempt record, not just aggregated
/// somewhere else -- and a backend that reports nothing leaves it
/// genuinely unknown (None), never a fabricated zero.
#[test]
fn dispatch_fix_records_per_attempt_usage_from_backend_output() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nprintf 'input_tokens: 500\\noutput_tokens: 120\\nestimated_cost_usd: 0.02\\n'\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
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

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["usage"]["input_tokens"], 500);
    assert_eq!(attempts[0]["usage"]["output_tokens"], 120);
    assert_eq!(attempts[0]["usage"]["total_tokens"], 620);
    assert_eq!(attempts[0]["usage"]["estimated_cost_usd"], Value::Null);
}

#[test]
fn dispatch_fix_shutdown_records_cancelled_shutdown_and_dispatch_finished_event() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    let codex = FakeBackend::new(tmp.path(), "codex");
    codex.install(Scenario::success().with_delay_ms(30_000));
    make_fake_bin_with_body(
        &tmp.path().join("bin"),
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
    );

    // Keep the isolated environment alive until the spawned process exits.
    let mut command = spawn_bin();
    let mut child = command
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
        .env("PATH", prepend_path(&tmp.path().join("bin")))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    wait_for_backend_call(&codex, &mut child, 1);
    #[cfg(unix)]
    send_signal(child.id(), libc::SIGINT);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(codex.call_count(), 1);

    let ledger_text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(ledger_text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "harness_error");
    assert_eq!(entry["failure_stage"], "agent_run");
    assert_eq!(entry["validation_result"], "cancelled_shutdown");
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["failure_class"], "harness_error");
    assert_eq!(attempts[0]["failure_stage"], "agent_run");
    assert_eq!(attempts[0]["validation_result"], "cancelled_shutdown");

    let events_text = fs::read_to_string(&events_path).unwrap();
    assert!(events_text.contains("dispatch_started"));
    assert!(events_text.contains("dispatch_finished"));
    assert!(events_text.contains("shutdown requested while codex was running"));
}

/// TICKET-079: --escalate seeds the *initial* route decision (not just an
/// internal retry) as a genuine agent-capability failure, so the same
/// TICKET-089 cost-aware escalation logic picks the stronger candidate on
/// the very first attempt.
#[test]
fn dispatch_fix_escalate_flag_picks_stronger_backend_on_first_attempt() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    // setup_fix_dispatch_repo always appends its own single-line
    // `[profiles.real.routing]` table (a second header would be invalid
    // TOML), so patch the candidate list into that same table afterward
    // instead of trying to inject a second `[profiles.real.routing]`.
    let cfg_text = fs::read_to_string(&cfg).unwrap();
    let cfg_text = cfg_text.replace(
        "improve_backend = \"codex\"",
        "improve_backend = \"codex\"\nimprove_candidates = [{ backend = \"openhands\", model = \"deepseek-flash\" }, { backend = \"codex\", model = \"gpt-5.4\" }]",
    );
    fs::write(&cfg, cfg_text).unwrap();
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "openhands",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "auto",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
            "--escalate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "codex");
    assert_eq!(entry["effective_model"], "gpt-5.4");
}

/// TICKET-064, test 2: an attempt that fails validation (differently from
/// baseline, so it retries) followed by a passing attempt must record
/// exactly two attempts, with attempt 1's failure and attempt 2's success
/// both preserved — not just the final outcome.
#[test]
fn dispatch_fix_fail_then_success_records_two_attempts() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"cat marker.txt; grep -q '^done$' marker.txt\"]\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let counter = tmp.path().join("codex-call-count");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{counter}' ] && cat '{counter}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{counter}'\nif [ \"$n\" -eq 1 ]; then echo partial > marker.txt; else echo done > marker.txt; fi\nexit 0\n",
            counter = counter.display(),
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
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
            "--skip-validation-gate",
            "--target",
            "fix the marker file",
            "--retries",
            "2",
            // TICKET-111: baseline fails (marker.txt missing on pristine
            // tree) purely to set up the retry-loop scenario under test.
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["attempts_started"], 2);
    assert_eq!(entry["attempts_completed"], 2);
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["attempt_number"], 1);
    assert_eq!(attempts[0]["validation_result"], "failed");
    assert_eq!(attempts[0]["failure_class"], "validation_failure");
    assert!(attempts[0]["diff_path"]
        .as_str()
        .unwrap()
        .contains("attempt-diff.patch"));
    assert_eq!(attempts[1]["attempt_number"], 2);
    assert_eq!(attempts[1]["validation_result"], "passed");
}

#[test]
fn dispatch_backend_retry_continues_checkpointed_progress() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let counter = tmp.path().join("codex-call-count");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{counter}' ] && cat '{counter}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{counter}'\nif [ \"$n\" -eq 1 ]; then printf 'first-attempt-progress\\n' >> README.md; exit 17; fi\ngrep -q 'first-attempt-progress' README.md || exit 19\nprintf 'second-attempt-completion\\n' >> README.md\nexit 0\n",
            counter = counter.display(),
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; fi\nexit 0\n",
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
            "--skip-validation-gate",
            "--target",
            "continue partial backend work",
            "--retries",
            "1",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let entry: Value = serde_json::from_str(
        fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(entry["attempts_started"], 2);
    assert_eq!(entry["attempts"][0]["exit_code"], 17);
    assert_eq!(entry["attempts"][1]["validation_result"], "passed");
    let branch = entry["branch"].as_str().unwrap();
    let readme = ProcessCommand::new("git")
        .args(["show", &format!("{branch}:README.md")])
        .current_dir(repo)
        .output()
        .unwrap();
    let readme = String::from_utf8_lossy(&readme.stdout);
    assert!(readme.contains("first-attempt-progress"));
    assert!(readme.contains("second-attempt-completion"));
}

/// TICKET-064, test 3: a no-progress abort (TICKET-062) must record exactly
/// the attempts that were actually consumed, not the full retry budget.
/// `--retries 2` gives 3 attempts available; only 1 should be consumed
/// since attempt 1 already matches the baseline.
#[test]
fn dispatch_fix_no_progress_abort_records_exact_consumed_attempts() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"false\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "2",
            // TICKET-111: baseline fails ("false") to set up the
            // no-progress / attempt-matches-baseline scenario under test.
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["attempts_started"], 1);
    assert_eq!(entry["attempts_completed"], 1);
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["failure_class"], "agent_no_progress");
    assert_eq!(entry["failure_class"], "agent_no_progress");
}

/// TICKET-062: a validation failure identical to the pristine-tree baseline
/// on attempt 1 must abort immediately — there is no "previous attempt" yet
/// to compare against, so the old prev_failure-only comparison couldn't
/// catch this and would burn a second paid attempt for free. `--retries 2`
/// (3 attempts available) proves only ONE was actually consumed: no
/// attempt-2 session directory is ever created.
#[test]
fn dispatch_fix_aborts_on_first_attempt_when_failure_matches_baseline() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"false\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );

    let out = bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "2",
            // TICKET-111: baseline fails ("false") to set up the
            // no-progress / attempt-matches-baseline scenario under test.
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("pristine-tree baseline"));

    // Only attempt-1 ever ran. If the baseline/previous-attempt distinction
    // regressed back to prev_failure-only comparison, this would burn a
    // second attempt before aborting and attempt-2 would exist.
    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(session_dir.join("attempt-1").exists());
    assert!(!session_dir.join("attempt-2").exists());

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["push_attempted"], false);
    let _ = out;
}

/// TICKET-062, test case 4: an "expected red" ticket — where the baseline
/// is genuinely broken and the ticket's job is to fix it — must still be
/// able to succeed. Attempt 1 changes the failure text (real progress, not
/// a no-op), so it must retry rather than abort; attempt 2 then passes.
///
/// Also exercises TICKET-110/111's real `BaselineDisposition::ExpectedRed`:
/// the profile explicitly configures `known_baseline_failure_markers`
/// matching the missing-file text, so dispatch proceeds instead of
/// stopping (the default for an unconfigured/unknown_red baseline).
#[test]
fn dispatch_fix_expected_red_baseline_can_still_succeed() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"cat marker.txt; grep -q '^done$' marker.txt\"]\nknown_baseline_failure_markers = [\"marker.txt: No such file or directory\"]\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    // marker.txt does not exist on the pristine branch, so the baseline
    // validation fails ("No such file or directory"). The fake backend
    // tracks its own call count in a file outside the worktree (the
    // worktree gets git-reset between attempts) and writes progressively
    // closer output: attempt 1 writes "partial" (still fails, but with
    // different captured output than the missing-file baseline — real
    // progress); attempt 2 writes "done" (passes).
    let counter = tmp.path().join("codex-call-count");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{counter}' ] && cat '{counter}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{counter}'\nif [ \"$n\" -eq 1 ]; then echo partial > marker.txt; else echo done > marker.txt; fi\nexit 0\n",
            counter = counter.display(),
        ),
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
            "--skip-validation-gate",
            "--target",
            "fix the marker file",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(session_dir.join("attempt-1").exists());
    assert!(session_dir.join("attempt-2").exists());

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["validation_result"], "passed");
    assert!(gh_log.exists());
    assert!(fs::read_to_string(&gh_log).unwrap().contains("pr create"));
    let _ = repo;
}

/// TICKET-111 AC1: a harness_error baseline (validation command itself
/// cannot run -- POSIX exit 127) must stop dispatch before any attempt runs,
/// regardless of --allow-unknown-red-baseline (that flag only covers
/// unknown_red, not harness/environment errors).
#[test]
fn dispatch_fix_harness_error_baseline_always_stops() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"definitely-not-a-real-command-xyz\"]\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "2",
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("harness_error"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(
        !session_dir.join("attempt-1").exists(),
        "no attempt should ever run when the baseline is a harness_error"
    );
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "harness_error");
    assert_eq!(entry["failure_stage"], "baseline_validation");
}

/// TICKET-111 AC1: an environment_error baseline (well-known missing-
/// dependency signature) must also stop dispatch before any attempt runs.
#[test]
fn dispatch_fix_environment_error_baseline_stops() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"echo 'ModuleNotFoundError: No module named repo_thing'; exit 1\"]\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("environment_error"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(!session_dir.join("attempt-1").exists());
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "environment_error");
}

/// TICKET-111 AC2: unknown_red (a baseline failure matching none of the
/// known signatures, and not explicitly configured as expected) stops by
/// default -- proving --allow-unknown-red-baseline in the other tests in
/// this file is opting into real, non-default behavior rather than masking
/// a no-op.
#[test]
fn dispatch_fix_unknown_red_baseline_stops_without_override() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"false\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("unknown_red")
                .and(predicate::str::contains("--allow-unknown-red-baseline")),
        );

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(!session_dir.join("attempt-1").exists());
}

/// TICKET-063: a representative dispatch failure (backend exits nonzero)
/// must populate structured failure_class/failure_stage on the ledger
/// entry, not just the old free-text error_summary.
#[test]
fn dispatch_fix_backend_nonzero_exit_records_structured_failure_attribution() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(&fake_bin, "codex", "#!/bin/sh\nexit 1\n");

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
        .failure();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "backend_error");
    assert_eq!(entry["failure_stage"], "agent_run");
}

/// Priority-3 coverage: the git push can genuinely succeed while the
/// provider CLI (MR creation) fails afterward. That is a real partial
/// completion, not a false success — the ledger must show push_succeeded
/// true and mr_created false, and the CLI must still exit nonzero with the
/// provider's own error text surfaced.
#[test]
fn dispatch_fix_provider_cli_nonzero_after_successful_push() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "");
    let ledger_path = tmp.path().join("ledger.jsonl");

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
        "#!/bin/sh\necho 'insufficient permission to create pr' >&2\nexit 1\n",
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
        .failure()
        .stderr(predicate::str::contains(
            "insufficient permission to create pr",
        ));

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["push_attempted"], true);
    assert_eq!(entry["push_succeeded"], true);
    assert_eq!(entry["mr_attempted"], true);
    assert_eq!(entry["mr_created"], false);
    let branch = entry["branch"].as_str().unwrap();
    assert!(branch_exists_on_bare_origin(
        &repo.parent().unwrap().join("github-root"),
        branch
    ));
}

#[test]
fn dispatch_dry_run_ticket_metadata_feeds_routing() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let availability_path = tmp.path().join("availability.json");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let ticket = tmp.path().join("ticket.md");
    fs::write(
        &ticket,
        "Difficulty: medium\nRisk: low\nRecommended backend: codex\nRecommended model: test-model\n",
    )
    .unwrap();
    let cfg = write_real_repo_config_with_extra(&tmp, &repo, "github", "", "");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "codex");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "improve",
            "--target",
            ticket.to_str().unwrap(),
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("Effective:    codex"))
        .stdout(predicate::str::contains("LLM model:").not())
        .stdout(predicate::str::contains("LLM base:").not());
}
