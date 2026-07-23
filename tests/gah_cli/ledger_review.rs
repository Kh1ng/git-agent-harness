use super::*;

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

/// Data source for the frontend attempt-timeline view (Work detail page).
/// Thin wrapper around `ledger::entries_for_work_id` -- this test proves
/// the CLI wiring (flag parsing, filtering by work_id, JSON shape), not the
/// filtering logic itself, which already has its own unit tests in
/// src/ledger/mod.rs.
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

/// Regression: review mode used to fail the whole dispatch outright on an
/// empty-output AGY failure (quota exhaustion, exit=0) even when
/// review_candidates listed a real fallback -- the candidate list was
/// consulted once for the initial route, then never touched again. Fakes
/// AGY returning empty stdout with a RESOURCE_EXHAUSTED cli.log (the exact
/// live failure signature) and confirms review actually falls through to
/// the next candidate instead of erroring.
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

/// AGY's subscription CLI may emit quota exhaustion only on stderr with a
/// nonzero exit.  That failure must make the exact review route unavailable
/// before selecting the fallback; otherwise every loop cycle repeats AGY.
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

fn setup_review_repo_and_gh(
    tmp: &TempDir,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_github_review_api(&fake_bin);
    (repo, fake_bin, tmp.path().join("home"))
}

/// TICKET-109/105: reviewing with a required-but-uninstalled capability must
/// stop the review outright, not silently degrade to an ordinary one.
/// Uses an isolated HOME with no `.claude/plugins/cache/` at all, so this
/// doesn't depend on whether the real dev machine happens to have Ponytail
/// installed.
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

/// TICKET-109: when the required capability IS installed (fake plugin-cache
/// directory under an isolated HOME), the review prompt must contain the
/// activation text, and the verdict must record it in applied_capabilities.
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

/// TICKET-105: a capability that IS installed (plugin dir present) but that
/// GAH has no known activation mapping for must refuse with "reviewer
/// degraded", not silently run an ordinary review.
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

/// A review backend failure must propagate through the controller-facing
/// dispatch path.  Otherwise `gah loop --once` records `review: success`
/// even though the ledger correctly says backend_error, which can conceal a
/// failed review from the operator and the next controller observation.
#[test]
fn loop_reports_nonzero_review_backend_as_failure_not_success() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    ProcessCommand::new("git")
        .args(["branch", "gah/real-review", "feature/review"])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "origin", "gah/real-review"])
        .current_dir(&repo)
        .output()
        .unwrap();
    checkout_branch(&repo, "main");
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
        "#!/bin/sh\nprintf 'subscription quota exhausted\\n' >&2\nexit 23\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\ncase \"$4\" in */pulls?*) echo '[{\"title\":\"[GAH] Fix: TICKET-500\",\"body\":\"MR body\",\"head\":{\"ref\":\"gah/real-review\",\"sha\":\"source-sha\"},\"html_url\":\"https://github.com/owner/real/pull/7\",\"labels\":[],\"number\":7,\"state\":\"open\",\"draft\":true,\"updated_at\":\"2026-07-18T17:22:35-05:00\"}]'; exit 0;; */check-runs?*) echo '{\"total_count\":1,\"check_runs\":[{\"status\":\"completed\",\"conclusion\":\"success\"}]}'; exit 0;; esac\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"number\":7,\"url\":\"https://github.com/owner/real/pull/7\",\"title\":\"[GAH] Fix: TICKET-500\",\"body\":\"MR body\",\"headRefName\":\"gah/real-review\",\"baseRefName\":\"main\",\"headRefOid\":\"source-sha\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"SUCCESS\"}]}'; exit 0; fi\nexit 0\n",
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
        .failure()
        .stderr(predicate::str::contains(
            "review backend exited with status 23",
        ));

    let events_text = fs::read_to_string(events_path).unwrap();
    assert!(events_text.contains("dispatch_started"));
    assert!(events_text.contains("review backend exited with status 23"));
    assert!(!events_text.contains("review: success"));
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
