use super::*;
use git_agent_harness::{config, ledger::LedgerEntry};

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_with_home(repo: &std::path::Path, home: &std::path::Path, args: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .args(args)
        .current_dir(repo)
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn setup_conflicted_repair(
    tmp: &TempDir,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let (repo, home, cfg) = setup_fix_dispatch_repo(tmp, "validation_commands = [\"true\"]\n");
    git_with_home(&repo, &home, &["checkout", "-b", "repair"]);
    fs::write(repo.join("README.md"), "repair version\n").unwrap();
    git_with_home(&repo, &home, &["add", "README.md"]);
    git_with_home(&repo, &home, &["commit", "-m", "repair version"]);
    git_with_home(&repo, &home, &["push", "-u", "origin", "repair"]);
    let source_sha = git_with_home(&repo, &home, &["rev-parse", "HEAD"]);

    git_with_home(&repo, &home, &["checkout", "main"]);
    fs::write(repo.join("README.md"), "target version\n").unwrap();
    git_with_home(&repo, &home, &["add", "README.md"]);
    git_with_home(&repo, &home, &["commit", "-m", "target version"]);
    git_with_home(&repo, &home, &["push", "origin", "main"]);

    let loaded = config::load(Some(cfg.to_str().unwrap())).unwrap();
    let profile = loaded.profiles.get("real").unwrap();
    let mut review = LedgerEntry::new(
        "real",
        profile,
        "claude",
        "review",
        "repair",
        Some("review-session".into()),
        None,
    );
    review.branch = Some("repair".into());
    review.work_id = Some("repair".into());
    review.review_verdict = Some("NEEDS_FIX".into());
    review.review_source_sha = Some(source_sha);
    review.review_blocking_findings = vec!["README.md: preserve both accepted versions".into()];
    review.reviewer_backend = Some("claude".into());
    review.reviewer_model = Some("sonnet".into());
    let ledger_path = tmp.path().join("ledger.jsonl");
    fs::write(
        &ledger_path,
        format!("{}\n", serde_json::to_string(&review).unwrap()),
    )
    .unwrap();

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        r#"#!/bin/sh
if [ "$1" = "api" ] && [ "$2" = "--method" ] && [ "$3" = "GET" ]; then printf '[{"number":7}]\n'; exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then printf '{"number":7,"url":"https://github.com/owner/real/pull/7","title":"Draft: repair","body":"repair","headRefName":"repair","baseRefName":"main","headRefOid":"abc","statusCheckRollup":[]}\n'; exit 0; fi
if [ "$1" = "api" ]; then exit 0; fi
echo "unexpected gh invocation: $*" >&2
exit 1
"#,
    );
    (repo, home, cfg, ledger_path)
}

fn dispatch_command(
    cfg: &std::path::Path,
    home: &std::path::Path,
    ledger_path: &std::path::Path,
    fake_bin: &std::path::Path,
) -> IsolatedCommand<Command> {
    let mut command = bin();
    command
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "repair",
            "--existing-branch",
            "repair",
            "--retries",
            "0",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(fake_bin))
        .env("HOME", home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", ledger_path);
    command
}

#[test]
fn conflicted_fix_is_resolved_validated_and_pushed_to_same_branch() {
    let tmp = test_tempdir();
    let (repo, home, cfg, ledger_path) = setup_conflicted_repair(&tmp);
    let fake_bin = tmp.path().join("bin");
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'target version\\nrepair version\\n' > README.md\ngit add README.md\nexit 0\n",
    );

    dispatch_command(&cfg, &home, &ledger_path, &fake_bin)
        .assert()
        .success()
        .stdout(predicate::str::contains("routing the live merge"))
        .stdout(predicate::str::contains("Validation passed"))
        .stdout(predicate::str::contains("Updated existing MR"));

    git_with_home(&repo, &home, &["fetch", "origin"]);
    let ancestor = ProcessCommand::new("git")
        .args([
            "merge-base",
            "--is-ancestor",
            "origin/main",
            "origin/repair",
        ])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(ancestor.success());
    assert_eq!(
        git(&repo, &["show", "origin/repair:README.md"]),
        "target version\nrepair version"
    );
    let entries = fs::read_to_string(&ledger_path).unwrap();
    let latest: Value = serde_json::from_str(entries.lines().last().unwrap()).unwrap();
    assert_eq!(latest["validation_result"], "passed");
    assert_eq!(latest["push_succeeded"], true);
    assert_eq!(latest["branch"], "repair");
}

#[test]
fn no_op_backend_reports_typed_unresolved_conflict_and_preserves_recovery() {
    let tmp = test_tempdir();
    let (_repo, home, cfg, ledger_path) = setup_conflicted_repair(&tmp);
    let fake_bin = tmp.path().join("bin");
    make_fake_bin_with_body(&fake_bin, "codex", "#!/bin/sh\nexit 0\n");

    dispatch_command(&cfg, &home, &ledger_path, &fake_bin)
        .assert()
        .failure()
        .stderr(predicate::str::contains("unresolved merge conflicts"));

    let entries = fs::read_to_string(&ledger_path).unwrap();
    let latest: Value = serde_json::from_str(entries.lines().last().unwrap()).unwrap();
    assert_eq!(latest["failure_class"], "agent_no_progress");
    assert_eq!(
        latest["validation_result"],
        "not_run_unresolved_merge_conflicts"
    );
    let session = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let recovery = session.join("conflict-recovery/attempt-1");
    assert!(recovery.join("manifest.txt").exists());
    assert!(recovery.join("working.patch").exists());
    assert!(recovery.join("files.json").exists());
    assert!(fs::read_to_string(recovery.join("files/0000.bin"))
        .unwrap()
        .contains("<<<<<<<"));
    assert!(!tmp.path().join("worktrees/repair").exists());
}

#[test]
fn shutdown_during_conflict_resolution_preserves_recovery_and_cleans_worktree() {
    let tmp = test_tempdir();
    let (_repo, home, cfg, ledger_path) = setup_conflicted_repair(&tmp);
    let backend = FakeBackend::new(tmp.path(), "codex");
    backend.install(Scenario::success().with_delay_ms(30_000));
    let events_path = tmp.path().join("events.jsonl");

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
            "repair",
            "--existing-branch",
            "repair",
            "--retries",
            "0",
            "--skip-validation-gate",
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

    wait_for_backend_call(&backend, &mut child, 1);
    #[cfg(unix)]
    send_signal(child.id(), libc::SIGTERM);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());

    let entries = fs::read_to_string(&ledger_path).unwrap();
    let latest: Value = serde_json::from_str(entries.lines().last().unwrap()).unwrap();
    assert_eq!(latest["validation_result"], "cancelled_shutdown");
    let session = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(session
        .join("conflict-recovery/attempt-1/manifest.txt")
        .exists());
    assert!(!tmp.path().join("worktrees/repair").exists());
    assert!(fs::read_to_string(events_path)
        .unwrap()
        .contains("shutdown requested while codex was running"));
}
