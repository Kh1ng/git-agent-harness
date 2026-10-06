use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::config::Profile;
use crate::ledger::LedgerUsage;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::RunResult;

/// Cursor has no per-profile idle-timeout knob yet; this matches the default
/// every other backend falls back to. A hung `cursor-agent -p` (#863) ends
/// here as the ordinary idle-timeout attempt failure.
pub(crate) const IDLE_TIMEOUT_SECONDS: u64 = 300;

/// The permission flags a Cursor run launches with, derived from what the
/// profile already grants its Codex/Claude runs. `-f` (apply edits and run
/// commands without asking) is passed only when one of those backends is
/// already configured to write without prompting; every other profile gets
/// the read-only `--mode ask`, so Cursor never gains more write authority
/// than the operator has granted elsewhere.
pub(crate) fn permission_args(profile: &Profile) -> Vec<String> {
    if profile_allows_unprompted_writes(profile) {
        vec!["-f".to_string()]
    } else {
        vec!["--mode".to_string(), "ask".to_string()]
    }
}

fn profile_allows_unprompted_writes(profile: &Profile) -> bool {
    let codex = profile.codex_args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--dangerously-bypass-approvals-and-sandbox" | "--yolo" | "--full-auto"
        )
    });
    let claude_mode = |mode: &str| matches!(mode, "bypassPermissions" | "acceptEdits");
    let claude = profile.claude_args.iter().enumerate().any(|(i, arg)| {
        arg == "--dangerously-skip-permissions"
            || arg
                .strip_prefix("--permission-mode=")
                .is_some_and(claude_mode)
            || (arg == "--permission-mode"
                && profile
                    .claude_args
                    .get(i + 1)
                    .is_some_and(|mode| claude_mode(mode)))
    });
    codex || claude
}

/// Run Cursor non-interactively via
/// `cursor-agent -p --output-format json [--model <m>] [-f | --mode ask] <prompt>`.
/// `extra_args` are the permission flags from [`permission_args`]; the
/// single JSON result object Cursor prints on completion carries the final
/// text and, when the CLI reports it, token usage.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

    let mut cmd = Command::new(executable);
    cmd.args(["-p", "--output-format", "json"])
        .current_dir(worktree);
    if let Some(model) = model {
        cmd.args(["--model", model]);
    }
    cmd.args(extra_args).arg(task);
    crate::runner::apply_child_env(&mut cmd, env_vars);

    let (exit_code, duration_secs, resources) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching cursor-agent; is it installed and on PATH?",
    )?;

    let output_text = fs::read_to_string(&log_path).unwrap_or_default();
    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary: parse_output(&output_text).final_text,
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: None,
        agy_version: None,
        resources,
    })
}

#[derive(Debug, Default)]
pub(crate) struct CursorOutput {
    pub(crate) final_text: Option<String>,
    pub(crate) usage: LedgerUsage,
}

/// Parse Cursor's `--output-format json` result object out of the captured
/// log (stdout and stderr share it, so the object is found by shape rather
/// than assumed to be the whole text). Token counters are read only from a
/// `usage` object the CLI actually emitted: an absent object or an absent
/// counter stays `None` (unknown), never zero.
pub(crate) fn parse_output(output: &str) -> CursorOutput {
    let Some(result) = output
        .lines()
        .rev()
        .chain(std::iter::once(output))
        .filter_map(|candidate| serde_json::from_str::<Value>(candidate.trim()).ok())
        .find(|value| value.get("type").and_then(Value::as_str) == Some("result"))
    else {
        return CursorOutput::default();
    };

    let final_text = result
        .get("result")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);

    let counter = |keys: [&str; 2]| {
        let usage = result.get("usage")?;
        keys.iter()
            .find_map(|key| usage.get(*key).and_then(Value::as_u64))
    };
    let input_tokens = counter(["inputTokens", "input_tokens"]);
    let output_tokens = counter(["outputTokens", "output_tokens"]);
    let reasoning_tokens = counter(["reasoningTokens", "reasoning_tokens"]);
    let cache_read_tokens = counter(["cacheReadTokens", "cache_read_tokens"]);
    let cache_write_tokens = counter(["cacheWriteTokens", "cache_write_tokens"]);
    let total_tokens = counter(["totalTokens", "total_tokens"]);
    let observed = [
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens,
    ]
    .iter()
    .any(Option::is_some);

    CursorOutput {
        final_text,
        usage: LedgerUsage {
            usage_source: observed.then(|| "cursor_output_json".to_string()),
            input_tokens,
            output_tokens,
            reasoning_tokens,
            cache_read_tokens,
            cache_write_tokens,
            total_tokens,
            requests_count: observed.then_some(1),
            ..LedgerUsage::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;

    // Documented-shape fixture, not a live capture -- see
    // tests/fixtures/cursor/PROVENANCE.md.
    const RESULT_FIXTURE: &str =
        include_str!("../../../tests/fixtures/cursor/result_documented_shape.json");

    fn run_recorded(model: Option<&str>, extra_args: &[String]) -> Vec<String> {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "cursor-agent", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_with_executable(
            Path::new("cursor-agent"),
            &f.worktree,
            "the cursor task",
            &f.session_dir,
            model,
            extra_args,
            &envs,
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        recorded_argv(&f.record_dir)
    }

    // ── golden argv ──────────────────────────────────────────────────────

    #[test]
    fn default_argv_is_print_mode_json_then_prompt() {
        assert_eq!(
            run_recorded(None, &[]),
            ["-p", "--output-format", "json", "the cursor task"]
        );
    }

    #[test]
    fn model_override_argv_binds_the_route_model() {
        assert_eq!(
            run_recorded(Some("sonnet-4.5"), &[]),
            [
                "-p",
                "--output-format",
                "json",
                "--model",
                "sonnet-4.5",
                "the cursor task"
            ]
        );
    }

    #[test]
    fn write_trust_argv_passes_force() {
        let mut profile = test_profile();
        profile.codex_args = vec!["--dangerously-bypass-approvals-and-sandbox".to_string()];

        assert_eq!(
            run_recorded(None, &permission_args(&profile)),
            ["-p", "--output-format", "json", "-f", "the cursor task"]
        );
    }

    #[test]
    fn read_only_argv_passes_ask_mode_and_never_force() {
        let argv = run_recorded(None, &permission_args(&test_profile()));

        assert_eq!(
            argv,
            [
                "-p",
                "--output-format",
                "json",
                "--mode",
                "ask",
                "the cursor task"
            ]
        );
        assert!(!argv.contains(&"-f".to_string()));
    }

    #[test]
    fn force_follows_only_flags_that_already_let_codex_or_claude_write_unprompted() {
        let force = vec!["-f".to_string()];
        let with = |codex: &[&str], claude: &[&str]| {
            let mut profile = test_profile();
            profile.codex_args = codex.iter().map(|arg| arg.to_string()).collect();
            profile.claude_args = claude.iter().map(|arg| arg.to_string()).collect();
            permission_args(&profile)
        };

        assert_eq!(with(&["--full-auto"], &[]), force);
        assert_eq!(with(&[], &["--dangerously-skip-permissions"]), force);
        assert_eq!(with(&[], &["--permission-mode", "acceptEdits"]), force);
        assert_eq!(with(&[], &["--permission-mode=bypassPermissions"]), force);
        assert_eq!(
            with(&["--trace"], &["--permission-mode", "plan"]),
            ["--mode", "ask"]
        );
        assert_eq!(
            with(&[], &["--allowedTools", "Edit,Write"]),
            ["--mode", "ask"]
        );
    }

    // ── process behavior ─────────────────────────────────────────────────

    #[test]
    fn run_cursor_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_with_executable(
            Path::new("cursor-agent"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("launching cursor-agent; is it installed"));
    }

    #[test]
    fn run_cursor_hang_ends_as_the_idle_timeout_failure() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "cursor-agent",
            "#!/bin/sh\necho 'step1'\nsleep 10\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_with_executable(
            Path::new("cursor-agent"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            3,
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 3s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_cursor_reports_the_json_result_text_as_the_final_summary() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let fixture_path = f.record_dir.join("result.json");
        fs::write(&fixture_path, RESULT_FIXTURE).unwrap();
        make_fake_bin(
            &f.bin_dir,
            "cursor-agent",
            &format!(
                "#!/bin/sh\necho 'warming up' >&2\n/bin/cat '{}'\n",
                fixture_path.display()
            ),
        );

        let result = run_with_executable(
            &f.bin_dir.join("cursor-agent"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &[],
            300,
        )
        .unwrap();

        assert_eq!(
            result.final_summary.as_deref(),
            Some("The repository builds with `cargo build`.")
        );
    }

    // ── result parsing ───────────────────────────────────────────────────

    #[test]
    fn fixture_result_yields_final_text_and_unknown_usage() {
        let parsed = parse_output(RESULT_FIXTURE);

        assert_eq!(
            parsed.final_text.as_deref(),
            Some("The repository builds with `cargo build`.")
        );
        // The documented result object carries no usage: every counter must
        // stay unknown rather than become a recorded zero.
        assert_eq!(parsed.usage.usage_source, None);
        assert_eq!(parsed.usage.input_tokens, None);
        assert_eq!(parsed.usage.output_tokens, None);
        assert_eq!(parsed.usage.total_tokens, None);
        assert_eq!(parsed.usage.requests_count, None);
    }

    #[test]
    fn reported_usage_is_recorded_and_absent_counters_stay_unknown() {
        let parsed = parse_output(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done","usage":{"inputTokens":1200,"outputTokens":0,"cacheReadTokens":300}}"#,
        );

        assert_eq!(
            parsed.usage.usage_source.as_deref(),
            Some("cursor_output_json")
        );
        assert_eq!(parsed.usage.input_tokens, Some(1200));
        // A reported zero is a real observation, distinct from absent.
        assert_eq!(parsed.usage.output_tokens, Some(0));
        assert_eq!(parsed.usage.cache_read_tokens, Some(300));
        assert_eq!(parsed.usage.cache_write_tokens, None);
        assert_eq!(parsed.usage.reasoning_tokens, None);
        assert_eq!(parsed.usage.total_tokens, None);
    }

    #[test]
    fn output_without_a_result_object_yields_nothing() {
        let parsed = parse_output("warming up\n{\"type\":\"system\"}\nnot json\n");

        assert_eq!(parsed.final_text, None);
        assert_eq!(parsed.usage.usage_source, None);
    }

    // ── auth failure ─────────────────────────────────────────────────────

    #[test]
    fn authentication_required_classifies_as_an_authentication_error() {
        let parsed = crate::quota_parser::parse(
            "cursor",
            "Error: Authentication required. Please run 'cursor-agent login' first.",
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();

        assert_eq!(
            parsed.kind,
            crate::quota_parser::FailureKind::AuthenticationError
        );
        assert_eq!(parsed.backend, "cursor");
    }
}
