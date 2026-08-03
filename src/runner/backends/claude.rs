use anyhow::Result;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::claude_monitor::find_claude_transcript;
use crate::runner::output;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::resolve::filtered_backend_args;
use crate::runner::RunResult;

/// Run Claude CLI non-interactively via `claude -p`.
/// extra_args come from profile.claude_args (e.g. `--allowedTools Edit,Write,Bash`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_claude(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_claude_with_executable(
        Path::new("claude"),
        worktree,
        task,
        session_dir,
        None,
        extra_args,
        env_vars,
        idle_timeout_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_claude_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    effective_model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

    // Issue #153: pin a stable session id so we can locate the exact
    // transcript `.jsonl` Claude Code writes afterwards (the source of real
    // per-attempt token/cost usage, rather than scraping stdout).
    let session_id = uuid::Uuid::new_v4().to_string();
    // Find the HOME this invocation will run under (a per-attempt HOME is
    // injected via env_vars; fall back to the ambient HOME).
    let home = env_vars
        .iter()
        .find_map(|(k, v)| (k == "HOME").then_some(PathBuf::from(v)))
        .or_else(|| env::var("HOME").ok().map(PathBuf::from));

    let mut cmd = Command::new(executable);
    cmd.args([
        "-p",
        task,
        "--output-format",
        "text",
        "--verbose",
        "--session-id",
        &session_id,
    ])
    .current_dir(worktree);
    if let Some(model) = effective_model {
        cmd.args(["--model", model]);
    }
    cmd.args(filtered_backend_args("claude", extra_args));
    crate::runner::apply_child_env(&mut cmd, env_vars);

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching claude; is it installed and on PATH?",
    )?;

    // Locate the transcript for the pinned session id so per-attempt usage
    // parsing can consume it.
    let transcript_path = home
        .as_ref()
        .and_then(|h| find_claude_transcript(h, worktree, &session_id))
        .map(|p| p.to_string_lossy().into_owned());
    let final_summary = transcript_path
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| output::extract_claude_transcript_summary(&text));

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary,
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path,
        agy_version: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use std::fs;
    // ── run_claude ───────────────────────────────────────────────────────

    #[test]
    fn run_claude_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result =
            run_claude(&f.worktree, "claude task", &f.session_dir, &[], &envs, 300).unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-claude"));
        assert!(log.contains("stderr-marker-claude"));
    }

    #[test]
    fn run_claude_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 1);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap();

        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn run_claude_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_claude(
            &f.worktree,
            "the claude task",
            &f.session_dir,
            &["--allowedTools".to_string(), "Edit,Bash".to_string()],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"the claude task".to_string()));
        assert!(argv.contains(&"--allowedTools".to_string()));
        assert!(argv.contains(&"Edit,Bash".to_string()));
    }

    #[test]
    fn run_claude_binds_the_effective_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_claude_with_executable(
            &f.bin_dir.join("claude"),
            &f.worktree,
            "the claude task",
            &f.session_dir,
            Some("haiku"),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
    }

    #[test]
    fn run_claude_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_claude_with_executable(
            &f.bin_dir.join("claude"),
            &f.worktree,
            "the claude task",
            &f.session_dir,
            Some("haiku"),
            &[
                "--allowedTools".to_string(),
                "Edit".to_string(),
                "--model".to_string(),
                "opus".to_string(),
                "--model=sonnet".to_string(),
            ],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
        assert!(argv.contains(&"--allowedTools".to_string()));
        assert!(!argv.contains(&"opus".to_string()));
        assert!(!argv.contains(&"sonnet".to_string()));
        assert!(!argv.contains(&"--model=sonnet".to_string()));
    }

    #[test]
    fn run_claude_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "claude-env-value".to_string()),
        ];

        run_claude(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=claude-env-value"));
    }

    #[test]
    fn run_claude_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap_err();

        assert!(err
            .to_string()
            .contains("launching claude; is it installed"));
    }

    #[test]
    fn run_claude_kills_process_after_idle_timeout_with_no_new_output() {
        // claude used a plain blocking cmd.status() with zero supervision,
        // same class of bug as issues #87/#170. Pins the shared
        // spawn_with_idle_watch fix for this backend.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
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

        let result = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs, 1).unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }
}
