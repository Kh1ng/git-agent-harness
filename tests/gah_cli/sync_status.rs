use super::*;

#[test]
fn sync_classifies_open_gah_prs() {
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
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-1\",\"url\":\"https://example/pr/1\",\"labels\":[{\"name\":\"gah-ready-for-human\"}],\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[]}]'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("READY_FOR_HUMAN"))
        .stdout(predicate::str::contains(
            "recommended: human review and merge decision",
        ));
}

#[test]
fn sync_classifies_closed_unmerged_github_prs() {
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
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-closed\",\"url\":\"https://example/pr/closed\",\"labels\":[{\"name\":\"gah-ready-for-human\"}],\"state\":\"CLOSED\",\"isDraft\":true,\"mergeStateStatus\":\"DIRTY\",\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"FAILURE\"}]}]'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("CLOSED_UNMERGED"))
        .stdout(predicate::str::contains("recommended: none"))
        .stdout(predicate::str::contains("gah/test-closed"));
}

/// Build a fake `glab` that responds to `api projects/42/merge_requests`
/// with the given JSON body and exits 0. Anything else exits 0 with no
/// output.
fn make_fake_glab(dir: &std::path::Path, mr_list_json: &str) {
    make_fake_bin_with_body(
        dir,
        "glab",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/merge_requests\" ]; then echo '{}'; exit 0; fi\nexit 0\n",
            mr_list_json.replace('\'', "'\\''"),
        ),
    );
}

#[test]
fn sync_gitlab_classifies_open_mr() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_glab(
        &fake_bin,
        r#"[{"title":"[GAH] fix","source_branch":"gah/test-1","web_url":"https://example/mr/1","labels":["gah-ready-for-human"],"state":"opened","merged_at":null,"updated_at":"2099-01-01T00:00:00Z","head_pipeline":{"status":"success"}}]"#,
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("READY_FOR_HUMAN"))
        .stdout(predicate::str::contains("gah/test-1"));
}

#[test]
fn sync_gitlab_classifies_merged_mr() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_glab(
        &fake_bin,
        r#"[{"title":"[GAH] fix","source_branch":"gah/test-2","web_url":"https://example/mr/2","labels":[],"state":"merged","merged_at":"2099-01-01T00:00:00Z","updated_at":"2099-01-01T00:00:00Z"}]"#,
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("MERGED"))
        .stdout(predicate::str::contains("gah/test-2"))
        .stdout(predicate::str::contains("recommended: none"));
}

#[test]
fn sync_gitlab_closed_unmerged_mr_is_terminal() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_glab(
        &fake_bin,
        r#"[{"title":"[GAH] fix","source_branch":"gah/test-3","web_url":"https://example/mr/3","labels":[],"state":"closed","merged_at":null,"updated_at":"2099-01-01T00:00:00Z"}]"#,
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("CLOSED_UNMERGED"))
        .stdout(predicate::str::contains("recommended: none"))
        .stdout(predicate::str::contains("gah/test-3"));
}

#[test]
fn status_json_excludes_closed_unmerged_history() {
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
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-status\",\"url\":\"https://example/pr/status\",\"labels\":[{\"name\":\"gah-human-review\"}],\"state\":\"closed\",\"isDraft\":true,\"mergeStateStatus\":\"DIRTY\",\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"FAILURE\"}]}]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "status",
            "--profile",
            "real",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("status stdout must be valid JSON");
    let mrs = parsed["merge_requests"]
        .as_array()
        .expect("merge_requests must be an array");
    assert!(mrs.is_empty());
}

#[test]
fn sync_gitlab_malformed_json_fails_loudly() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_glab(&fake_bin, "not json at all");

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .failure();
}

#[test]
fn sync_gitlab_no_matching_mr() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_glab(&fake_bin, "[]");

    let out = bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(!stdout.contains("gah/test"));
}

#[test]
fn sync_gitlab_fails_when_glab_missing() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", "")
        .assert()
        .failure()
        .stderr(predicate::str::contains("glab api GitLab request"));
}

#[test]
fn sync_gitlab_fails_when_glab_fails() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        "#!/bin/sh\necho \"API ERROR\" >&2\nexit 1\n",
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .failure()
        .stderr(predicate::str::contains("API ERROR"));
}

// ── TDD: machine-readable state for autonomous manager agents ──────────────
// These define the contract for junior-agent tickets. Remove #[ignore] when
// implementing.

#[test]
fn sync_json_outputs_machine_readable_classification() {
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
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-1\",\"url\":\"https://example/pr/1\",\"labels\":[{\"name\":\"gah-ready-for-human\"}],\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[]}]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let mrs = parsed.as_array().expect("top level must be an array");
    assert_eq!(mrs[0]["classification"], "READY_FOR_HUMAN");
    assert_eq!(mrs[0]["branch"], "gah/test-1");
    assert!(mrs[0]["recommended_action"].is_string());
    assert_eq!(mrs[0]["url"], "https://example/pr/1");
}

/// TICKET-070: the JSON view must expose the richer fields the ticket
/// requires ("at minimum": MR identifier, state, draft, merge status), not
/// just the classification/recommendation floor the contract test checks.
/// Also verifies --json prints ONLY JSON — no "Profile: ..." human header
/// mixed into stdout, since that would break every consumer that expects
/// pure JSON on stdout.
#[test]
fn sync_json_includes_id_state_draft_and_merge_status() {
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
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-1\",\"url\":\"https://example/pr/1\",\"labels\":[],\"number\":42,\"state\":\"OPEN\",\"isDraft\":true,\"mergeStateStatus\":\"BEHIND\",\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[]}]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(
        !stdout.contains("Profile:"),
        "--json must print only JSON, not the human header: {stdout}"
    );
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    let mrs = parsed.as_array().unwrap();
    assert_eq!(mrs[0]["id"], "42");
    assert_eq!(mrs[0]["state"], "OPEN");
    assert_eq!(mrs[0]["draft"], true);
    assert_eq!(mrs[0]["merge_status"], "BEHIND");
    assert_eq!(mrs[0]["profile"], "real");
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
fn status_reports_human_and_json_views() {
    let tmp = write_fixture_dir();
    let cfg = write_dispatch_config(&tmp);
    let root = tmp.path().join("real");
    init_git_repo(&root);
    write_real_repo_config(&tmp, &root, "test-repo");

    let availability_path = tmp.path().join("avail.json");
    let ledger_path = tmp.path().join("ledger.jsonl");

    // Write a mock availability record
    let avail_state = serde_json::json!({
        "version": 1,
        "records": [
            {
                "backend": "claude",
                "model": "claude-3-5",
                "status": "unavailable",
                "reason": "rate_limited",
                "observed_at": "2026-07-04T12:00:00Z",
                "unavailable_until": "2099-01-01T00:00:00Z",
                "source": "backend_error"
            }
        ]
    });
    fs::write(
        &availability_path,
        serde_json::to_string(&avail_state).unwrap(),
    )
    .unwrap();

    // Write a mock ledger entry
    let ledger_entry: Value = serde_json::from_str(
        r#"{
            "timestamp": "2026-07-04T13:00:00Z",
            "profile": "test-repo",
            "display_name": "Test Repo",
            "repo_id": "test-repo",
            "repo": "owner/test-repo",
            "local_path": "/tmp",
            "provider": "github",
            "backend": "claude",
            "requested_backend": "claude",
            "effective_backend": "claude",
            "requested_model": null,
            "effective_model": "claude-3-5",
            "routing_reason": "explicit",
            "fallback_used": false,
            "confidence_impact": null,
            "human_required": false,
            "mode": "improve",
            "target_summary": null,
            "branch": "gah/test-branch",
            "session_dir": null,
            "duration_seconds": null,
            "backend_exit_code": null,
            "validation_result": null,
            "commit_attempted": false,
            "commit_created": false,
            "push_attempted": false,
            "push_succeeded": false,
            "mr_attempted": false,
            "mr_created": false,
            "mr_url": null,
            "files_changed": null,
            "insertions": null,
            "deletions": null,
            "error_summary": null,
            "failure_class": "backend_error",
            "failure_stage": "agent_run",
            "attempts_started": 3,
            "attempts_completed": 2,
            "routing_diagnostics": {
                "policy_reordered_candidates": true,
                "selected_backend": "claude",
                "selected_model": "claude-3-5",
                "selected_quota_pool": "claude-main",
                "selected_pace_band": "normal",
                "selected_cost_class": "included_quota",
                "selected_over": ["codex/gpt-5.4 (paid $0.2500)"],
                "candidates": [
                    {
                        "backend": "claude",
                        "model": "claude-3-5",
                        "quota_pool": "claude-main",
                        "default_order": 1,
                        "consideration_order": 0,
                        "pace_band": "normal",
                        "cost_class": "included_quota",
                        "skip_reason": null,
                        "unavailable_until": null
                    }
                ],
                "human_summary": "selected claude/claude-3-5"
            },
            "usage": {}
        }"#,
    )
    .unwrap();
    fs::write(
        &ledger_path,
        serde_json::to_string(&ledger_entry).unwrap() + "\n",
    )
    .unwrap();

    let out = bin()
        .current_dir(&root)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .args([
            "status",
            "--json",
            "--profile",
            "test-repo",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["profile"]["display_name"], "Test Repo");
    assert_eq!(parsed["profile"]["provider"], "github");
    assert!(parsed["observations"]["sync"]["status"].is_string());
    assert!(parsed["observations"]["availability"]["status"].is_string());
    assert!(parsed["observations"]["ledger"]["status"].is_string());

    // Verify availability fields, specifically observed_at is populated
    let avail = &parsed["availability"][0];
    assert_eq!(avail["backend"], "claude");
    assert_eq!(avail["model"], "claude-3-5");
    assert_eq!(avail["observed_at"], "2026-07-04T12:00:00Z");

    // Verify ledger fields
    let ledger = &parsed["recent_ledger"];
    assert_eq!(ledger["most_recent_failure_class"], "backend_error");
    assert_eq!(ledger["most_recent_failure_stage"], "agent_run");
    assert_eq!(ledger["attempts_started"], 3);
    assert_eq!(ledger["attempts_completed"], 2);
    assert_eq!(
        ledger["routing_diagnostics"]["selected_quota_pool"],
        "claude-main"
    );

    bin()
        .current_dir(&root)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .args([
            "status",
            "--profile",
            "test-repo",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status for Profile: test-repo"))
        .stdout(predicate::str::contains("Observations: Sync="))
        .stdout(predicate::str::contains("Recent Routing:"))
        .stdout(predicate::str::contains("selected claude/claude-3-5"));
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
