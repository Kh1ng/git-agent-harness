use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::runner::output;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::{LlmConfig, RunResult};

/// Load LLM config from an OpenHands named profile (~/.openhands/profiles/<name>.json).
pub fn load_oh_profile(name: &str) -> Result<LlmConfig> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let path = PathBuf::from(format!("{}/.openhands/profiles/{}.json", home, name));
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "openhands profile '{}' not found at {}",
            name,
            path.display()
        )
    })?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing openhands profile {}", path.display()))?;
    Ok(LlmConfig {
        base_url: v["base_url"].as_str().unwrap_or("").to_string(),
        api_key: v["api_key"].as_str().unwrap_or("").to_string(),
        model: v["model"].as_str().unwrap_or("").to_string(),
    })
}

/// List available OpenHands profiles by name (without .json extension).
#[allow(dead_code)]
pub fn list_oh_profiles() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let dir = PathBuf::from(format!("{}/.openhands/profiles", home));
    let Ok(entries) = fs::read_dir(&dir) else {
        return vec![];
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".json").map(|s| s.to_string())
        })
        .collect();
    names.sort();
    names
}

/// Run OpenHands in headless mode. LLM config is injected via --override-with-envs.
/// extra_args come from profile.openhands_args in config (e.g. plugin/skill flags).
///
/// Issue #87: kills the process once its log has gone quiet for
/// `idle_timeout_seconds` -- OpenHands has no timeout of its own, and a
/// hung/looping run previously blocked `gah dispatch` (and anything built
/// on it, like `gah loop --once`) indefinitely. Mirrors
/// `run_opencode_with_executable`'s idle-watch loop exactly.
pub fn run_openhands(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    llm: &LlmConfig,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_openhands_with_executable(
        Path::new("openhands"),
        worktree,
        task,
        session_dir,
        llm,
        extra_args,
        env_vars,
        idle_timeout_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_openhands_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    llm: &LlmConfig,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

    let mut cmd = Command::new(executable);
    cmd.args([
        "--headless",
        "--json",
        "-t",
        task,
        "--exit-without-confirmation",
        "--always-approve",
        "--override-with-envs",
    ])
    .args(extra_args)
    .env("OPENHANDS_SUPPRESS_BANNER", "1")
    .env("LLM_BASE_URL", &llm.base_url)
    .env("LLM_API_KEY", &llm.api_key)
    .env("LLM_MODEL", &llm.model)
    .current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching openhands; is it installed and on PATH?",
    )?;

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary: fs::read_to_string(&log_path)
            .ok()
            .and_then(|text| output::extract_openhands_jsonl_summary(&text)),
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
    // ── run_openhands ────────────────────────────────────────────────────

    #[test]
    fn run_openhands_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_openhands(
            &f.worktree,
            "my task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("stdout-marker-openhands"),
            "log missing stdout: {log}"
        );
        assert!(
            log.contains("stderr-marker-openhands"),
            "log missing stderr: {log}"
        );
        let task = fs::read_to_string(f.session_dir.join("task.md")).unwrap();
        assert_eq!(task, "my task");
    }

    #[test]
    fn backend_output_and_task_artifacts_are_redacted_before_persisting() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "openhands",
            "#!/bin/sh\necho 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz'\necho 'glpat-abcdefghijklmnopqrstuv' >&2\n",
        );
        let envs = vec![("PATH".to_string(), f.bin_dir.to_string_lossy().into_owned())];

        let result = run_openhands(
            &f.worktree,
            "task includes ghp_abcdefghijklmnopqrstuvwxyz",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let log = fs::read_to_string(result.log_path).unwrap();
        let task = fs::read_to_string(f.session_dir.join("task.md")).unwrap();
        for raw in [
            "abcdefghijklmnopqrstuvwxyz",
            "glpat-abcdefghijklmnopqrstuv",
            "ghp_abcdefghijklmnopqrstuvwxyz",
        ] {
            assert!(!log.contains(raw), "raw secret persisted in log: {raw}");
            assert!(!task.contains(raw), "raw secret persisted in task: {raw}");
        }
        assert!(log.contains("[REDACTED:TOKEN]"));
        assert!(task.contains("[REDACTED:GITHUB_TOKEN]"));
    }

    #[test]
    fn run_openhands_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 3);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_openhands(
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 3);
    }

    #[test]
    fn run_openhands_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_openhands(
            &f.worktree,
            "the task text",
            &f.session_dir,
            &test_llm(),
            &["--extra-flag".to_string()],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--headless".to_string()));
        assert!(argv.contains(&"-t".to_string()));
        assert!(argv.contains(&"the task text".to_string()));
        assert!(argv.contains(&"--override-with-envs".to_string()));
        assert!(argv.contains(&"--extra-flag".to_string()));
    }

    #[test]
    fn run_openhands_propagates_llm_config_via_env() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let llm = LlmConfig {
            base_url: "http://distinct-base.test".into(),
            api_key: "distinct-api-key".into(),
            model: "distinct-model-name".into(),
        };

        run_openhands(&f.worktree, "task", &f.session_dir, &llm, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("LLM_BASE_URL=http://distinct-base.test"));
        assert!(env.contains("LLM_API_KEY=distinct-api-key"));
        assert!(env.contains("LLM_MODEL=distinct-model-name"));
    }

    #[test]
    fn run_openhands_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "env-file-value".to_string()),
        ];

        run_openhands(
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=env-file-value"));
    }

    #[test]
    fn run_openhands_missing_binary_produces_useful_error() {
        let f = fixture(); // bin_dir stays empty — no openhands on PATH
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_openhands(
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("launching openhands; is it installed"));
    }

    #[test]
    fn run_openhands_kills_process_after_idle_timeout_with_no_new_output() {
        // Issue #87: a live openhands dispatch ran for 2h20m+ with no
        // supervision at all -- run_openhands previously used a plain
        // blocking `cmd.status()`. This pins the fix: it must be killed
        // once output has genuinely stopped, not allowed to run forever.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "openhands",
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

        let result = run_openhands(
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            1, // idle timeout: 1s of silence is stalled
        )
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
}
