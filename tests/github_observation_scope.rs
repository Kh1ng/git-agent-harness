//! Regression coverage for issue #665: recurring observations must not scan
//! the repository's complete pull-request history every controller tick.

mod support;

use support::scenario::ScenarioHarness;
use support::{FakeBackend, Scenario};
use tempfile::TempDir;

fn assert_scope(args: &[String], state: &str, limit: &str) {
    let state_pos = args.iter().position(|arg| arg == "--state").unwrap();
    let limit_pos = args.iter().position(|arg| arg == "--limit").unwrap();
    assert_eq!(args.get(state_pos + 1).map(String::as_str), Some(state));
    assert_eq!(args.get(limit_pos + 1).map(String::as_str), Some(limit));
}

#[test]
fn recurring_status_fetches_only_bounded_open_pull_requests() {
    let mut harness = ScenarioHarness::new("github").github_scenario("empty");
    harness.run_status_json().expect("status should succeed");

    let args = harness.github_argv_for_call(1);
    assert_eq!(args.first().map(String::as_str), Some("api"));
    assert!(
        args.iter().any(|arg| arg
            == "repos/owner/repo/pulls?state=open&per_page=100&sort=updated&direction=desc"),
        "{args:?}"
    );
    assert!(!args.iter().any(|arg| arg == "pr"), "{args:?}");
    assert!(!args.iter().any(|arg| arg == "list"), "{args:?}");
}

#[test]
fn explicit_sync_retains_full_history_for_reconciliation() {
    let mut harness = ScenarioHarness::new("github").github_scenario("empty");
    harness.run_sync_json().expect("sync should succeed");

    let args = harness.github_argv_for_call(1);
    assert_scope(&args, "all", "1000");
}

#[test]
fn active_observation_fails_closed_at_the_query_cap() {
    let prs: Vec<_> = (1..=100)
        .map(|number| {
            serde_json::json!({
                "number": number,
                "title": format!("[GAH] Fix: #{number} bounded observation"),
                "body": null,
                "html_url": format!("https://github.com/owner/repo/pull/{number}"),
                "state": "open",
                "draft": true,
                "head": {
                    "ref": format!("gah/bounded-{number}"),
                    "sha": format!("sha-{number}")
                },
                "labels": [],
                "updated_at": "2026-07-01T00:00:00Z"
            })
        })
        .collect();
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    gh.install(Scenario::success().with_stdout(serde_json::to_string(&prs).unwrap()));
    let mut harness = ScenarioHarness::new("github").github_scenario("empty");
    harness.install_custom_gh(&gh);

    let status = harness
        .run_status_json()
        .expect("status command should report the error");
    assert_eq!(status["observations"]["sync"]["status"], "error");
    assert!(status["merge_requests"].as_array().unwrap().is_empty());
    assert!(status["errors"].as_array().unwrap().iter().any(|error| {
        error["message"].as_str().is_some_and(|message| {
            message.contains("active observation open-PR snapshot reached its cap")
        })
    }));
}
