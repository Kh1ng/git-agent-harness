use super::*;

#[test]
fn stalled_before_changes_quarantines_the_exact_backend_route() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\ncodex_idle_timeout_seconds = 1\n",
    );
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(&fake_bin, "codex", "#!/bin/sh\nsleep 5\n");
    make_fake_bin_with_body(&fake_bin, "gh", "#!/bin/sh\nexit 0\n");
    let availability_path = tmp.path().join("availability.json");

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
            "prove stalled routes are quarantined",
            "--retries",
            "0",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("made no repository progress"));

    let availability = fs::read_to_string(availability_path).unwrap();
    assert!(availability.contains("\"backend\": \"codex\""));
    assert!(availability.contains("\"reason\": \"backend_outage\""));
    assert!(availability.contains("backend idle watchdog stalled; cooldown=15m"));
}

#[test]
fn stalled_validation_continues_checkpointed_progress_and_records_exact_outcome() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\ncodex_idle_timeout_seconds = 1\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");
    let counter = tmp.path().join("codex-call-count");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{counter}' ] && cat '{counter}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{counter}'\nif [ \"$n\" -eq 1 ]; then printf 'checkpointed-progress\\n' >> README.md; sleep 5; exit 17; fi\ngrep -q 'checkpointed-progress' README.md || exit 19\nprintf 'continued-after-stall\\n' >> README.md\nexit 0\n",
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
            "continue work after validation stalls",
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
    assert_eq!(
        entry["attempts"][0]["validation_result"],
        "not_run_backend_stalled_during_validation"
    );
    assert_eq!(entry["attempts"][0]["failure_class"], "harness_error");
    assert_eq!(entry["attempts"][1]["validation_result"], "passed");

    let branch = entry["branch"].as_str().unwrap();
    let readme = ProcessCommand::new("git")
        .args(["show", &format!("{branch}:README.md")])
        .current_dir(repo)
        .output()
        .unwrap();
    let readme = String::from_utf8_lossy(&readme.stdout);
    assert!(readme.contains("checkpointed-progress"));
    assert!(readme.contains("continued-after-stall"));
}

#[test]
fn shutdown_during_validation_preserves_wip_without_starting_a_retry() {
    let tmp = test_tempdir();
    let validation_started = tmp.path().join("validation-started");
    let (repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        &format!(
            "validation_commands = [\"if grep -q validation-shutdown-wip README.md; then touch {}; while :; do sleep 0.05; done; fi\"]\n",
            validation_started.display()
        ),
    );
    let ledger_path = tmp.path().join("ledger.jsonl");
    let counter = tmp.path().join("codex-call-count");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{counter}' ] && cat '{counter}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{counter}'\nprintf 'validation-shutdown-wip\\n' >> README.md\nexit 0\n",
            counter = counter.display(),
        ),
    );
    make_fake_bin_with_body(&fake_bin, "gh", "#!/bin/sh\nexit 0\n");

    let mut child = spawn_bin()
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
            "preserve validation shutdown work",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while !validation_started.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("dispatch exited before validation started: {status:?}");
        }
        assert!(Instant::now() < deadline, "validation did not start");
        thread::sleep(Duration::from_millis(20));
    }
    send_signal(child.id(), libc::SIGTERM);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(counter).unwrap().trim(), "1");

    let entry: Value = serde_json::from_str(
        fs::read_to_string(ledger_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(entry["attempts_started"], 1);
    assert_eq!(
        entry["attempts"][0]["validation_result"],
        "cancelled_shutdown"
    );
    assert_eq!(entry["attempts"][0]["failure_stage"], "post_validation");
    let branch = entry["branch"].as_str().unwrap();
    let readme = ProcessCommand::new("git")
        .args(["show", &format!("{branch}:README.md")])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&readme.stdout).contains("validation-shutdown-wip"));
}
