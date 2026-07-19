use super::*;

/// TICKET-250: an exit-0 backend that leaves the worktree unchanged has
/// consumed a real attempt but made no ticket progress. It must be surfaced as
/// agent_no_progress, never as a successful no-op dispatch.
#[test]
fn dispatch_fix_no_change_is_agent_no_progress() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(&fake_bin, "codex", "#!/bin/sh\nexit 0\n");

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
            "0",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("produced no worktree changes"));

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "agent_no_progress");
    assert_eq!(entry["failure_stage"], "agent_run");
    assert_eq!(entry["validation_result"], "not_run_no_changes");
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["exit_code"], 0);
    assert_eq!(attempts[0]["failure_class"], "agent_no_progress");
    assert_eq!(attempts[0]["validation_result"], "not_run_no_changes");
}

#[test]
fn dispatch_fix_grounded_already_satisfied_closes_issue_without_mr() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\n[profiles.real.publishing]\nallow_source_issue_closure = true\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        r#"#!/bin/sh
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"GAH_DISPOSITION: already_satisfied\nfile:README.md\ntest:true"}}'
printf '%s\n' '{"type":"turn.completed"}'
exit 0
"#,
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        r###"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/real/issues/42") printf '{"number":42,"title":"Already complete","body":"Acceptance criteria already met","labels":[],"user":{"login":"owner","type":"User"},"state":"open"}\n' ;;
  "api --method GET repos/owner/real/issues/42/comments") printf '[]\n' ;;
  "api --method POST repos/owner/real/issues/42/comments") echo comment >> "$0.calls" ;;
  "api repos/owner/real/issues/42 --jq .state") printf 'open\n' ;;
  "issue close 42 --repo") echo close >> "$0.calls" ;;
  "pr create"*) echo "unexpected PR creation" >&2; exit 1 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
exit 0
"###,
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
            "#42",
            "--retries",
            "0",
            "--skip-validation-gate",
            "--issue-intake-override",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let calls = fs::read_to_string(fake_bin.join("gh.calls")).unwrap();
    assert_eq!(calls, "comment\nclose\n");
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "already_satisfied");
    assert_eq!(entry["validation_result"], "already_satisfied");
    assert_eq!(entry["mr_attempted"], false);
    assert_eq!(entry["attempts"][0]["failure_class"], "already_satisfied");
}
