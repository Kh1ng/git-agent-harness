//! End-to-end status contract coverage for canonical issue dependencies.

mod support;

use support::scenario::ScenarioHarness;
use support::{FakeBackend, Scenario};
use tempfile::TempDir;

#[test]
fn status_json_v1_additively_surfaces_dependency_blockers() {
    let temp = TempDir::new().unwrap();
    let gh = FakeBackend::new(temp.path(), "gh");
    gh.install_sequence(vec![
        // sync::fetch_mrs
        Scenario::success().with_stdout("[]"),
        // Native issue discovery: #653 is deliberately the only listed issue,
        // forcing the prerequisite through the provider query path.
        Scenario::success().with_stdout(
            r#"[{"number":653,"title":"Approval notifications","body":"Blocked by: #652","labels":[],"author":{"login":"owner","type":"User","is_bot":false},"state":"OPEN"}]"#,
        ),
        // fetch_dependency_issue(#652)
        Scenario::success().with_stdout(r#"{"number":652,"body":"","state":"OPEN"}"#),
    ]);

    let mut harness = ScenarioHarness::new("github");
    harness.install_custom_gh(&gh);
    let snapshot = harness
        .run_status_json()
        .expect("status JSON should succeed");

    assert_eq!(snapshot["schema_version"], 1);
    let blockers = snapshot["dependency_blockers"]
        .as_array()
        .expect("new CLI must always serialize dependency_blockers");
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0]["work_id"], "#653");
    assert_eq!(blockers[0]["reason_code"], "dependency_open");
    assert_eq!(blockers[0]["dependencies"][0]["identity"], "#652");
    assert_eq!(blockers[0]["dependencies"][0]["normalized_state"], "open");
    assert!(snapshot["blocked_work_items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|blocker| blocker["source_reference"] == "#653"));
}
