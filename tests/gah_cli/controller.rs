use super::*;

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
