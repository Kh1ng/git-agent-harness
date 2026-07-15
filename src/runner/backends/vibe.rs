use anyhow::Result;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::runner::output;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::RunResult;

/// Run Mistral's Vibe CLI non-interactively via `vibe -p`.
/// Used for both worker/fix and review execution.
/// extra_args come from profile.vibe_args (e.g. `--max-turns 40 --max-price 2`).
/// No --model flag exists on this CLI; model selection is config/env-var
/// driven on vibe's own side (VIBE_ACTIVE_MODEL / ~/.vibe/config.toml),
/// so GAH binds the effective route model through `VIBE_ACTIVE_MODEL`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_vibe(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_vibe_with_executable(
        Path::new("vibe"),
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
pub fn run_vibe_with_executable(
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
    let started_at = std::time::SystemTime::now();
    // Vibe can retain old session directories for the same worktree. Capture
    // them before launch so a reused or concurrently-updated metadata file is
    // never misreported as this attempt's token consumption.
    let sessions_before = snapshot_vibe_session_metadata_paths(env_vars);

    let mut cmd = Command::new(executable);
    // --trust: automation-only, not persisted to trusted_folders.toml --
    // skips the interactive trust prompt without touching global config.
    // --auto-approve: same automation need as agy's --dangerously-skip-permissions.
    // Vibe's durable session metadata supplies the authoritative token/model
    // totals. Keep the established text output mode for the runner's idle
    // watcher and backend summary handling.
    cmd.args(["-p", task, "--trust", "--auto-approve"])
        .args(extra_args)
        .current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    // Vibe has no per-invocation --model flag. Its documented environment
    // selector must receive the route's effective model so the actual model
    // and the ledger attribution cannot diverge. Set this after env_file
    // variables so the route is authoritative for this attempt.
    if let Some(model) = effective_model {
        cmd.env("VIBE_ACTIVE_MODEL", model);
    }

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching vibe; is it installed and on PATH?",
    )?;

    let metadata_path =
        find_vibe_session_metadata(env_vars, worktree, started_at, &sessions_before);
    let final_summary = metadata_path
        .as_deref()
        .and_then(|path| Path::new(path).parent())
        .map(|directory| directory.join("messages.jsonl"))
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| output::extract_vibe_messages_summary(&text));

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary,
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: metadata_path,
        agy_version: None,
    })
}

fn vibe_sessions_dir(env_vars: &[(String, String)]) -> Option<PathBuf> {
    let home = env_vars
        .iter()
        .find(|(key, _)| key == "VIBE_HOME")
        .map(|(_, value)| PathBuf::from(value))
        .or_else(|| {
            env_vars
                .iter()
                .find(|(key, _)| key == "HOME")
                .map(|(_, value)| PathBuf::from(value).join(".vibe"))
        })
        .or_else(|| env::var_os("VIBE_HOME").map(PathBuf::from))
        .or_else(|| env::var_os("HOME").map(|value| PathBuf::from(value).join(".vibe")))?;
    Some(home.join("logs/session"))
}

pub(crate) fn snapshot_vibe_session_metadata_paths(
    env_vars: &[(String, String)],
) -> HashSet<PathBuf> {
    let Some(sessions) = vibe_sessions_dir(env_vars) else {
        return HashSet::new();
    };
    fs::read_dir(sessions)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("meta.json"))
        .filter(|path| path.is_file())
        .collect()
}

/// Locate a *new* Vibe session created by this invocation. Merely matching a
/// worktree and recent mtime is insufficient: Vibe can update a pre-existing
/// session whose counters are cumulative. If no new metadata file appears,
/// usage is deliberately left unknown rather than attributed incorrectly.
pub(crate) fn find_vibe_session_metadata(
    env_vars: &[(String, String)],
    worktree: &Path,
    started_at: std::time::SystemTime,
    sessions_before: &HashSet<PathBuf>,
) -> Option<String> {
    let sessions = vibe_sessions_dir(env_vars)?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let worktree_string = worktree.to_string_lossy().into_owned();
    for entry in fs::read_dir(sessions).ok()?.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let metadata_path = dir.join("meta.json");
        if sessions_before.contains(&metadata_path) {
            continue;
        }
        let Ok(modified) = fs::metadata(&metadata_path).and_then(|metadata| metadata.modified())
        else {
            continue;
        };
        if modified < started_at {
            continue;
        }
        let Ok(metadata) = fs::read_to_string(&metadata_path) else {
            continue;
        };
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&metadata) else {
            continue;
        };
        let cwd = root
            .get("environment")
            .and_then(|environment| environment.get("working_directory"))
            .and_then(|value| value.as_str());
        if cwd != Some(worktree_string.as_str()) {
            continue;
        }
        if best
            .as_ref()
            .map(|(time, _)| modified > *time)
            .unwrap_or(true)
        {
            best = Some((modified, metadata_path));
        }
    }
    best.map(|(_, path)| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn run_vibe_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_vibe(&f.worktree, "vibe task", &f.session_dir, &[], &envs, 300).unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-vibe"));
        assert!(log.contains("stderr-marker-vibe"));
    }

    #[test]
    fn vibe_usage_metadata_ignores_preexisting_cumulative_session() {
        let f = fixture();
        let vibe_home = f._tmp.path().join("vibe-home");
        let sessions = vibe_home.join("logs/session");
        let old = sessions.join("old-session");
        fs::create_dir_all(&old).unwrap();
        fs::write(
            old.join("meta.json"),
            format!(
                r#"{{"environment":{{"working_directory":"{}"}},"stats":{{"session_total_llm_tokens":999999}}}}"#,
                f.worktree.display()
            ),
        )
        .unwrap();
        let envs = vec![("VIBE_HOME".to_string(), vibe_home.display().to_string())];
        let sessions_before = snapshot_vibe_session_metadata_paths(&envs);
        let started_at = std::time::SystemTime::now() - Duration::from_secs(1);

        let current = sessions.join("this-attempt");
        fs::create_dir_all(&current).unwrap();
        fs::write(
            current.join("meta.json"),
            format!(
                r#"{{"environment":{{"working_directory":"{}"}},"stats":{{"session_total_llm_tokens":1200}}}}"#,
                f.worktree.display()
            ),
        )
        .unwrap();

        let selected =
            find_vibe_session_metadata(&envs, &f.worktree, started_at, &sessions_before).unwrap();
        assert_eq!(PathBuf::from(selected), current.join("meta.json"));
        let usage = crate::usage::parse_vibe_session_metadata(
            &fs::read_to_string(current.join("meta.json")).unwrap(),
        );
        assert_eq!(usage.total_tokens, Some(1200));

        let after = snapshot_vibe_session_metadata_paths(&envs);
        assert!(find_vibe_session_metadata(&envs, &f.worktree, started_at, &after).is_none());
    }

    #[test]
    fn run_vibe_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 1);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap();

        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn run_vibe_core_argv_always_includes_trust_and_auto_approve() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_vibe(
            &f.worktree,
            "the vibe task",
            &f.session_dir,
            &["--max-turns".to_string(), "40".to_string()],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"the vibe task".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        assert!(argv.contains(&"--max-turns".to_string()));
        assert!(argv.contains(&"40".to_string()));
    }

    #[test]
    fn run_vibe_binds_effective_model_through_environment() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("VIBE_ACTIVE_MODEL".to_string(), "wrong-default".to_string()),
        ];

        run_vibe_with_executable(
            &f.bin_dir.join("vibe"),
            &f.worktree,
            "the vibe task",
            &f.session_dir,
            Some("devstral-small"),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("VIBE_ACTIVE_MODEL=devstral-small"));
        assert!(!env.contains("VIBE_ACTIVE_MODEL=wrong-default"));
    }

    #[test]
    fn run_vibe_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "vibe-env-value".to_string()),
        ];

        run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=vibe-env-value"));
    }

    #[test]
    fn run_vibe_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap_err();

        assert!(err.to_string().contains("launching vibe; is it installed"));
    }

    #[test]
    fn run_vibe_kills_process_after_idle_timeout_with_no_new_output() {
        // Live-observed (issue #154 dispatch, TICKET-154): a vibe attempt
        // hung for 15+ minutes with zero output and was only stopped by an
        // external watchdog script, not by gah itself -- same class of bug
        // as issues #87/#170. Pins the shared spawn_with_idle_watch fix.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "vibe",
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

        let result = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 1).unwrap();

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
    fn run_vibe_allows_silent_backend_that_keeps_changing_worktree() {
        // Subscription CLIs can make tool calls without printing progress.
        // Repeated tracked-file edits must keep an otherwise silent process
        // alive, while the adjacent test proves a truly idle process dies.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        make_fake_bin(
            &f.bin_dir,
            "vibe",
            "#!/bin/sh\nsleep 1\nprintf 'first\\n' > progress.txt\nsleep 1\nprintf 'second\\n' > progress.txt\nsleep 1\nexit 0\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 2).unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(
            result.duration_secs >= 3.0,
            "ran only {}s",
            result.duration_secs
        );
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(!log.contains("GAH: killed"), "got log: {log}");
        assert_eq!(
            fs::read_to_string(f.worktree.join("progress.txt")).unwrap(),
            "second\n"
        );
    }
}
