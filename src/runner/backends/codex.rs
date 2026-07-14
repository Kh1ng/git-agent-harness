use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::runner::output;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::resolve::{codex_model_args, filtered_codex_args};
use crate::runner::RunResult;

/// Run Codex non-interactively via `codex exec`.
/// extra_args come from profile.codex_args, but stale model flags are
/// stripped so the resolved route controls the launched model.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_codex(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_codex_with_executable(
        Path::new("codex"),
        worktree,
        task,
        session_dir,
        model,
        extra_args,
        env_vars,
        idle_timeout_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_codex_with_executable(
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
    // Issue #152: --json produces structured JSONL output for programmatic
    // usage extraction in parse_codex_exec_json (usage.rs).
    cmd.arg("exec")
        .arg("--json")
        .arg(task)
        .args(filtered_codex_args(extra_args))
        .args(codex_model_args(model))
        .current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching codex; is it installed and on PATH?",
    )?;

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary: fs::read_to_string(&log_path)
            .ok()
            .and_then(|text| output::extract_codex_jsonl_summary(&text)),
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: None,
        agy_version: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use std::fs;
    // ── run_codex ────────────────────────────────────────────────────────

    #[test]
    fn run_codex_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_codex(
            &f.worktree,
            "codex task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
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

        let result = run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap();

        assert_eq!(result.exit_code, 7);
    }

    #[test]
    fn run_codex_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_codex(
            &f.worktree,
            "the codex task",
            &f.session_dir,
            None,
            &["-c".to_string(), "model=gpt".to_string()],
            &envs,
            300,
        )
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

        run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=codex-env-value"));
    }

    #[test]
    fn run_codex_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err =
            run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap_err();

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

        let result = run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs, 1).unwrap();

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

        run_codex(
            &f.worktree,
            "task",
            &f.session_dir,
            Some("gpt-5.4"),
            &[
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "-m".to_string(),
                "legacy-mini".to_string(),
                "--model=older".to_string(),
                "--trace".to_string(),
            ],
            &envs,
            300,
        )
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
