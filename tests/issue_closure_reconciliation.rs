mod support;

use support::scenario::ScenarioHarness;
use support::{FakeBackend, Scenario};
use tempfile::TempDir;

fn merged_github_pr_json(body: &str) -> String {
    format!(
        r#"[{{"title":"[GAH] Fix: TICKET-72","body":"{body}","headRefName":"gah/improve-example-123456","url":"https://github.com/owner/repo/pull/72","labels":[],"number":72,"state":"MERGED","isDraft":false,"mergeStateStatus":"MERGED","mergedAt":"2026-01-02T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z","statusCheckRollup":[{{"conclusion":"SUCCESS"}}]}}]"#
    )
}

fn ledger_entry(source_issue_number: Option<&str>) -> serde_json::Value {
    let mut entry = support::fake_ledger::ledger_entry_full(
        "improve",
        "gah/improve-example-123456",
        None,
        "TICKET-72",
        "2026-01-01T00:00:00Z",
    );
    if let Some(obj) = entry.as_object_mut() {
        obj.insert(
            "source_issue_number".into(),
            source_issue_number
                .map(|issue| serde_json::Value::String(issue.to_string()))
                .unwrap_or(serde_json::Value::Null),
        );
    }
    entry
}

fn closure_enabled_toml() -> &'static str {
    "[profiles.test.publishing]\nallow_source_issue_closure = true\n"
}

#[test]
fn merged_pr_explicit_reference_closes_once_then_becomes_already_closed() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("open\n"),
        Scenario::success(),
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(None)));
    harness.install_custom_gh(&gh);

    let first = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(first["issue_closure"]["closed"], serde_json::json!(["42"]));
    assert_eq!(gh.call_count(), 3);
    assert_eq!(gh.argv_for_call(1)[..2], ["pr", "list"]);
    assert_eq!(gh.argv_for_call(2)[0], "api");
    assert_eq!(gh.argv_for_call(3)[..3], ["issue", "close", "42"]);

    let second = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        second["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(gh.call_count(), 5);
    assert_eq!(gh.argv_for_call(5)[0], "api");

    let third = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        third["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(gh.call_count(), 7);
    assert_eq!(gh.argv_for_call(7)[0], "api");
}

#[test]
fn merged_pr_structured_source_identity_closes_without_body_reference() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("No closing keyword here")),
        Scenario::success().with_stdout("open\n"),
        Scenario::success(),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(Some("55"))));
    harness.install_custom_gh(&gh);

    let report = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(report["issue_closure"]["closed"], serde_json::json!(["55"]));
    assert_eq!(gh.call_count(), 3);
    assert_eq!(gh.argv_for_call(2)[0], "api");
    assert_eq!(gh.argv_for_call(2)[1], "repos/owner/repo/issues/55");
    assert_eq!(gh.argv_for_call(3)[..3], ["issue", "close", "55"]);
}

#[test]
fn conflicting_explicit_and_structured_mappings_close_nothing() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42"))
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(Some("55"))));
    harness.install_custom_gh(&gh);

    let report = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        report["issue_closure"]["ambiguous"],
        serde_json::json!(["unknown"])
    );
    assert_eq!(gh.call_count(), 1);
    assert_eq!(gh.argv_for_call(1)[..2], ["pr", "list"]);
}

#[test]
fn dry_run_reports_would_close_and_performs_no_close_write() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("open\n"),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(None)));
    harness.install_custom_gh(&gh);

    let report = harness.run_ledger_reconcile_json(true).unwrap();
    assert_eq!(
        report["issue_closure"]["would_close"],
        serde_json::json!(["42"])
    );
    assert_eq!(gh.call_count(), 2);
    assert_eq!(gh.argv_for_call(1)[..2], ["pr", "list"]);
    assert_eq!(gh.argv_for_call(2)[0], "api");
}

#[test]
fn gitlab_dry_run_observes_open_issue_without_writing() {
    let tmp = TempDir::new().unwrap();
    let glab = FakeBackend::new(tmp.path(), "glab");
    glab.install_sequence(vec![
        Scenario::success().with_stdout(
            r#"[{"title":"[GAH] Fix: TICKET-72","description":"Closes #42","source_branch":"gah/improve-example-123456","web_url":"https://gitlab.example.com/group/repo/-/merge_requests/72","labels":[],"iid":72,"state":"merged","draft":false,"merged_at":"2026-01-02T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","head_pipeline":{"status":"success"}}]"#,
        ),
        Scenario::success().with_stdout(r#"{"state":"opened"}"#),
    ]);

    let mut harness = ScenarioHarness::new("gitlab")
        .with_config_append(
            "provider_api_base = \"https://gitlab.example.com/api/v4\"\nprovider_project_id = \"123\"\n[profiles.test.publishing]\nallow_source_issue_closure = true\n",
        )
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(None)));
    harness.install_custom_glab(&glab);

    let report = harness.run_ledger_reconcile_json(true).unwrap();
    assert_eq!(
        report["issue_closure"]["would_close"],
        serde_json::json!(["42"])
    );
    assert_eq!(glab.call_count(), 2);
    assert_eq!(
        &glab.argv_for_call(1)[..2],
        ["api", "projects/123/merge_requests"]
    );
    assert!(glab.argv_for_call(1).contains(&"--hostname".to_string()));
    assert_eq!(
        glab.argv_for_call(2)[..2],
        ["api", "projects/123/issues/42"]
    );
    assert!(glab.argv_for_call(2).contains(&"--hostname".to_string()));
}

#[test]
fn multiple_explicit_references_with_matching_structured_source_closes_target() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42, fixes #43")),
        Scenario::success().with_stdout("open\n"),
        Scenario::success(),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(Some("42"))));
    harness.install_custom_gh(&gh);

    let report = harness.run_ledger_reconcile_json(false).unwrap();
    // Should close issue #42 because it matches the structured source, despite multiple explicit references
    assert_eq!(report["issue_closure"]["closed"], serde_json::json!(["42"]));
    assert_eq!(gh.call_count(), 3);
    assert_eq!(gh.argv_for_call(3)[..3], ["issue", "close", "42"]);
}

#[test]
fn multiple_explicit_references_without_matching_structured_source_ambiguous() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42, fixes #43"))
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(Some("99"))));
    harness.install_custom_gh(&gh);

    let report = harness.run_ledger_reconcile_json(false).unwrap();
    // Should be ambiguous because multiple explicit references don't match structured source
    assert_eq!(
        report["issue_closure"]["ambiguous"],
        serde_json::json!(["unknown"])
    );
    assert_eq!(gh.call_count(), 1); // Only the PR list call, no issue closure
}

// Validates provider_already_closed -> provider_already_closed idempotency:
// when an issue is already closed on the provider prior to the first reconcile run,
// both runs observe provider_already_closed and zero new issue closure records are appended on the second run.
#[test]
fn reconcile_twice_over_same_closed_fixture_writes_zero_records_second_run() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(None)));
    harness.install_custom_gh(&gh);

    let first = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        first["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    let first_written = harness.reconciliation_entry_count();

    let second = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        second["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(
        second["new_entries"].as_array().unwrap().len(),
        0,
        "second reconcile must write zero records"
    );
    let second_written = harness.reconciliation_entry_count();
    assert_eq!(
        second_written, first_written,
        "no new records written on second run"
    );
}

// Validates gah_reconciliation_write -> provider_already_closed idempotency on GitHub:
// Run 1 closes open issue via GAH (gah_reconciliation_write).
// Run 2 observes issue is now closed on provider (provider_already_closed), skips append and writes zero new entries.
#[test]
fn gah_closed_then_already_closed_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("open\n"),
        Scenario::success(),
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(None)));
    harness.install_custom_gh(&gh);

    let first = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(first["issue_closure"]["closed"], serde_json::json!(["42"]));
    let first_written = harness.reconciliation_entry_count();

    let second = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        second["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(
        second["issue_closure"]["skipped"],
        serde_json::json!(["42"]),
        "second reconcile must report the duplicate as skipped"
    );
    assert_eq!(
        second["new_entries"].as_array().unwrap().len(),
        0,
        "second reconcile after gah close must write zero records"
    );
    let second_written = harness.reconciliation_entry_count();
    assert_eq!(
        second_written, first_written,
        "no duplicate issue_closure written after mode transition"
    );
}

// Validates gah_reconciliation_write -> provider_already_closed idempotency on GitLab:
// Run 1 closes open issue via GAH (gah_reconciliation_write) using glab API.
// Run 2 observes issue is now closed on GitLab (provider_already_closed), skips append and writes zero new entries.
#[test]
fn gitlab_closed_then_already_closed_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let glab = FakeBackend::new(tmp.path(), "glab");
    glab.install_sequence(vec![
        // Run 1: MR list returns merged MR with "Closes #42" in description
        Scenario::success().with_stdout(
            r#"[{"title":"[GAH] Fix: TICKET-72","description":"Closes #42","source_branch":"gah/improve-example-123456","web_url":"https://gitlab.example.com/group/repo/-/merge_requests/72","labels":[],"iid":72,"state":"merged","draft":false,"merged_at":"2026-01-02T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","head_pipeline":{"status":"success"}}]"#,
        ),
        // GET issue state -> opened
        Scenario::success().with_stdout(r#"{"state":"opened"}"#),
        // PUT issue close -> closed
        Scenario::success().with_stdout(r#"{"state":"closed"}"#),
        // Run 2: MR list returns merged MR
        Scenario::success().with_stdout(
            r#"[{"title":"[GAH] Fix: TICKET-72","description":"Closes #42","source_branch":"gah/improve-example-123456","web_url":"https://gitlab.example.com/group/repo/-/merge_requests/72","labels":[],"iid":72,"state":"merged","draft":false,"merged_at":"2026-01-02T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","head_pipeline":{"status":"success"}}]"#,
        ),
        // GET issue state -> closed
        Scenario::success().with_stdout(r#"{"state":"closed"}"#),
    ]);

    let mut harness = ScenarioHarness::new("gitlab")
        .with_config_append(
            "provider_api_base = \"https://gitlab.example.com/api/v4\"\nprovider_project_id = \"123\"\n[profiles.test.publishing]\nallow_source_issue_closure = true\n",
        )
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(None)));
    harness.install_custom_glab(&glab);

    let first = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(first["issue_closure"]["closed"], serde_json::json!(["42"]));
    assert_eq!(glab.call_count(), 3);
    assert_eq!(
        glab.argv_for_call(3)[..2],
        ["api", "projects/123/issues/42"]
    );
    assert!(glab.argv_for_call(3).contains(&"--method".to_string()));
    assert!(glab.argv_for_call(3).contains(&"PUT".to_string()));
    let first_written = harness.reconciliation_entry_count();

    let second = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        second["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(
        second["issue_closure"]["skipped"],
        serde_json::json!(["42"]),
        "second reconcile on gitlab must report the duplicate as skipped"
    );
    assert_eq!(
        second["new_entries"].as_array().unwrap().len(),
        0,
        "second reconcile on gitlab after close must write zero records"
    );
    let second_written = harness.reconciliation_entry_count();
    assert_eq!(
        second_written, first_written,
        "no duplicate issue_closure written for gitlab after mode transition"
    );
}

#[test]
fn explicit_reference_matching_structured_source_takes_precedence() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("open\n"),
        Scenario::success(),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(Some("42"))));
    harness.install_custom_gh(&gh);

    let report = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(report["issue_closure"]["closed"], serde_json::json!(["42"]));
    assert_eq!(gh.argv_for_call(3)[..3], ["issue", "close", "42"]);
}

#[test]
fn reopened_issue_reclosed_for_same_work_id_appends_new_record() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        // Run 1: issue open -> closed by GAH
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("open\n"),
        Scenario::success(),
        // Run 2: issue closed on provider -> provider_already_closed, skipped
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
        // Run 3: issue reopened externally -> open on provider -> closed by GAH
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("open\n"),
        Scenario::success(),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(None)));
    harness.install_custom_gh(&gh);

    let first = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(first["issue_closure"]["closed"], serde_json::json!(["42"]));
    let count_after_first = harness.reconciliation_entry_count();

    let second = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        second["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(
        second["issue_closure"]["skipped"],
        serde_json::json!(["42"])
    );
    assert_eq!(second["new_entries"].as_array().unwrap().len(), 0);
    let count_after_second = harness.reconciliation_entry_count();
    assert_eq!(count_after_second, count_after_first);

    let third = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(third["issue_closure"]["closed"], serde_json::json!(["42"]));
    assert_eq!(third["issue_closure"]["skipped"], serde_json::json!([]));
    assert_eq!(third["new_entries"].as_array().unwrap().len(), 1);
    let count_after_third = harness.reconciliation_entry_count();
    assert_eq!(
        count_after_third,
        count_after_second + 1,
        "re-closure of reopened issue must append exactly one new record"
    );
}

#[test]
fn different_profiles_with_same_work_and_issue_do_not_dedupe_each_other() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
    ]);

    let glab = FakeBackend::new(tmp.path(), "glab");
    glab.install_sequence(vec![
        Scenario::success().with_stdout(
            r#"[{"title":"[GAH] Fix: TICKET-72","description":"Closes #42","source_branch":"gah/improve-example-123456","web_url":"https://gitlab.example.com/group/repo/-/merge_requests/72","labels":[],"iid":72,"state":"merged","draft":false,"merged_at":"2026-01-02T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","head_pipeline":{"status":"success"}}]"#,
        ),
        Scenario::success().with_stdout(r#"{"state":"closed"}"#),
    ]);

    let mut gitlab_ledger_entry = ledger_entry(None);
    gitlab_ledger_entry["profile"] = serde_json::json!("gitlab");
    gitlab_ledger_entry["repo_id"] = serde_json::json!("gitlab");
    let ledger = support::fake_ledger::TestLedger::new()
        .with_entry(ledger_entry(None))
        .with_entry(gitlab_ledger_entry);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(
            "[profiles.test.publishing]\nallow_source_issue_closure = true\n\n[profiles.gitlab]\ndisplay_name = \"GitLab Test\"\nrepo_id = \"gitlab\"\nrepo = \"group/repo\"\nlocal_path = \".\"\nartifact_root = \".\"\ndefault_target_branch = \"main\"\nprovider = \"gitlab\"\nprovider_api_base = \"https://gitlab.example.com/api/v4\"\nprovider_project_id = \"123\"\n[profiles.gitlab.publishing]\nallow_source_issue_closure = true\n",
        )
        .with_ledger(ledger);
    harness.install_custom_gh(&gh);
    harness.install_custom_glab(&glab);

    // Run reconcile on github profile ("test")
    let first = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        first["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(first["new_entries"].as_array().unwrap().len(), 2); // mr_state + issue_closure

    // Run reconcile on gitlab profile ("gitlab")
    let second = harness
        .run_ledger_reconcile_json_for_profile("gitlab", false)
        .unwrap();
    assert_eq!(
        second["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(
        second["issue_closure"]["skipped"],
        serde_json::json!([]),
        "gitlab profile closure must not be deduped by github profile closure"
    );
    assert_eq!(
        second["new_entries"].as_array().unwrap().len(),
        2,
        "gitlab profile must record its own mr_state and issue_closure"
    );
}

#[test]
fn dry_run_reports_duplicate_as_skipped() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install_sequence(vec![
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
        Scenario::success().with_stdout(merged_github_pr_json("Closes #42")),
        Scenario::success().with_stdout("closed\n"),
    ]);

    let mut harness = ScenarioHarness::new("github")
        .with_config_append(closure_enabled_toml())
        .with_ledger(support::fake_ledger::TestLedger::new().with_entry(ledger_entry(None)));
    harness.install_custom_gh(&gh);

    let first = harness.run_ledger_reconcile_json(false).unwrap();
    assert_eq!(
        first["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );

    let dry = harness.run_ledger_reconcile_json(true).unwrap();
    assert_eq!(
        dry["issue_closure"]["already_closed"],
        serde_json::json!(["42"])
    );
    assert_eq!(
        dry["issue_closure"]["skipped"],
        serde_json::json!(["42"]),
        "dry-run must report duplicate as skipped"
    );
    assert_eq!(
        dry["new_entries"].as_array().unwrap().len(),
        0,
        "dry-run must return empty new_entries for duplicate"
    );
}
