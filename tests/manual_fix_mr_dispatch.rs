mod support;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use support::fake_ledger::TestLedger;
use support::scenario::ScenarioHarness;

fn manual_fix_review_ledger_entry(
    branch: &str,
    work_id: &str,
    source_issue: Option<&str>,
    timestamp: &str,
) -> serde_json::Value {
    let mut entry = support::fake_ledger::ledger_entry_full(
        "review",
        branch,
        Some("review"),
        work_id,
        timestamp,
    );
    if let Some(obj) = entry.as_object_mut() {
        obj.insert(
            "review_verdict".into(),
            serde_json::json!("NEEDS_FIX".to_string()),
        );
        obj.insert(
            "review_source_sha".into(),
            serde_json::json!("HEAD".to_string()),
        );
        obj.insert(
            "review_blocking_findings".into(),
            serde_json::json!(vec!["stability regression".to_string()]),
        );
        obj.insert(
            "source_issue_number".into(),
            source_issue
                .map(|issue| serde_json::Value::String(issue.to_string()))
                .unwrap_or(serde_json::Value::Null),
        );
    }
    entry
}

fn install_change_making_worker(harness: &mut ScenarioHarness, backend: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let fake = support::FakeBackend::new(temp.path(), backend);
    let script = "#!/bin/sh\n\
set -eu\n\
printf 'work complete\\n'\n\
printf 'manual fix test\\n' > .manual_fix_dispatch_change.txt\n\
exit 0\n"
        .to_string();
    let script_path = fake.bin_dir().join(backend);
    std::fs::write(&script_path, script).unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }
    harness.install_custom_worker(backend, &fake);
    temp
}

#[test]
fn github_fix_dispatch_resolves_manual_mr_source_branch_and_work_identity() {
    let branch = "gah/fix-needs-fix";
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("manual_fix_needs_fix")
        .worker_scenario("success")
        .with_config_append(
            "[profiles.test.publishing]\nallow_pull_request_creation = false\nallow_commit_message_generation = false\n",
        )
        .with_ledger(
            TestLedger::new().with_entry(manual_fix_review_ledger_entry(
                branch,
                "#269",
                None,
                "2026-07-01T00:00:00Z",
            )),
        );
    harness.create_remote_branch(branch);
    let _openhands_fake = install_change_making_worker(&mut harness, "openhands");
    let _vibe_fake = install_change_making_worker(&mut harness, "vibe");
    let _opencode_fake = install_change_making_worker(&mut harness, "opencode");
    let _claude_fake = install_change_making_worker(&mut harness, "claude");
    let _codex_fake = install_change_making_worker(&mut harness, "codex");
    let _agy_fake = install_change_making_worker(&mut harness, "agy");

    let result = harness
        .run_dispatch(&["--mode", "fix", "--mr", "269"])
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "stderr was {}", result.stderr);

    assert!(
        result
            .stdout
            .contains("Creating worktree from existing branch 'gah/fix-needs-fix'"),
        "{}",
        result.stdout
    );
    assert!(result
        .stdout
        .contains("Resolved MR 269 to branch gah/fix-needs-fix"));
    assert!(!result.stdout.contains("Creating worktree from main"));

    let ledger = TestLedger::read_from(&harness.ledger_path).unwrap();
    let entry = ledger.last().unwrap();
    assert_eq!(entry["branch"], serde_json::json!(branch));
    assert_eq!(entry["work_id"], serde_json::json!("TICKET-269"));
    assert_eq!(entry["source_issue_number"], serde_json::json!("269"));
}

#[test]
fn gitlab_fix_dispatch_resolves_manual_mr_source_branch_and_work_identity() {
    let branch = "gah/fix-needs-fix";
    let mut harness = ScenarioHarness::new("gitlab")
        .gitlab_scenario("manual_fix_needs_fix")
        .worker_scenario("success")
        .with_config_append(
            "provider_api_base = \"https://gitlab.example.com\"\nprovider_project_id = \"42\"\n\n[profiles.test.publishing]\nallow_pull_request_creation = false\nallow_commit_message_generation = false\n",
        )
        .with_ledger(
            TestLedger::new().with_entry(manual_fix_review_ledger_entry(
                branch,
                "TICKET-269",
                Some("269"),
                "2026-07-01T00:00:00Z",
            )),
        );
    harness.create_remote_branch(branch);
    let _openhands_fake = install_change_making_worker(&mut harness, "openhands");
    let _vibe_fake = install_change_making_worker(&mut harness, "vibe");
    let _opencode_fake = install_change_making_worker(&mut harness, "opencode");
    let _claude_fake = install_change_making_worker(&mut harness, "claude");
    let _codex_fake = install_change_making_worker(&mut harness, "codex");
    let _agy_fake = install_change_making_worker(&mut harness, "agy");

    let result = harness
        .run_dispatch(&["--mode", "fix", "--mr", "269"])
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "stderr was {}", result.stderr);

    assert!(
        result
            .stdout
            .contains("Creating worktree from existing branch 'gah/fix-needs-fix'"),
        "{}",
        result.stdout
    );
    assert!(result
        .stdout
        .contains("Resolved MR 269 to branch gah/fix-needs-fix"));
    assert!(!result.stdout.contains("Creating worktree from main"));

    let ledger = TestLedger::read_from(&harness.ledger_path).unwrap();
    let entry = ledger.last().unwrap();
    assert_eq!(entry["branch"], serde_json::json!(branch));
    assert_eq!(entry["work_id"], serde_json::json!("TICKET-269"));
    assert_eq!(entry["source_issue_number"], serde_json::json!("269"));
}

#[test]
fn manual_fix_dispatch_rejects_missing_work_identity() {
    let mut harness = ScenarioHarness::new("github").github_scenario("manual_fix_needs_fix");

    let result = harness
        .run_dispatch(&["--mode", "fix", "--mr", "269"])
        .unwrap();
    assert_ne!(result.exit_code, Some(0));
    assert!(
        result
            .stderr
            .contains("could not resolve a work identity for branch"),
        "{}",
        result.stderr
    );
    assert!(!result.stdout.contains("Creating worktree from main"));
}

#[test]
fn manual_fix_dispatch_rejects_ambiguous_work_identity() {
    let branch = "gah/fix-needs-fix";
    let mut harness = ScenarioHarness::new("github").github_scenario("manual_fix_needs_fix");
    harness = harness.with_ledger(
        TestLedger::new()
            .with_entry(manual_fix_review_ledger_entry(
                branch,
                "#269",
                None,
                "2026-07-01T00:00:00Z",
            ))
            .with_entry(manual_fix_review_ledger_entry(
                branch,
                "TICKET-270",
                Some("270"),
                "2026-07-02T00:00:00Z",
            )),
    );

    let result = harness
        .run_dispatch(&["--mode", "fix", "--mr", "269"])
        .unwrap();
    assert_ne!(result.exit_code, Some(0));
    assert!(
        result
            .stderr
            .contains("MR source branch 'gah/fix-needs-fix' has multiple work identities"),
        "{}",
        result.stderr
    );
    assert!(!result
        .stdout
        .contains("Creating worktree from existing branch 'gah/fix-needs-fix'"));
    assert!(!result.stdout.contains("Creating worktree from main"));
}

#[test]
fn github_manual_fix_dispatch_rejects_merged_mr() {
    let mut harness = ScenarioHarness::new("github").github_scenario("manual_fix_needs_fix_merged");
    harness = harness.with_ledger(TestLedger::new().with_entry(manual_fix_review_ledger_entry(
        "gah/fix-needs-fix",
        "#269",
        None,
        "2026-07-01T00:00:00Z",
    )));

    let result = harness
        .run_dispatch(&["--mode", "fix", "--mr", "269"])
        .unwrap();
    assert_ne!(result.exit_code, Some(0));
    assert!(
        result
            .stderr
            .contains("MR 269 is merged and cannot be reused for fix repair"),
        "{}",
        result.stderr
    );
    assert!(!result
        .stdout
        .contains("Creating worktree from existing branch 'gah/fix-needs-fix'"));
    assert!(!result.stdout.contains("Creating worktree from main"));
}

#[test]
fn gitlab_manual_fix_dispatch_rejects_merged_mr() {
    let mut harness = ScenarioHarness::new("gitlab").gitlab_scenario("manual_fix_needs_fix_merged");
    harness = harness.with_config_append(
        "provider_api_base = \"https://gitlab.example.com\"\nprovider_project_id = \"42\"\n\n[profiles.test.publishing]\nallow_pull_request_creation = false\nallow_commit_message_generation = false\n",
    );
    harness = harness.with_ledger(TestLedger::new().with_entry(manual_fix_review_ledger_entry(
        "gah/fix-needs-fix",
        "#269",
        None,
        "2026-07-01T00:00:00Z",
    )));

    let result = harness
        .run_dispatch(&["--mode", "fix", "--mr", "269"])
        .unwrap();
    assert_ne!(result.exit_code, Some(0));
    assert!(
        result
            .stderr
            .contains("MR 269 is merged and cannot be reused for fix repair"),
        "{}",
        result.stderr
    );
    assert!(!result
        .stdout
        .contains("Creating worktree from existing branch 'gah/fix-needs-fix'"));
    assert!(!result.stdout.contains("Creating worktree from main"));
}
