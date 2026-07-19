mod support;

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use support::fake_ledger::{ledger_entry_full, TestLedger};
use support::scenario::ScenarioHarness;

const UPDATE_FIXTURES_ENV: &str = "GAH_UPDATE_CONTRACT_FIXTURES";

#[test]
fn status_json_matches_checked_fixture_recursively() {
    let mut harness = ScenarioHarness::new("github").github_scenario("one_pr_needs_review");
    let payload = harness.run_status_json().expect("status --json must run");
    assert_or_update_fixture("status.json", &payload);
}

#[test]
fn quota_list_json_matches_checked_fixture_recursively() {
    let mut harness = ScenarioHarness::new("github").github_scenario("empty");
    let payload = harness
        .run_quota_list_json()
        .expect("quota list --json must run");
    assert_or_update_fixture("quota-list.json", &payload);
}

#[test]
fn report_json_matches_checked_fixture_recursively() {
    let mut entry = ledger_entry_full(
        "implementation",
        "gah/contracts-fixture",
        Some("agent-ready"),
        "#637",
        "2026-07-19T00:00:00Z",
    );
    let object = entry
        .as_object_mut()
        .expect("ledger entry must be an object");
    object.insert(
        "effective_model".into(),
        Value::String("gpt-5.4-mini".into()),
    );
    object.insert("duration_seconds".into(), serde_json::json!(12.5));
    object.insert("validation_result".into(), Value::String("passed".into()));
    object.insert("review_verdict".into(), Value::String("APPROVE".into()));
    object.insert(
        "usage".into(),
        serde_json::json!({
            "usage_source": "codex_session",
            "usage_classification": "quota_backed",
            "backend_instance": "codex",
            "provider": "openai",
            "actual_model": "gpt-5.4-mini",
            "input_tokens": 100,
            "output_tokens": 40,
            "reasoning_tokens": 10,
            "cache_read_tokens": 20,
            "cache_write_tokens": 5,
            "total_tokens": 175,
            "requests_count": 1,
            "estimated_cost_usd": null,
            "actual_cost_usd": null,
            "quota_window": "weekly",
            "quota_used_percent": 25.0,
            "quota_remaining_percent": 75.0,
            "quota_reset_at": "2026-07-20T00:00:00Z"
        }),
    );
    let ledger = TestLedger::new().with_entry(entry);
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .with_ledger(ledger);
    let payload = harness
        .run_report_json("backend")
        .expect("report --json must run");
    assert_or_update_fixture("report.json", &payload);
}

fn assert_or_update_fixture(name: &str, actual: &Value) {
    let path = fixture_path(name);
    if std::env::var_os(UPDATE_FIXTURES_ENV).is_some() {
        fs::create_dir_all(path.parent().expect("fixture path has parent"))
            .expect("create contract fixture directory");
        fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(actual).expect("serialize fixture")
            ),
        )
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        return;
    }

    let expected: Value = serde_json::from_slice(&fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e}; regenerate with `{}`",
            path.display(),
            regeneration_command()
        )
    }))
    .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    assert_same_json_shape("$", actual, &expected);
}

fn assert_same_json_shape(path: &str, actual: &Value, expected: &Value) {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            let actual_keys = actual.keys().collect::<std::collections::BTreeSet<_>>();
            let expected_keys = expected.keys().collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                actual_keys,
                expected_keys,
                "object field drift at {path}; regenerate with `{}`",
                regeneration_command()
            );
            for (key, actual_value) in actual {
                assert_same_json_shape(&format!("{path}.{key}"), actual_value, &expected[key]);
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(
                actual.len(),
                expected.len(),
                "array shape drift at {path}; regenerate with `{}`",
                regeneration_command()
            );
            for (index, (actual_value, expected_value)) in
                actual.iter().zip(expected.iter()).enumerate()
            {
                assert_same_json_shape(&format!("{path}[{index}]"), actual_value, expected_value);
            }
        }
        _ => assert_eq!(
            json_kind(actual),
            json_kind(expected),
            "JSON type drift at {path}; regenerate with `{}`",
            regeneration_command()
        ),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("packages/contracts/src/fixtures")
        .join(name)
}

fn regeneration_command() -> &'static str {
    "GAH_UPDATE_CONTRACT_FIXTURES=1 cargo test --test contracts_drift"
}
