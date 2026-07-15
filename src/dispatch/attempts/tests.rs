use super::*;
use crate::availability::{availability_for, load_state, Reason};
use crate::config::RoutingPolicy;
use crate::dispatch::test_util::{gah_config, gah_config_with_ledger, profile};
use crate::ledger::LedgerEntry;
use crate::routing::{CandidateIdentity, RouteRequest};
use crate::test_support::PathGuard;
use std::fs;
use std::path::Path;
use time::OffsetDateTime;

const CODEX_FULL_RESET: &str =
    include_str!("../../../tests/fixtures/quota-logs/codex_usage_exhausted_full_reset.txt");
const OPENCODE_HY3_RATE_LIMIT: &str =
    include_str!("../../../tests/fixtures/quota-logs/opencode_hy3_rate_limit.log");

#[test]
fn review_preflight_fails_with_backend_unavailable_when_executable_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.claude_path = Some("/definitely/does/not/exist/claude".into());
    let cfg = gah_config(RoutingPolicy::default());

    let err = review_preflight(&cfg, &prof, "claude").unwrap_err();
    assert!(format!("{:#}", err).contains("backend unavailable"));
}

#[test]
fn attempt_usage_parses_real_log_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("backend-output.log");
    fs::write(
        &path,
        "some agent output\ninput_tokens: 500\noutput_tokens: 120\n",
    )
    .unwrap();

    let usage = attempt_usage(
        path.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("vibe"), None),
        None,
        None,
    );
    assert_eq!(usage.input_tokens, Some(500));
    assert_eq!(usage.output_tokens, Some(120));
    assert_eq!(usage.total_tokens, Some(620));
}

#[test]
fn attempt_usage_attributes_missing_artifact_without_fabricating_tokens() {
    let usage = attempt_usage(
        "/definitely/does/not/exist/backend-output.log",
        None,
        UsageAttribution::backend(Some("codex"), Some("gpt-5.4-mini")),
        None,
        None,
    );
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
    assert_eq!(usage.provider.as_deref(), Some("openai"));
    assert_eq!(usage.usage_classification.as_deref(), Some("quota_backed"));
    assert!(usage.actual_model_unknown_reason.is_some());
}

#[test]
fn attempt_usage_is_empty_when_log_has_no_usage_info() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("backend-output.log");
    fs::write(&path, "agent made some edits, no usage reported\n").unwrap();

    let usage = attempt_usage(
        path.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("vibe"), None),
        None,
        None,
    );
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
    assert_eq!(usage.requests_count, Some(1));
    assert_eq!(usage.usage_classification, Some("quota_backed".to_string()));
}

#[test]
fn attempt_usage_records_the_bound_agy_model_when_cli_logs_only_quota_state() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("backend-output.log");
    fs::write(&path, "completed successfully\n").unwrap();

    let usage = attempt_usage(
        path.to_str().unwrap(),
        Some("quotaRefreshLoop: completed"),
        UsageAttribution::backend(Some("agy"), Some("Gemini 3.5 Flash (Medium)")),
        None,
        None,
    );

    assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
    assert_eq!(usage.usage_classification.as_deref(), Some("quota_backed"));
    assert_eq!(usage.provider.as_deref(), Some("google"));
    assert_eq!(
        usage.actual_model.as_deref(),
        Some("Gemini 3.5 Flash (Medium)")
    );
    assert_eq!(usage.requests_count, Some(1));
    assert_eq!(usage.quota_window.as_deref(), Some("AGY individual quota"));
}

#[test]
fn review_usage_records_an_agy_review_without_token_counters() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("review-stdout.log");
    fs::write(&path, "review completed; no token counters exposed\n").unwrap();

    let usage = review_usage(
        path.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("agy"), Some("Claude Sonnet 4.6 (Thinking)")),
        None,
        None,
    );

    assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
    assert_eq!(usage.usage_classification.as_deref(), Some("quota_backed"));
    assert_eq!(usage.backend_instance.as_deref(), Some("agy"));
    assert_eq!(usage.provider.as_deref(), Some("anthropic"));
    assert_eq!(
        usage.actual_model.as_deref(),
        Some("Claude Sonnet 4.6 (Thinking)")
    );
    assert_eq!(usage.requests_count, Some(1));
    assert!(usage.token_usage_unknown_reason.is_some());
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.quota_window.as_deref(), Some("AGY individual quota"));
}

#[test]
fn review_usage_consumes_each_backends_run_scoped_artifact() {
    let tmp = tempfile::tempdir().unwrap();

    let claude_log = tmp.path().join("claude.log");
    let claude_transcript = tmp.path().join("claude.jsonl");
    fs::write(&claude_log, "review complete\n").unwrap();
    fs::write(
        &claude_transcript,
        r#"{"type":"assistant","message":{"model":"claude-sonnet-5","usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":40}},"cost_usd":9.99}"#,
    )
    .unwrap();
    let claude = review_usage(
        claude_log.to_str().unwrap(),
        None,
        UsageAttribution::routed("claude", "sonnet", "claude-main", "included_quota"),
        claude_transcript.to_str(),
        None,
    );
    assert_eq!(claude.actual_model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(claude.input_tokens, Some(100));
    assert_eq!(claude.actual_cost_usd, None);

    let codex_log = tmp.path().join("codex.jsonl");
    let codex_transcript = tmp.path().join("codex-transcript.jsonl");
    fs::write(
        &codex_log,
        r#"{"type":"turn.completed","usage":{"input_tokens":80,"output_tokens":12,"reasoning_output_tokens":3}}"#,
    )
    .unwrap();
    fs::write(
        &codex_transcript,
        "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4-mini\"}}\n",
    )
    .unwrap();
    let codex = review_usage(
        codex_log.to_str().unwrap(),
        None,
        UsageAttribution::routed("codex", "gpt-5.4-mini", "codex-main", "included_quota"),
        codex_transcript.to_str(),
        None,
    );
    assert_eq!(codex.actual_model.as_deref(), Some("gpt-5.4-mini"));
    assert_eq!(codex.reasoning_tokens, Some(3));

    let vibe_meta = tmp.path().join("vibe-meta.json");
    fs::write(
        &vibe_meta,
        r#"{"config":{"active_model":"mistral-medium-3.5"},"stats":{"session_prompt_tokens":70,"session_completion_tokens":11,"steps":2}}"#,
    )
    .unwrap();
    let vibe = review_usage(
        claude_log.to_str().unwrap(),
        None,
        UsageAttribution::routed(
            "vibe",
            "mistral-medium-3.5",
            "vibe-monthly",
            "included_quota",
        ),
        vibe_meta.to_str(),
        None,
    );
    assert_eq!(vibe.actual_model.as_deref(), Some("mistral-medium-3.5"));
    assert_eq!(vibe.total_tokens, Some(81));

    let opencode_meta = tmp.path().join("opencode-session.json");
    fs::write(
        &opencode_meta,
        r#"{"model":{"id":"glm-5.2","providerID":"z-ai"},"tokens_input":60,"tokens_output":10,"tokens_reasoning":4,"tokens_cache_read":0,"tokens_cache_write":0,"actual_cost_usd":0.0042}"#,
    )
    .unwrap();
    let opencode = review_usage(
        claude_log.to_str().unwrap(),
        None,
        UsageAttribution::routed(
            "opencode",
            "nous-portal/z-ai/glm-5.2",
            "nous-portal-api",
            "paid",
        ),
        opencode_meta.to_str(),
        None,
    );
    assert_eq!(opencode.actual_model.as_deref(), Some("glm-5.2"));
    assert_eq!(opencode.actual_cost_usd, Some(0.0042));

    let agy = review_usage(
        claude_log.to_str().unwrap(),
        Some("Quota exceeded. Resets in 16m44s."),
        UsageAttribution::routed(
            "agy-second",
            "Claude Sonnet 4.8 (Thinking)",
            "agy-account-2",
            "included_quota",
        ),
        None,
        None,
    );
    assert_eq!(
        agy.backend_instance.as_deref(),
        Some("agy-second:agy-account-2")
    );
    assert!(agy
        .usage_source
        .as_deref()
        .is_some_and(|source| source.contains("agy_cli_log_delta")));
    assert!(agy.quota_reset_at.is_some());
}

#[test]
fn attempt_usage_does_not_scrape_codex_tool_output_as_usage() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("backend-output.log");
    fs::write(
        &path,
        r#"{"type":"item.completed","item":{"aggregated_output":"input_tokens: 500"}}
{"type":"item.started","item":{"type":"command_execution"}}
"#,
    )
    .unwrap();

    let usage = attempt_usage(
        path.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("codex"), None),
        None,
        None,
    );
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.output_tokens, None);
    assert_eq!(usage.requests_count, Some(1));
    assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
}

#[test]
fn implementation_escalation_ignores_review_failure_routes() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
    let prof = profile(tmp.path());
    let mut review = LedgerEntry::new("test", &prof, "codex", "review", "x", None, None);
    review.work_id = Some("ISSUE-42".into());
    review.effective_backend = "codex".into();
    review.effective_model = Some("review-model".into());
    review.set_failure(
        crate::ledger::FailureClass::AgentFailure,
        crate::ledger::FailureStage::AgentRun,
    );
    crate::ledger::append(&cfg, &review).unwrap();

    let mut current = LedgerEntry::new("test", &prof, "auto", "fix", "x", None, None);
    current.work_id = Some("ISSUE-42".into());
    let state = routing_runtime_state(&cfg, &current).unwrap();
    assert!(state.attempted.is_empty());

    let mut implementation = LedgerEntry::new("test", &prof, "codex", "improve", "x", None, None);
    implementation.work_id = Some("ISSUE-42".into());
    implementation.effective_backend = "codex".into();
    implementation.effective_model = Some("worker-model".into());
    implementation.set_failure(
        crate::ledger::FailureClass::AgentFailure,
        crate::ledger::FailureStage::AgentRun,
    );
    crate::ledger::append(&cfg, &implementation).unwrap();

    let state = routing_runtime_state(&cfg, &current).unwrap();
    assert_eq!(state.attempted.len(), 1);
    assert!(state
        .attempted
        .contains(&CandidateIdentity::new("codex", Some("worker-model"))));
}

fn make_fake_bin(dir: &Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }
    path
}

#[test]
fn agy_second_backend_runs_with_agy_second_home_override() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let home_capture = tmp.path().join("captured-home.txt");

    let fake_agy = bin_dir.join("agy");
    fs::write(
        &fake_agy,
        format!(
            "#!/bin/sh\necho \"$HOME\" > {}\nexit 0\n",
            home_capture.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_agy).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_agy, perms).unwrap();
    }

    let mut prof = profile(tmp.path());
    prof.agy_path = Some(fake_agy.display().to_string());
    prof.agy_second_home = Some("/tmp/second-account-home".to_string());

    let session_dir = tmp.path().join("session");
    fs::create_dir_all(&session_dir).unwrap();
    let llm = crate::runner::LlmConfig {
        base_url: String::new(),
        api_key: String::new(),
        model: "Gemini 3.5 Flash (Medium)".to_string(),
    };

    run_backend(
        "agy-second",
        &prof,
        tmp.path(),
        "do the thing",
        &session_dir,
        &llm,
        None,
        None,
    )
    .unwrap();

    let captured = fs::read_to_string(&home_capture).unwrap();
    assert_eq!(captured.trim(), "/tmp/second-account-home");
}

#[test]
fn agy_backend_without_second_home_uses_real_home() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let home_capture = tmp.path().join("captured-home.txt");

    let fake_agy = bin_dir.join("agy");
    fs::write(
        &fake_agy,
        format!(
            "#!/bin/sh\necho \"$HOME\" > {}\nexit 0\n",
            home_capture.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_agy).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_agy, perms).unwrap();
    }

    let mut prof = profile(tmp.path());
    prof.agy_path = Some(fake_agy.display().to_string());
    prof.agy_second_home = Some("/tmp/second-account-home".to_string());

    let session_dir = tmp.path().join("session");
    fs::create_dir_all(&session_dir).unwrap();
    let llm = crate::runner::LlmConfig {
        base_url: String::new(),
        api_key: String::new(),
        model: "Gemini 3.5 Flash (Medium)".to_string(),
    };

    run_backend(
        "agy",
        &prof,
        tmp.path(),
        "do the thing",
        &session_dir,
        &llm,
        None,
        None,
    )
    .unwrap();

    let captured = fs::read_to_string(&home_capture).unwrap();
    assert_ne!(captured.trim(), "/tmp/second-account-home");
}

#[test]
fn run_backend_looks_up_agy_print_timeout_by_exact_model_name() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let argv_capture = tmp.path().join("captured-argv.txt");

    let fake_agy = bin_dir.join("agy");
    fs::write(
        &fake_agy,
        format!(
            "#!/bin/sh\necho \"$@\" > {}\nexit 0\n",
            argv_capture.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_agy).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_agy, perms).unwrap();
    }

    let mut prof = profile(tmp.path());
    prof.agy_path = Some(fake_agy.display().to_string());
    prof.agy_print_timeout_seconds
        .insert("Gemini 3.5 Flash (Medium)".to_string(), 900);
    prof.agy_print_timeout_seconds
        .insert("Gemini 3.1 Pro (High)".to_string(), 300);

    let session_dir = tmp.path().join("session");
    fs::create_dir_all(&session_dir).unwrap();
    let llm = crate::runner::LlmConfig {
        base_url: String::new(),
        api_key: String::new(),
        model: "Gemini 3.5 Flash (Medium)".to_string(),
    };

    run_backend(
        "agy",
        &prof,
        tmp.path(),
        "do the thing",
        &session_dir,
        &llm,
        None,
        None,
    )
    .unwrap();

    let captured = fs::read_to_string(&argv_capture).unwrap();
    assert!(captured.contains("--print-timeout 900s"), "got: {captured}");
}

#[test]
fn run_backend_omits_print_timeout_for_unmapped_model() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let argv_capture = tmp.path().join("captured-argv.txt");

    let fake_agy = bin_dir.join("agy");
    fs::write(
        &fake_agy,
        format!(
            "#!/bin/sh\necho \"$@\" > {}\nexit 0\n",
            argv_capture.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_agy).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_agy, perms).unwrap();
    }

    let mut prof = profile(tmp.path());
    prof.agy_path = Some(fake_agy.display().to_string());
    prof.agy_print_timeout_seconds
        .insert("Gemini 3.5 Flash (Medium)".to_string(), 900);

    let session_dir = tmp.path().join("session");
    fs::create_dir_all(&session_dir).unwrap();
    let llm = crate::runner::LlmConfig {
        base_url: String::new(),
        api_key: String::new(),
        model: "Gemini 3.1 Pro (High)".to_string(), // not in the map
    };

    run_backend(
        "agy",
        &prof,
        tmp.path(),
        "do the thing",
        &session_dir,
        &llm,
        None,
        None,
    )
    .unwrap();

    let captured = fs::read_to_string(&argv_capture).unwrap();
    assert!(!captured.contains("--print-timeout"), "got: {captured}");
}

/// Issue: opencode routes both a free-tier model that hangs at zero
/// output when rate-limited (kill fast) and a real-but-slow self-hosted
/// litellm model (give it more time) through the same flat
/// `opencode_idle_timeout_seconds`. Mirrors
/// `run_backend_looks_up_agy_print_timeout_by_exact_model_name`: prove
/// the per-model override in `opencode_idle_timeout_seconds_by_model`
/// is what actually governs the kill, not the flat default, by setting
/// the flat default so high the test would hang if it were used.
#[test]
fn run_backend_looks_up_opencode_idle_timeout_by_exact_model_name() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let fake_opencode = bin_dir.join("opencode");
    fs::write(
        &fake_opencode,
        "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_opencode).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_opencode, perms).unwrap();
    }

    let mut prof = profile(tmp.path());
    prof.opencode_path = Some(fake_opencode.display().to_string());
    prof.opencode_idle_timeout_seconds = Some(100); // flat default: would hang the test if used
    prof.opencode_idle_timeout_seconds_by_model
        .insert("litellm-lan/qwen3.6:35b-a3b".to_string(), 1);

    let session_dir = tmp.path().join("session");
    fs::create_dir_all(&session_dir).unwrap();
    let llm = crate::runner::LlmConfig {
        base_url: String::new(),
        api_key: String::new(),
        model: "unused-for-opencode".to_string(),
    };

    let result = run_backend(
        "opencode",
        &prof,
        tmp.path(),
        "do the thing",
        &session_dir,
        &llm,
        Some("litellm-lan/qwen3.6:35b-a3b"),
        None,
    )
    .unwrap();

    assert_eq!(result.exit_code, -1);
    let log = fs::read_to_string(&result.log_path).unwrap();
    assert!(
        log.contains("killed after 1s with no new worktree progress"),
        "got: {log}"
    );
}

/// Complement to the above: a model with no per-model entry must fall
/// back to the flat `opencode_idle_timeout_seconds`, not silently pick
/// up some other model's override.
#[test]
fn run_backend_falls_back_to_flat_opencode_idle_timeout_for_unmapped_model() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let fake_opencode = bin_dir.join("opencode");
    fs::write(
        &fake_opencode,
        "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_opencode).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_opencode, perms).unwrap();
    }

    let mut prof = profile(tmp.path());
    prof.opencode_path = Some(fake_opencode.display().to_string());
    prof.opencode_idle_timeout_seconds = Some(1); // flat fallback: should apply
    prof.opencode_idle_timeout_seconds_by_model
        .insert("hy3-free".to_string(), 100); // a different model's override

    let session_dir = tmp.path().join("session");
    fs::create_dir_all(&session_dir).unwrap();
    let llm = crate::runner::LlmConfig {
        base_url: String::new(),
        api_key: String::new(),
        model: "unused-for-opencode".to_string(),
    };

    let result = run_backend(
        "opencode",
        &prof,
        tmp.path(),
        "do the thing",
        &session_dir,
        &llm,
        Some("litellm-lan/qwen3.6:35b-a3b"), // not in the map
        None,
    )
    .unwrap();

    assert_eq!(result.exit_code, -1);
    let log = fs::read_to_string(&result.log_path).unwrap();
    assert!(
        log.contains("killed after 1s with no new worktree progress"),
        "got: {log}"
    );
}

#[test]
fn run_backend_routes_vibe_to_run_vibe_not_the_openhands_fallthrough() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    // Regression: run_backend's match had a catch-all `_ => run_openhands(...)`.
    // An unrecognized backend name silently ran openhands instead of
    // erroring -- adding "vibe" without an explicit match arm would have
    // silently spent real OpenHands API $ on every "vibe" dispatch instead
    // of running vibe at all.
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let marker = tmp.path().join("which-backend-ran.txt");

    let fake_vibe = bin_dir.join("vibe");
    fs::write(
        &fake_vibe,
        format!("#!/bin/sh\necho vibe > {}\nexit 0\n", marker.display()),
    )
    .unwrap();
    fs::write(
        bin_dir.join("openhands"),
        format!("#!/bin/sh\necho openhands > {}\nexit 0\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for bin in ["vibe", "openhands"] {
            let path = bin_dir.join(bin);
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
    }

    let mut prof = profile(tmp.path());
    prof.vibe_path = Some(fake_vibe.display().to_string());

    let session_dir = tmp.path().join("session");
    fs::create_dir_all(&session_dir).unwrap();
    let llm = crate::runner::LlmConfig {
        base_url: String::new(),
        api_key: String::new(),
        model: "unused-for-vibe".to_string(),
    };

    run_backend(
        "vibe",
        &prof,
        tmp.path(),
        "do the thing",
        &session_dir,
        &llm,
        None,
        None,
    )
    .unwrap();

    assert_eq!(fs::read_to_string(&marker).unwrap().trim(), "vibe");
}

#[test]
fn apply_route_to_ledger_records_effective_model() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entry = LedgerEntry::new(
        "test",
        &profile(tmp.path()),
        "codex",
        "improve",
        "target",
        Some("session-1".into()),
        None,
    );
    let route = RouteDecision {
        requested_backend: "auto".into(),
        effective_backend: "codex".into(),
        requested_model: None,
        effective_model: Some("claude-sonnet-4".into()),
        effective_quota_pool: None,
        routing_reason: "ticket recommendation".into(),
        fallback_used: false,
        confidence_impact: None,
        human_required: false,
        routing_diagnostics: None,
    };

    apply_route_to_ledger(&mut entry, &route);

    assert_eq!(entry.effective_model.as_deref(), Some("claude-sonnet-4"));
    assert_eq!(entry.effective_backend, "codex");
    assert_eq!(
        entry.routing_reason.as_deref(),
        Some("ticket recommendation")
    );
}

#[test]
fn record_route_attempt_preserves_each_route_and_its_diagnostics() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entry = LedgerEntry::new(
        "test",
        &profile(tmp.path()),
        "codex",
        "improve",
        "target",
        Some("session-1".into()),
        None,
    );
    let first = RouteDecision {
        requested_backend: "auto".into(),
        effective_backend: "agy".into(),
        requested_model: None,
        effective_model: Some("gemini".into()),
        effective_quota_pool: None,
        routing_reason: "profile routing policy".into(),
        fallback_used: false,
        confidence_impact: None,
        human_required: false,
        routing_diagnostics: Some(crate::ledger::RoutingDiagnostics {
            selected_backend: Some("agy".into()),
            selected_model: Some("gemini".into()),
            human_summary: Some("selected agy/gemini".into()),
            ..Default::default()
        }),
    };
    let second = RouteDecision {
        effective_backend: "codex".into(),
        effective_model: Some("gpt-5.4-mini".into()),
        fallback_used: true,
        routing_diagnostics: Some(crate::ledger::RoutingDiagnostics {
            selected_backend: Some("codex".into()),
            selected_model: Some("gpt-5.4-mini".into()),
            human_summary: Some("agy skipped: quota_exhausted".into()),
            ..Default::default()
        }),
        ..first.clone()
    };

    record_route_attempt(&mut entry, &first);
    record_route_attempt(&mut entry, &second);

    assert!(entry
        .routing_runtime
        .dispatch_attempted
        .contains(&CandidateIdentity::new("agy", Some("gemini"))));
    assert_eq!(entry.attempt_routing.len(), 2);
    assert_eq!(entry.attempt_routing[0].backend_instance, "agy");
    assert_eq!(
        entry.attempt_routing[1]
            .routing_diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.human_summary.as_deref()),
        Some("agy skipped: quota_exhausted")
    );

    let serialized = serde_json::to_string(&entry).unwrap();
    let parsed: LedgerEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed.attempt_routing, entry.attempt_routing);
    assert!(parsed.routing_runtime.dispatch_attempted.is_empty());
}

#[test]
fn apply_route_to_ledger_leaves_null_when_no_model() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entry = LedgerEntry::new(
        "test",
        &profile(tmp.path()),
        "codex",
        "fix",
        "target",
        Some("session-1".into()),
        None,
    );
    let route = RouteDecision {
        requested_backend: "auto".into(),
        effective_backend: "openhands".into(),
        requested_model: None,
        effective_model: None,
        effective_quota_pool: None,
        routing_reason: "profile routing policy".into(),
        fallback_used: false,
        confidence_impact: None,
        human_required: false,
        routing_diagnostics: None,
    };

    apply_route_to_ledger(&mut entry, &route);

    assert_eq!(entry.effective_model, None);
    assert_eq!(entry.effective_backend, "openhands");
}

// Live incident: a `git fetch` failure during worktree setup (bad
// remote URL, auth prompt) propagated via `?` past every
// `ledger.set_failure()` call site, leaving `failure_class` `None` in
// the ledger and making the ticket permanently un-retryable (see
// `git_fetch_harness_error_is_retried_not_orphaned` in controller.rs).
// `classify_worktree_result` is the fix: it must classify the error as
// `harness_error`/`preflight` before propagating it.
#[test]
fn classify_worktree_result_sets_harness_error_on_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entry = LedgerEntry::new(
        "test",
        &profile(tmp.path()),
        "codex",
        "fix",
        "target",
        Some("session-1".into()),
        None,
    );
    assert_eq!(entry.failure_class, None);

    let result: anyhow::Result<()> = Err(anyhow::anyhow!(
        "git fetch -q origin --prune: fatal: could not read Username for 'https://gitlab.com': terminal prompts disabled"
    ));
    let classified = classify_worktree_result(&mut entry, result);

    assert!(classified.is_err());
    assert_eq!(entry.failure_class.as_deref(), Some("harness_error"));
    assert_eq!(entry.failure_stage.as_deref(), Some("preflight"));
}

#[test]
fn transient_git_failure_is_environment_error_without_backend_side_effects() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entry = LedgerEntry::new(
        "test",
        &profile(tmp.path()),
        "codex",
        "fix",
        "target",
        Some("session-1".into()),
        None,
    );
    let result: anyhow::Result<()> = Err(anyhow::anyhow!(
        "push failed: ssh: connect to host github.com port 22: Connection timed out"
    ));

    let classified =
        classify_git_operation_result(&mut entry, crate::ledger::FailureStage::Push, result);

    assert!(classified.is_err());
    assert_eq!(entry.failure_class.as_deref(), Some("environment_error"));
    assert_eq!(entry.failure_stage.as_deref(), Some("push"));
    assert!(
        entry.attempts.is_empty(),
        "git weather must not look like an agent attempt"
    );
}

#[test]
fn classify_worktree_result_leaves_ledger_untouched_on_success() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entry = LedgerEntry::new(
        "test",
        &profile(tmp.path()),
        "codex",
        "fix",
        "target",
        Some("session-1".into()),
        None,
    );

    let result: anyhow::Result<u32> = Ok(42);
    let classified = classify_worktree_result(&mut entry, result);

    assert_eq!(classified.unwrap(), 42);
    assert_eq!(entry.failure_class, None);
}

// Live bug: every candidate backend being simultaneously unavailable
// (quota/cooldown) is transient and self-resolves once availability
// windows expire -- same reasoning as `classify_worktree_result` above.
// `decide_route` used to classify `RouteError::NoEligibleBackend` as
// `human_blocked`, which `controller::is_infra_failure` deliberately
// excludes from retry, permanently orphaning the ticket even after a
// backend recovers. It must classify as `backend_error` instead.
#[test]
fn decide_route_classifies_no_eligible_backend_as_backend_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    // A backend name unknown to `runner::backend_command_name` is always
    // reported unavailable regardless of the host's real PATH, making
    // `RouteError::NoEligibleBackend` deterministic without touching
    // PATH or the on-disk availability state file.
    prof.routing.pm_candidates = Some(vec![crate::config::CandidateConfig {
        backend: "not-a-real-backend".into(),
        ..Default::default()
    }]);
    let cfg = gah_config(RoutingPolicy::default());
    let mut ledger = LedgerEntry::new("test", &prof, "codex", "pm", "target", None, None);

    let req = RouteRequest {
        mode: "pm",
        requested_backend: "auto",
        requested_model: None,
        recommended_backend: None,
        recommended_model: None,
        session_id: None,
        usage_summary: None,
        last_failure_class: None,
    };

    let err = decide_route(&cfg, &prof, req, None, &mut ledger).unwrap_err();
    assert!(err.downcast_ref::<RouteError>().is_some());
    assert_eq!(ledger.failure_class.as_deref(), Some("backend_error"));
}

#[test]
fn preflight_uses_profile_executable_override() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let claude_path = make_fake_bin(&bin_dir, "claude-explicit");
    let git_path = make_fake_bin(&bin_dir, "git");
    let _guard = PathGuard::set(git_path.parent().unwrap());

    let mut profile = profile(tmp.path());
    profile.claude_path = Some(claude_path.display().to_string());

    let result = preflight(&profile, "claude");

    assert!(result.is_ok());
}

#[test]
fn backend_failure_fixture_marks_unavailability() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("availability.json");
    let parsed = mark_backend_unavailable_from_output_at(
        &state,
        "codex",
        Some("local/test"),
        None,
        CODEX_FULL_RESET,
        "/tmp/backend-output.log",
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        parsed.kind,
        crate::quota_parser::FailureKind::QuotaExhausted
    );
    let state = load_state(&state).unwrap();
    assert_eq!(state.records.len(), 1);
    assert_eq!(state.records[0].backend, "codex");
    assert_eq!(state.records[0].model.as_deref(), Some("local/test"));
    assert_eq!(state.records[0].reason, Reason::QuotaExhausted);
    assert!(state.records[0].unavailable_until.is_some());
}

#[test]
fn opencode_internal_rate_limit_marks_the_model_unavailable() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("availability.json");
    let parsed = mark_backend_unavailable_from_output_at(
        &state,
        "opencode",
        Some("opencode/hy3-free"),
        None,
        OPENCODE_HY3_RATE_LIMIT,
        "/tmp/opencode.log",
    )
    .unwrap()
    .unwrap();

    assert_eq!(parsed.kind, crate::quota_parser::FailureKind::RateLimited);
    let decision = availability_for(
        &state,
        "opencode",
        Some("opencode/hy3-free"),
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(!decision.eligible);
    assert_eq!(decision.reason, Some(Reason::RateLimited));
}

#[test]
fn harness_idle_watchdog_marks_backend_outage_not_rate_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("availability.json");
    let parsed = mark_backend_unavailable_from_output_at(
        &state,
        "vibe",
        Some("mistral-medium-3.5"),
        Some("vibe-monthly"),
        "GAH: killed after 600s with no new backend output or worktree progress (stalled, not just slow).",
        "/tmp/vibe.log",
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        parsed.kind,
        crate::quota_parser::FailureKind::BackendStalled
    );
    let decision = availability_for(
        &state,
        "vibe",
        Some("mistral-medium-3.5"),
        Some("vibe-monthly"),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(!decision.eligible);
    assert_eq!(decision.reason, Some(Reason::BackendOutage));
}

#[test]
fn unrecognized_backend_failure_does_not_invent_unavailability() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("availability.json");
    let parsed = mark_backend_unavailable_from_output_at(
        &state,
        "codex",
        Some("local/test"),
        None,
        "plain old crash with no quota language",
        "/tmp/backend-output.log",
    )
    .unwrap();

    assert!(parsed.is_none());
    let decision = availability_for(
        &state,
        "codex",
        Some("local/test"),
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(decision.eligible);
}

#[test]
fn backend_failure_reset_time_resolves_in_local_offset_not_utc() {
    // Live-observed bug: a Codex reset message with a bare "9:01 PM"
    // (no timezone) was resolved as if it were UTC, so on this
    // UTC-5 host a ~3am local reset displayed as "~14h remaining"
    // instead of already having passed. now_with_local_offset() must
    // supply the host's real offset so "9:01 PM" means 9:01 PM local
    // time, not 9:01 PM UTC.
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("availability.json");
    mark_backend_unavailable_from_output_at(
        &state,
        "codex",
        Some("local/test"),
        None,
        CODEX_FULL_RESET,
        "/tmp/backend-output.log",
    )
    .unwrap()
    .unwrap();

    let state = load_state(&state).unwrap();
    let unavailable_until = state.records[0].unavailable_until.as_deref().unwrap();
    let resolved = OffsetDateTime::parse(
        unavailable_until,
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let local_offset_seconds = chrono::Local::now().offset().local_minus_utc();
    let local_offset = time::UtcOffset::from_whole_seconds(local_offset_seconds).unwrap();
    let in_local = resolved.to_offset(local_offset);

    // The fixture says "9:01 PM" -- that must be the LOCAL hour/minute
    // regardless of what the host's offset actually is.
    assert_eq!(in_local.hour(), 21);
    assert_eq!(in_local.minute(), 1);
}
