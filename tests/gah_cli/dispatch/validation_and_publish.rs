use crate::*;

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

#[test]
fn ledger_summary_json_outputs_machine_readable_counts() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    // Empty ledger: still valid JSON with zero counts
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
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(parsed["entries"], 0);
    assert!(parsed["by_mode"].is_object());
    assert!(parsed["by_backend"].is_object());
}

#[test]
fn ledger_summary_json_includes_model_and_failure_class_breakdown() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    fs::write(
        &ledger_path,
        "{\"timestamp\":\"2099-01-01T00:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"claude\",\"requested_backend\":\"claude\",\"effective_backend\":\"claude\",\"requested_model\":null,\"effective_model\":\"claude-sonnet-4\",\"routing_reason\":\"explicit\",\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":true,\"failure_class\":\"agent_failure\",\"mode\":\"pm\",\"target_summary\":\"x\",\"branch\":null,\"session_dir\":null,\"duration_seconds\":1.0,\"backend_exit_code\":0,\"validation_result\":\"not_run\",\"commit_attempted\":false,\"commit_created\":false,\"push_attempted\":false,\"push_succeeded\":false,\"mr_attempted\":false,\"mr_created\":false,\"mr_url\":null,\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"usage_source\":null}}\n",
    )
    .unwrap();

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
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(parsed["entries"], 1);
    assert_eq!(parsed["by_model"]["claude-sonnet-4"], 1);
    assert_eq!(parsed["by_failure_class"]["agent_failure"], 1);
    assert_eq!(parsed["human_required_count"], 1);
}

#[test]
fn dispatch_agy_multi_instance_isolated_execution() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();

    // Setup git stub, just in case
    make_fake_bin_with_body(&fake_bin, "gh", "#!/bin/sh\nexit 0\n");

    // Let's create fake binaries for agy, agy-main, and agy-second.
    // They will write distinct strings to files under tmp so we can verify they executed.
    let agy_log = tmp.path().join("agy.log");
    let agy_main_log = tmp.path().join("agy_main.log");
    let agy_second_log = tmp.path().join("agy_second.log");

    make_fake_bin_with_body(
        &fake_bin,
        "agy",
        &format!(
            "#!/bin/sh\necho \"agy\" | tee -a \"{}\"\nprintf 'agent edit\n' >> README.md\nexit 0\n",
            agy_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "agy-main",
        &format!(
            "#!/bin/sh\necho \"agy-main\" | tee -a \"{}\"\nprintf 'agent edit\n' >> README.md\nexit 0\n",
            agy_main_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "agy-second",
        &format!(
            "#!/bin/sh\necho \"agy-second\" | tee -a \"{}\"\nprintf 'agent edit\n' >> README.md\nexit 0\n",
            agy_second_log.display()
        ),
    );

    // 1. Dispatch with backend agy-main
    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "agy-main",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "test target",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    // Verify agy-main was executed, and ledger recorded "agy-main"
    assert!(agy_main_log.exists());
    assert!(!agy_second_log.exists());
    assert!(!agy_log.exists());

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "agy-main");

    // Clear ledger for the next check
    let _ = fs::remove_file(&ledger_path);

    // Sleep for 1.1s to avoid timestamp/worktree conflict
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // 2. Dispatch with backend agy-second
    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "agy-second",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "test target",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    // Verify agy-second was executed, and ledger recorded "agy-second"
    assert!(agy_second_log.exists());
    assert!(!agy_log.exists());

    let text2 = fs::read_to_string(&ledger_path).unwrap();
    let entry2: Value = serde_json::from_str(text2.lines().next().unwrap()).unwrap();
    assert_eq!(entry2["effective_backend"], "agy-second");

    // Clear ledger again
    let _ = fs::remove_file(&ledger_path);

    // Sleep for 1.1s to avoid timestamp/worktree conflict
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // 3. Dispatch with backend agy (fallback)
    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "agy",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "test target",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    // Verify agy was executed, and ledger recorded "agy"
    assert!(agy_log.exists());

    let text3 = fs::read_to_string(&ledger_path).unwrap();
    let entry3: Value = serde_json::from_str(text3.lines().next().unwrap()).unwrap();
    assert_eq!(entry3["effective_backend"], "agy");
    assert_eq!(
        entry3["attempts"][0]["usage"]["quota_window"],
        "AGY individual quota"
    );
    assert_eq!(entry3["usage"]["quota_window"], "AGY individual quota");
}

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
