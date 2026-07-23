use super::*;

#[test]
fn dispatch_dry_run_improve_prints_plan() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("DRY RUN"))
        .stdout(predicate::str::contains("origin/main"))
        .stdout(predicate::str::contains("gah/test-repo-"));
}

#[test]
fn dispatch_dry_run_shows_backend_in_plan() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--backend",
            "claude",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude"));
}

#[test]
fn dispatch_dry_run_shows_oh_profile_when_given() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--oh-profile",
            "some-profile",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("some-profile"));
}

#[test]
fn dispatch_dry_run_pm_mode_prints_pm_steps() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "pm",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pm-report.md"));
}

#[test]
fn dispatch_unknown_mode_fails() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "bogus-mode",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .failure();
}

#[test]
fn dispatch_dry_run_shows_validation_commands() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config_with_validation(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "validated-repo",
            "--mode",
            "improve",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Validation"))
        .stdout(predicate::str::contains("cargo test --quiet"));
}

#[test]
fn dispatch_dry_run_shows_retries_in_plan() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config_with_validation(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "validated-repo",
            "--mode",
            "improve",
            "--retries",
            "3",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Retries:      3"));
}

#[test]
fn dispatch_dry_run_candidate_json_target_labeled() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    let fake_candidates = tmp.path().join("candidates.json");
    // Write a minimal valid candidates.json so build_task identifies it
    fs::write(
        &fake_candidates,
        r#"{"counts":{"seen":1,"converted":1,"skipped_warning":0},"candidates":[]}"#,
    )
    .unwrap();
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--target",
            fake_candidates.to_str().unwrap(),
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("candidate JSON"));
}

#[test]
fn dispatch_dry_run_allow_draft_fail_shown() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--allow-draft-fail",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Allow draft fail: true"));
}

#[test]
fn dispatch_dry_run_oh_profile_shows_model() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--oh-profile",
            "my-profile",
            "--model",
            "custom/model-name",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("OH profile:   my-profile"))
        .stdout(predicate::str::contains(
            "Model override: custom/model-name",
        ));
}

#[test]
fn dispatch_dry_run_model_override_shows_custom_model() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--model",
            "custom/test-model",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("custom/test-model"));
}

#[test]
fn dispatch_dry_run_oh_profile_does_not_pass_profile_flag() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    // The dry-run output must not contain "--profile" as an OpenHands argument.
    // It only shows the GAH --oh-profile flag which is a different thing.
    let output = bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--oh-profile",
            "some-profile",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    // GAH --oh-profile IS shown in dry-run output
    assert!(stdout.contains("some-profile"), "oh-profile should appear");
    // But there should be no mention of --profile being passed to openhands
    // The dry-run shows: openhands --headless --json -t ... (no --profile)
    let openhands_line = stdout.lines().find(|l| l.contains("openhands --headless"));
    if let Some(line) = openhands_line {
        assert!(
            !line.contains("--profile"),
            "OpenHands arg line must not contain --profile"
        );
    }
}

#[test]
fn dispatch_pm_writes_ledger_entry() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "pm",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success();

    let ledger = tmp.path().join("artifacts/ledger.jsonl");
    let text = fs::read_to_string(ledger).unwrap();
    assert!(text.contains("\"profile\":\"real\""));
    assert!(text.contains("\"mode\":\"pm\""));
}

#[test]
fn dispatch_records_effective_model_for_routed_runs() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let bare = repo.parent().unwrap().join("origin.git");
    ProcessCommand::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["remote", "add", "origin", bare.to_str().unwrap()])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let ticket = tmp.path().join("ticket.md");
    fs::write(
        &ticket,
        "# Ticket\n\nRecommended backend: claude\nRecommended model: claude-sonnet-4\n",
    )
    .unwrap();
    // This test verifies route attribution, not publication. The fixture
    // deliberately uses an illustrative GitHub remote, so keep publication
    // disabled once the backend creates the minimal diff required by the
    // no-progress guard.
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.publishing]\nallow_pull_request_creation = false\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "improve",
            "--target",
            ticket.to_str().unwrap(),
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();

    let ledger = tmp.path().join("artifacts/ledger.jsonl");
    let text = fs::read_to_string(ledger).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "claude");
    assert_eq!(entry["effective_model"], "claude-sonnet-4");
}

#[test]
fn prune_dry_run_reports_old_sessions_and_worktrees() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let session = tmp.path().join("artifacts/real/sessions/20240101");
    fs::create_dir_all(&session).unwrap();

    let worktree_root = tmp.path().join("worktrees");
    fs::create_dir_all(&worktree_root).unwrap();
    let worktree = worktree_root.join("gah-real-old");
    ProcessCommand::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "gah/real-old",
            worktree.to_str().unwrap(),
            "HEAD",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();

    ProcessCommand::new("touch")
        .args([
            "-t",
            "202401010000",
            session.to_str().unwrap(),
            worktree.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    bin()
        .args([
            "prune",
            "--profile",
            "real",
            "--older-than",
            "1",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("would remove session"))
        .stdout(predicate::str::contains("would remove worktree"));
}

#[test]
fn prune_retains_dirty_worktree_even_after_retention() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let worktree_root = tmp.path().join("worktrees");
    fs::create_dir_all(&worktree_root).unwrap();
    let worktree = worktree_root.join("gah-real-dirty");
    ProcessCommand::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "gah/real-dirty",
            worktree.to_str().unwrap(),
            "HEAD",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    fs::write(worktree.join("README.md"), "unpublished recovery work\n").unwrap();

    bin()
        .args([
            "prune",
            "--profile",
            "real",
            "--older-than",
            "0",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("retained dirty worktree"));

    assert!(worktree.exists());
}

#[test]
fn ledger_summary_reports_recent_counts() {
    let tmp = test_tempdir();
    let cfg = tmp.path().join("gah.toml");
    fs::write(
        &cfg,
        format!(
            r#"
[defaults]
artifact_root = "{root}/artifacts"
worktree_base = "{root}/worktrees"
llm_base_url = ""
llm_model_local = ""
llm_model_cloud = ""
"#,
            root = tmp.path().display()
        ),
    )
    .unwrap();
    let ledger_dir = tmp.path().join("artifacts");
    fs::create_dir_all(&ledger_dir).unwrap();
    fs::write(
        ledger_dir.join("ledger.jsonl"),
        "{\"timestamp\":\"2099-01-01T00:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"claude\",\"requested_backend\":\"claude\",\"effective_backend\":\"claude\",\"requested_model\":null,\"effective_model\":null,\"routing_reason\":\"explicit\",\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"mode\":\"pm\",\"target_summary\":\"x\",\"branch\":null,\"session_dir\":null,\"duration_seconds\":1.0,\"backend_exit_code\":0,\"validation_result\":\"not_run\",\"commit_attempted\":false,\"commit_created\":false,\"push_attempted\":false,\"push_succeeded\":false,\"mr_attempted\":false,\"mr_created\":false,\"mr_url\":null,\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"usage_source\":null}}\n",
    )
    .unwrap();

    bin()
        .args([
            "ledger",
            "summary",
            "--since",
            "7d",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Entries: 1"))
        .stdout(predicate::str::contains("By mode:"))
        .stdout(predicate::str::contains("pm"));
}

#[test]
fn ledger_work_filters_to_one_work_id_and_supports_json() {
    let tmp = test_tempdir();
    let cfg = tmp.path().join("gah.toml");
    fs::write(
        &cfg,
        format!(
            r#"
[defaults]
artifact_root = "{root}/artifacts"
worktree_base = "{root}/worktrees"
llm_base_url = ""
llm_model_local = ""
llm_model_cloud = ""
"#,
            root = tmp.path().display()
        ),
    )
    .unwrap();
    let ledger_dir = tmp.path().join("artifacts");
    fs::create_dir_all(&ledger_dir).unwrap();
    fs::write(
        ledger_dir.join("ledger.jsonl"),
        "{\"timestamp\":\"2026-07-04T10:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"codex\",\"requested_backend\":\"codex\",\"effective_backend\":\"codex\",\"requested_model\":null,\"effective_model\":\"gpt-5.4\",\"routing_reason\":null,\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"mode\":\"fix\",\"target_summary\":null,\"work_id\":\"TICKET-042\",\"branch\":\"gah/test-1\",\"session_dir\":null,\"duration_seconds\":42.0,\"backend_exit_code\":0,\"validation_result\":\"passed\",\"commit_attempted\":true,\"commit_created\":true,\"push_attempted\":true,\"push_succeeded\":true,\"mr_attempted\":true,\"mr_created\":true,\"mr_url\":\"https://example/pr/1\",\"files_changed\":3,\"insertions\":10,\"deletions\":2,\"error_summary\":null,\"usage\":{\"input_tokens\":100,\"output_tokens\":50,\"total_tokens\":150,\"estimated_cost_usd\":0.02,\"actual_cost_usd\":null,\"usage_source\":\"codex\"}}\n{\"timestamp\":\"2026-07-04T11:00:00Z\",\"session_id\":\"2\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"codex\",\"requested_backend\":\"codex\",\"effective_backend\":\"codex\",\"requested_model\":null,\"effective_model\":null,\"routing_reason\":null,\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"mode\":\"fix\",\"target_summary\":null,\"work_id\":\"TICKET-999-other\",\"branch\":null,\"session_dir\":null,\"duration_seconds\":null,\"backend_exit_code\":0,\"validation_result\":null,\"commit_attempted\":false,\"commit_created\":false,\"push_attempted\":false,\"push_succeeded\":false,\"mr_attempted\":false,\"mr_created\":false,\"mr_url\":null,\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"actual_cost_usd\":null,\"usage_source\":null}}\n",
    )
    .unwrap();

    bin()
        .args([
            "ledger",
            "work",
            "TICKET-042",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 entries"))
        .stdout(predicate::str::contains("codex/gpt-5.4"))
        .stdout(predicate::str::contains("$0.0200"))
        .stdout(predicate::str::contains("TICKET-999-other").not());

    let out = bin()
        .args([
            "ledger",
            "work",
            "TICKET-042",
            "--config-path",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let entries = parsed.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["work_id"], "TICKET-042");
    assert_eq!(entries[0]["usage"]["estimated_cost_usd"], 0.02);
}

#[test]
fn ledger_work_with_no_matching_entries_reports_none_found() {
    let tmp = test_tempdir();
    let cfg = tmp.path().join("gah.toml");
    fs::write(
        &cfg,
        format!(
            r#"
[defaults]
artifact_root = "{root}/artifacts"
worktree_base = "{root}/worktrees"
llm_base_url = ""
llm_model_local = ""
llm_model_cloud = ""
"#,
            root = tmp.path().display()
        ),
    )
    .unwrap();

    bin()
        .args([
            "ledger",
            "work",
            "TICKET-does-not-exist",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("No ledger entries found"));
}

#[test]
fn review_routes_to_agy_candidate_and_writes_verdict() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "agy",
        &format!(
            "#!/bin/sh\nprintf '%s\n' \"$@\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_candidates = [{ backend = \"agy\", model = \"Claude Sonnet 4.6 (Thinking)\" }, { backend = \"claude\" }]\n",
        "",
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
        .success();

    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let report = fs::read_to_string(session.join("review-report.md")).unwrap();
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(report.contains("Review notes"));
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
    assert!(verdict.contains("\"reviewer_backend\": \"agy\""));
    assert!(verdict.contains("\"reviewer_model\": \"Claude Sonnet 4.6 (Thinking)\""));
    assert!(prompt.contains("--print"));
    assert!(prompt.contains("--model"));
    assert!(prompt.contains("Claude Sonnet 4.6 (Thinking)"));
}

#[test]
fn review_falls_back_to_next_candidate_on_agy_empty_output() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_home = tmp.path().join("fake-home");
    fs::create_dir_all(fake_home.join(".gemini/antigravity-cli")).unwrap();
    let cli_log = fake_home.join(".gemini/antigravity-cli/cli.log");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    // Exit 0, empty stdout; quota error appended during the run, not pre-seeded
    // (the log delta only sees bytes written after this run starts).
    let agy_body = format!(
        "#!/bin/sh\nprintf 'E0000 00:00:00.000000 1 log.go:398] RESOURCE_EXHAUSTED (code 429): Individual quota reached. Resets in 114h2m37s.\\n' >> '{}'\nexit 0\n",
        cli_log.display(),
    );
    make_fake_bin_with_body(&fake_bin, "agy", &agy_body);
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}\nEOF\n",
    );
    make_fake_github_review_api(&fake_bin);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_candidates = [{ backend = \"agy\", model = \"Claude Sonnet 4.6 (Thinking)\" }, { backend = \"claude\" }]\n",
        "",
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
        .env("HOME", &fake_home)
        .env(
            "GAH_AVAILABILITY_PATH",
            tmp.path().join("availability.json"),
        )
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Backend unavailable; retrying review",
        ));

    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
    assert!(verdict.contains("\"reviewer_backend\": \"claude\""));
}

#[test]
fn review_falls_back_when_agy_quota_is_only_on_stderr() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "agy",
        "#!/bin/sh\nprintf 'Individual quota reached. Resets in 2h 15m.\\n' >&2\nexit 23\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}\nEOF\n",
    );
    make_fake_github_review_api(&fake_bin);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_candidates = [{ backend = \"agy\", model = \"Claude Sonnet 4.6 (Thinking)\" }, { backend = \"claude\" }]\n",
        "",
    );
    let availability_path = tmp.path().join("availability.json");

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
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Backend unavailable; retrying review with claude instead of agy/Claude Sonnet 4.6 (Thinking)",
        ));

    let availability = fs::read_to_string(availability_path).unwrap();
    assert!(availability.contains("Claude Sonnet 4.6 (Thinking)"));
    assert!(availability.contains("quota_exhausted"));
    let session = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"reviewer_backend\": \"claude\""));
}

#[test]
fn review_uses_explicit_claude_path() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    let explicit_claude = tmp.path().join("tools/claude-explicit");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(explicit_claude.parent().unwrap()).unwrap();
    make_fake_bin_with_body(
        explicit_claude.parent().unwrap(),
        "claude-explicit",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        &format!(
            "claude_path = \"{}\"\n[profiles.real.routing]\nreview_backend = \"claude\"\n",
            explicit_claude.display()
        ),
        "",
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
        .success();

    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("Source: feature/review"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

#[test]
fn review_fails_when_required_capability_not_installed() {
    let tmp = test_tempdir();
    let (repo, fake_bin, home) = setup_review_repo_and_gh(&tmp);
    fs::create_dir_all(&home).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\necho should never run\nexit 1\n",
    );
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\nreview_required_capabilities = { claude = [\"ponytail\"] }\n",
        "",
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
        .env("HOME", &home)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("ponytail").and(predicate::str::contains("not installed")),
        );
}

#[test]
fn review_activates_and_records_capability_when_installed() {
    let tmp = test_tempdir();
    let (repo, fake_bin, home) = setup_review_repo_and_gh(&tmp);
    fs::create_dir_all(home.join(".claude/plugins/cache/ponytail")).unwrap();
    let prompt_log = tmp.path().join("review-prompt.txt");
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\nreview_required_capabilities = { claude = [\"ponytail\"] }\n",
        "",
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
        .env("HOME", &home)
        .assert()
        .success();

    let prompt = fs::read_to_string(&prompt_log).unwrap();
    assert!(prompt.starts_with("/ponytail full"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict_text = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    let verdict: Value = serde_json::from_str(&verdict_text).unwrap();
    assert_eq!(verdict["verdict"], serde_json::json!("APPROVE"));
    assert_eq!(
        verdict["applied_capabilities"],
        serde_json::json!(["ponytail"])
    );
}

#[test]
fn review_fails_as_degraded_when_capability_has_no_known_activation() {
    let tmp = test_tempdir();
    let (repo, fake_bin, home) = setup_review_repo_and_gh(&tmp);
    fs::create_dir_all(home.join(".claude/plugins/cache/some-future-skill")).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\necho should never run\nexit 1\n",
    );
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\nreview_required_capabilities = { claude = [\"some-future-skill\"] }\n",
        "",
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
        .env("HOME", &home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("reviewer degraded"));
}

#[test]
fn review_parse_failure_preserves_raw_report_and_records_bounded_reroute() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n\n[profiles.real.publishing]\nallow_issue_comments = false\n",
        "",
    );
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false\nEOF\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"number\":7}]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"number\":7,\"url\":\"https://github.com/owner/real/pull/7\",\"title\":\"Draft: [GAH] Fix\",\"body\":\"MR body\",\"headRefName\":\"feature/review\",\"baseRefName\":\"main\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"SUCCESS\"}]}'; exit 0; fi\nexit 0\n",
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
        .stdout(predicate::str::contains("bounded reviewer reroute"));
    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let report = fs::read_to_string(session.join("review-report.md")).unwrap();
    assert!(report.contains("Review notes"));
    let verdict: serde_json::Value =
        serde_json::from_slice(&fs::read(session.join("review-verdict.json")).unwrap()).unwrap();
    assert_eq!(verdict["verdict"], "REVIEW_OUTPUT_INVALID");
}

#[test]
fn review_shutdown_records_cancelled_shutdown_and_dispatch_finished_event() {
    let tmp = test_tempdir();
    let (repo, fake_bin, home) = setup_review_repo_and_gh(&tmp);
    fs::create_dir_all(&home).unwrap();
    let claude = FakeBackend::new(tmp.path(), "claude");
    claude.install(Scenario::success().with_delay_ms(30_000));
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    // Keep the isolated environment alive until the spawned process exits.
    let mut command = spawn_bin();
    let mut child = command
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
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    wait_for_backend_call(&claude, &mut child, 1);
    #[cfg(unix)]
    send_signal(child.id(), libc::SIGINT);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(claude.call_count(), 1);

    let ledger_text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(ledger_text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "harness_error");
    assert_eq!(entry["failure_stage"], "review");
    assert_eq!(entry["validation_result"], "cancelled_shutdown");

    let events_text = fs::read_to_string(&events_path).unwrap();
    assert!(events_text.contains("dispatch_started"));
    assert!(events_text.contains("dispatch_finished"));
    assert!(events_text.contains("shutdown requested while claude was running"));
}

#[test]
fn review_gitlab_uses_host_scoped_glab_session_without_pat() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "gitlab",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    let glab_log = tmp.path().join("glab.log");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}\nEOF\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\ncase \"$1 $2\" in\n  \"api projects/42/merge_requests\")\n    printf '%s\\n' '[{{\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"iid\":7,\"source_branch\":\"feature/review\",\"target_branch\":\"main\"}}]'\n    ;;\n  \"api projects/42/merge_requests/7/notes\") printf '%s\\n' '{{\"id\":1}}' ;;\n  \"api projects/42/merge_requests/7\") printf '%s\\n' '{{\"iid\":7}}' ;;\n  *) echo \"unexpected glab invocation: $*\" >&2; exit 1 ;;\n esac\n",
            glab_log.display()
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
        .env_remove("GITLAB_PAT")
        .env_remove("GITLAB_PAT2")
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Resolved MR: https://gitlab.example.com/owner/real/-/merge_requests/7",
        ));

    let glab_log = fs::read_to_string(glab_log).unwrap();
    assert!(glab_log.contains("--hostname gitlab.example.com"));
    assert!(!glab_log.contains("PRIVATE-TOKEN"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

#[test]
fn review_by_mr_uses_provider_metadata_even_when_repo_is_on_main() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt-mr.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "gitlab",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        "#!/bin/sh\ncase \"$1 $2\" in\n  \"api projects/42/merge_requests/7\") printf '%s\\n' '{\"iid\":7,\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"source_branch\":\"feature/review\",\"target_branch\":\"main\",\"title\":\"Draft: [GAH] Fix\",\"description\":\"MR body\",\"detailed_merge_status\":\"mergeable\"}' ;;\n  \"api projects/42/merge_requests\") printf '%s\\n' '[{\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"iid\":7,\"source_branch\":\"feature/review\",\"target_branch\":\"main\",\"title\":\"Draft: [GAH] Fix\",\"description\":\"MR body\",\"detailed_merge_status\":\"mergeable\"}]' ;;\n  \"api projects/42/merge_requests/7/notes\") printf '%s\\n' '{\"id\":1}' ;;\n  *) echo \"unexpected glab invocation: $*\" >&2; exit 1 ;;\n esac\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--mr",
            "7",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env_remove("GITLAB_PAT")
        .assert()
        .success();

    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("MR: 7"));
    assert!(prompt.contains("Source: feature/review"));
    assert!(prompt.contains("Target: main"));
    assert!(prompt.contains("MR title: Draft: [GAH] Fix"));
    assert!(prompt.contains("MR body:\n  MR body"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

#[test]
fn review_uses_profile_repo_not_current_worktree() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let worktree = tmp.path().join("review-wt");
    let prompt_log = tmp.path().join("review-prompt-worktree.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    ProcessCommand::new("git")
        .args([
            "worktree",
            "add",
            worktree.to_str().unwrap(),
            "feature/review",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);

    bin()
        .current_dir(&worktree)
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

    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("Source: feature/review"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

#[test]
fn review_empty_diff_fails_loudly() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let bare = repo.parent().unwrap().join("origin.git");
    ProcessCommand::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["remote", "add", "origin", bare.to_str().unwrap()])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "claude");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "main",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .failure()
        .stderr(predicate::str::contains("empty review diff"))
        .stderr(predicate::str::contains("profile.local_path"))
        .stderr(predicate::str::contains("source branch: main"))
        .stderr(predicate::str::contains("target branch: main"));
}

#[test]
fn fix_mode_uses_ticket_title_in_mr_title() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let github_root = tmp.path().join("github-root");
    let origin = github_root.join("owner/real.git");
    let ticket = repo.join("docs/tickets/TICKET-058-descriptive-mr-titles.md");
    let gh_log = tmp.path().join("gh.log");
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
    fs::create_dir_all(ticket.parent().unwrap()).unwrap();
    fs::write(
        &ticket,
        "# TICKET-058: Descriptive Title Here\n\nGoal: Generate a descriptive MR body\nDifficulty: easy\nRisk: low\n\n## Problem\n\nThe old MR body is too sparse.\n",
    )
    .unwrap();

    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nimprove_backend = \"codex\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'ticket context update\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf '%s\\n' 'https://github.com/owner/real/pull/7'; exit 0; fi\nexit 0\n",
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
            ticket.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", home)
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success();

    let gh_log = fs::read_to_string(gh_log).unwrap();
    assert!(gh_log.contains("--title Draft: [GAH] Fix: TICKET-058 Descriptive Title Here"));
    assert!(gh_log.contains("## Work Item"));
    assert!(gh_log.contains("ID: `TICKET-058`"));
    assert!(gh_log.contains("## Problem"));
    assert!(gh_log.contains("The old MR body is too sparse."));
    assert!(gh_log.contains("## Goal"));
    assert!(gh_log.contains("Generate a descriptive MR body"));
    assert!(gh_log.contains("## Validation"));
    assert!(gh_log.contains("## Backend / Model"));
}

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
