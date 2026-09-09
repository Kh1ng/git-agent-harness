use anyhow::Result;
use std::fs;
use std::process::Command;

use crate::backend_kind::BackendKind;
use crate::runner::output;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::resolve::{codex_model_args, filtered_codex_args};
use crate::runner::{BackendRunner, RunContext, RunResult};

/// Runs a single Codex task, collecting its output, summary, and transcript.
/// Route models override stale model flags in the supplied profile arguments.
pub struct CodexRunner;

impl BackendRunner for CodexRunner {
    fn kind(&self) -> BackendKind {
        BackendKind::Codex
    }

    fn run(&self, ctx: &RunContext) -> Result<RunResult> {
        let log_path = ctx.session_dir.join("backend-output.log");
        write_redacted_task(ctx.session_dir, ctx.task)?;

        let mut cmd = Command::new(ctx.executable);
        // Issue #152: --json produces structured JSONL output for programmatic
        // usage extraction in parse_codex_exec_json (usage.rs).
        cmd.arg("exec")
            .arg("--json")
            .arg(ctx.task)
            .args(filtered_codex_args(ctx.extra_args))
            .args(codex_model_args(ctx.model))
            .current_dir(ctx.worktree);
        crate::runner::apply_child_env(&mut cmd, ctx.env_vars);

        let (exit_code, duration_secs) = spawn_with_idle_watch(
            cmd,
            &log_path,
            ctx.worktree,
            ctx.idle_timeout_seconds,
            "launching codex; is it installed and on PATH?",
        )?;

        let output_text = fs::read_to_string(&log_path).unwrap_or_default();
        let transcript_path =
            crate::runner::review_usage::find_codex_transcript(ctx.env_vars, &output_text)
                .map(|path| path.to_string_lossy().into_owned());
        Ok(RunResult {
            exit_code,
            duration_secs,
            log_path: log_path.to_string_lossy().into_owned(),
            final_summary: output::extract_codex_jsonl_summary(&output_text),
            agy_cli_log_delta: None,
            internal_log_delta: None,
            internal_log_path: None,
            transcript_path,
            agy_version: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn run_codex_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = CodexRunner
            .run(&RunContext {
                executable: Path::new("codex"),
                worktree: &f.worktree,
                task: "codex task",
                session_dir: &f.session_dir,
                model: None,
                extra_args: &[],
                env_vars: &envs,
                idle_timeout_seconds: 300,
                llm: None,
                print_timeout_seconds: None,
            })
            .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-codex"));
        assert!(log.contains("stderr-marker-codex"));
    }

    #[test]
    fn run_codex_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 7);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = CodexRunner
            .run(&RunContext {
                executable: Path::new("codex"),
                worktree: &f.worktree,
                task: "task",
                session_dir: &f.session_dir,
                model: None,
                extra_args: &[],
                env_vars: &envs,
                idle_timeout_seconds: 300,
                llm: None,
                print_timeout_seconds: None,
            })
            .unwrap();

        assert_eq!(result.exit_code, 7);
    }

    #[test]
    fn run_codex_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        CodexRunner
            .run(&RunContext {
                executable: Path::new("codex"),
                worktree: &f.worktree,
                task: "the codex task",
                session_dir: &f.session_dir,
                model: None,
                extra_args: &["-c".to_string(), "model=gpt".to_string()],
                env_vars: &envs,
                idle_timeout_seconds: 300,
                llm: None,
                print_timeout_seconds: None,
            })
            .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--json");
        assert!(argv.contains(&"the codex task".to_string()));
        assert!(argv.contains(&"-c".to_string()));
        assert!(argv.contains(&"model=gpt".to_string()));
    }

    #[test]
    fn run_codex_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "codex-env-value".to_string()),
        ];

        CodexRunner
            .run(&RunContext {
                executable: Path::new("codex"),
                worktree: &f.worktree,
                task: "task",
                session_dir: &f.session_dir,
                model: None,
                extra_args: &[],
                env_vars: &envs,
                idle_timeout_seconds: 300,
                llm: None,
                print_timeout_seconds: None,
            })
            .unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=codex-env-value"));
    }

    #[test]
    fn run_codex_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = CodexRunner
            .run(&RunContext {
                executable: Path::new("codex"),
                worktree: &f.worktree,
                task: "task",
                session_dir: &f.session_dir,
                model: None,
                extra_args: &[],
                env_vars: &envs,
                idle_timeout_seconds: 300,
                llm: None,
                print_timeout_seconds: None,
            })
            .unwrap_err();

        assert!(err.to_string().contains("launching codex; is it installed"));
    }

    #[test]
    fn run_codex_kills_process_after_idle_timeout_with_no_new_output() {
        // codex used a plain blocking cmd.status() with zero supervision,
        // same class of bug as issues #87/#170. Pins the shared
        // spawn_with_idle_watch fix for this backend.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "codex",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = CodexRunner
            .run(&RunContext {
                executable: Path::new("codex"),
                worktree: &f.worktree,
                task: "task",
                session_dir: &f.session_dir,
                model: None,
                extra_args: &[],
                env_vars: &envs,
                idle_timeout_seconds: 1,
                llm: None,
                print_timeout_seconds: None,
            })
            .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_codex_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        CodexRunner
            .run(&RunContext {
                executable: Path::new("codex"),
                worktree: &f.worktree,
                task: "task",
                session_dir: &f.session_dir,
                model: Some("gpt-5.4"),
                extra_args: &[
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                    "-m".to_string(),
                    "legacy-mini".to_string(),
                    "--model=older".to_string(),
                    "--trace".to_string(),
                ],
                env_vars: &envs,
                idle_timeout_seconds: 300,
                llm: None,
                print_timeout_seconds: None,
            })
            .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--json");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(argv.contains(&"--trace".to_string()));
        assert!(argv.contains(&"-m".to_string()));
        assert!(argv.contains(&"gpt-5.4".to_string()));
        assert!(!argv.contains(&"legacy-mini".to_string()));
        assert!(!argv.contains(&"--model".to_string()));
        assert!(!argv.contains(&"--model=older".to_string()));
    }
}
