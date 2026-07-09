mod support;

use std::fs;

use support::fake_ledger::TestLedger;
use support::scenario::ScenarioHarness;

fn usage_entry(
    backend: &str,
    model: Option<&str>,
    top_level_usage: serde_json::Value,
    attempts: Vec<serde_json::Value>,
) -> serde_json::Value {
    let mut entry = support::fake_ledger::ledger_entry_full(
        "fix",
        "gah/test-usage-1",
        None,
        "TICKET-201",
        "2026-01-01T00:00:00Z",
    );
    let obj = entry.as_object_mut().unwrap();
    obj.insert(
        "effective_backend".into(),
        serde_json::Value::String(backend.to_string()),
    );
    obj.insert(
        "effective_model".into(),
        model
            .map(|m| serde_json::Value::String(m.to_string()))
            .unwrap_or(serde_json::Value::Null),
    );
    obj.insert("attempts".into(), serde_json::Value::Array(attempts));
    obj.insert("usage".into(), top_level_usage);
    entry
}

fn attempt_usage(
    attempt_number: u64,
    backend: &str,
    model: Option<&str>,
    usage: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "attempt_number": attempt_number,
        "backend": backend,
        "effective_model": model,
        "exit_code": 0,
        "validation_result": "passed",
        "failure_class": null,
        "failure_stage": null,
        "duration_seconds": 1.0,
        "diff_path": null,
        "usage": usage,
    })
}

fn write_exec(path: &std::path::Path, body: &str) {
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}

#[test]
fn dispatch_populates_top_level_usage_from_attempt_history() {
    let mut harness = ScenarioHarness::new("github").with_config_append(
        "[profiles.test.publishing]\nallow_pull_request_creation = false\nallow_commit_message_generation = false\n",
    );
    write_exec(
        &harness.bin_dir.join("openhands"),
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nprintf 'input_tokens: 500\\noutput_tokens: 120\\nestimated_cost_usd: 0.02\\n'\n",
    );
    write_exec(
        &harness.bin_dir.join("gh"),
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/repo/pull/1\\n'; exit 0; fi\nexit 0\n",
    );

    let result = harness
        .run_dispatch(&[
            "--mode",
            "fix",
            "--backend",
            "openhands",
            "--target",
            "repair this",
        ])
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "stderr was {}", result.stderr);

    let ledger_entries = TestLedger::read_from(&harness.ledger_path).unwrap();
    let entry = ledger_entries.last().unwrap();
    assert_eq!(entry["attempts"][0]["usage"]["input_tokens"], 500);
    assert_eq!(entry["attempts"][0]["usage"]["output_tokens"], 120);
    assert_eq!(entry["usage"]["input_tokens"], 500);
    assert_eq!(entry["usage"]["output_tokens"], 120);
    assert_eq!(entry["usage"]["total_tokens"], 620);
    assert_eq!(entry["usage"]["estimated_cost_usd"], 0.02);
    assert_eq!(entry["usage"]["usage_source"], "attempt_aggregate");
}

#[test]
fn report_counts_mirrored_top_level_and_attempt_usage_once() {
    let attempts = vec![attempt_usage(
        1,
        "codex",
        Some("gpt-4"),
        serde_json::json!({
            "usage_source": "attempt_output_log",
            "observed_at": "2026-01-01T00:00:00Z",
            "input_tokens": 500,
            "output_tokens": 120,
            "total_tokens": 620,
            "estimated_cost_usd": 0.02
        }),
    )];
    let top_level = serde_json::json!({
        "usage_source": "attempt_aggregate",
        "observed_at": "2026-01-01T00:00:00Z",
        "input_tokens": 500,
        "output_tokens": 120,
        "total_tokens": 620,
        "estimated_cost_usd": 0.02
    });
    let ledger =
        TestLedger::new().with_entry(usage_entry("codex", Some("gpt-4"), top_level, attempts));

    let mut harness = ScenarioHarness::new("github").with_ledger(ledger);
    let report = harness.run_report_json("backend").unwrap();
    let codex = report["comparisons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["backend_or_model"] == "codex")
        .unwrap();

    assert_eq!(codex["input_tokens"], 500);
    assert_eq!(codex["output_tokens"], 120);
    assert_eq!(codex["total_tokens"], 620);
    assert_eq!(codex["estimated_cost_usd"], 0.02);
}

#[test]
fn report_attributes_usage_to_actual_attempt_backend_and_model() {
    let attempts = vec![
        attempt_usage(
            1,
            "codex",
            Some("gpt-4"),
            serde_json::json!({
                "usage_source": "attempt_output_log",
                "observed_at": "2026-01-01T00:00:00Z",
                "input_tokens": 100,
                "total_tokens": 100
            }),
        ),
        attempt_usage(
            2,
            "vibe",
            Some("mistral-medium"),
            serde_json::json!({
                "usage_source": "attempt_output_log",
                "observed_at": "2026-01-01T00:01:00Z",
                "input_tokens": 200,
                "actual_cost_usd": 0.25,
                "total_tokens": 200
            }),
        ),
    ];
    let ledger = TestLedger::new().with_entry(usage_entry(
        "codex",
        Some("gpt-4"),
        serde_json::json!({}),
        attempts,
    ));

    let mut harness = ScenarioHarness::new("github").with_ledger(ledger);
    let backend_report = harness.run_report_json("backend").unwrap();
    let rows = backend_report["comparisons"].as_array().unwrap();
    let codex = rows
        .iter()
        .find(|row| row["backend_or_model"] == "codex")
        .unwrap();
    let vibe = rows
        .iter()
        .find(|row| row["backend_or_model"] == "vibe")
        .unwrap();
    assert_eq!(codex["attempts"], 1);
    assert_eq!(codex["input_tokens"], 100);
    assert_eq!(vibe["attempts"], 1);
    assert_eq!(vibe["input_tokens"], 200);
    assert_eq!(vibe["actual_cost_usd"], 0.25);

    let model_report = harness.run_report_json("model").unwrap();
    let model_rows = model_report["comparisons"].as_array().unwrap();
    assert!(model_rows
        .iter()
        .any(|row| row["backend_or_model"] == "gpt-4" && row["input_tokens"] == 100));
    assert!(model_rows
        .iter()
        .any(|row| row["backend_or_model"] == "mistral-medium" && row["input_tokens"] == 200));
}

#[test]
fn report_preserves_unknown_and_exposes_quota_observations() {
    let mut ledger = TestLedger::new();
    ledger = ledger.with_entry(usage_entry(
        "claude",
        Some("claude-sonnet"),
        serde_json::json!({
            "usage_source": "subscription_status",
            "observed_at": "2026-01-02T03:04:05Z",
            "quota_window": "weekly",
            "quota_remaining_percent": 38.0,
            "quota_reset_at": "2026-01-12T00:00:00Z"
        }),
        vec![],
    ));
    ledger = ledger.with_entry(usage_entry(
        "agy-second",
        Some("gpt-5.4"),
        serde_json::json!({}),
        vec![],
    ));

    let mut harness = ScenarioHarness::new("github").with_ledger(ledger);
    let report = harness.run_report_json("backend").unwrap();
    let rows = report["comparisons"].as_array().unwrap();
    let claude = rows
        .iter()
        .find(|row| row["backend_or_model"] == "claude")
        .unwrap();
    let agy_second = rows
        .iter()
        .find(|row| row["backend_or_model"] == "agy-second")
        .unwrap();

    assert_eq!(agy_second["input_tokens"], serde_json::Value::Null);
    assert_eq!(agy_second["actual_cost_usd"], serde_json::Value::Null);

    let quota = &claude["quota_observations"][0];
    assert_eq!(quota["backend"], "claude");
    assert_eq!(quota["quota_window"], "weekly");
    assert_eq!(quota["quota_remaining_percent"], 38.0);
    assert_eq!(quota["quota_reset_at"], "2026-01-12T00:00:00Z");
    assert_eq!(quota["observed_at"], "2026-01-02T03:04:05Z");
    assert_eq!(quota["usage_source"], "subscription_status");
}
