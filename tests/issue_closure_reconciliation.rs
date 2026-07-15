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
        glab.argv_for_call(1),
        vec![
            "mr",
            "list",
            "--repo",
            "group/repo",
            "--all",
            "--output",
            "json"
        ]
    );
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
