use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::runner::backend_runner::ObservedSkills;
use crate::runner::process::{spawn_with_idle_watch, write_redacted_task};
use crate::runner::resolve::filtered_backend_args;
use crate::runner::RunResult;

/// Run Hermes non-interactively via `hermes -z` (one-shot mode: send a
/// single prompt, print only the final response text to stdout -- no
/// banner, no spinner, no TUI). `--yolo` bypasses dangerous-command approval
/// prompts and `--accept-hooks` auto-approves any unseen shell hooks
/// declared in the profile's Hermes config, since there is no TTY to
/// prompt on for either. Deliberately does not pass `--worktree`: GAH
/// already runs this command with `current_dir(worktree)` pointed at its
/// own managed worktree, and Hermes's `--worktree` flag would create a
/// second, independent isolated worktree on top of that.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_hermes(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_hermes_with_executable(
        Path::new("hermes"),
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
pub fn run_hermes_with_executable(
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
    cmd.arg("-z")
        .arg(task)
        .arg("--yolo")
        .arg("--accept-hooks")
        .args(filtered_backend_args("hermes", extra_args))
        .current_dir(worktree);
    if let Some(model) = model {
        cmd.args(["-m", model]);
    }
    crate::runner::apply_child_env(&mut cmd, env_vars);

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching hermes; is it installed and on PATH?",
    )?;

    // `-z`/`--oneshot` mode prints ONLY the final response text to stdout
    // (no banner, no spinner, no TUI -- see `hermes --help`), so the log
    // content itself is already the summary; no structured-format parsing
    // to do here unlike codex's JSONL or claude's transcript.
    let output_text = fs::read_to_string(&log_path).unwrap_or_default();
    let final_summary = {
        let trimmed = output_text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary,
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: None,
        agy_version: None,
    })
}

/// Self-report query for #966: ask a Hermes instance what skills it
/// currently has configured, bounded by `timeout` so a hung/unreachable
/// backend never blocks the caller. Hermes exposes enabled skills through
/// `skills list --enabled-only`; any unexpected output, a nonzero exit, or a
/// timeout folds to `Unknown` rather than fabricating an empty inventory.
///
/// `cwd`/`env_vars` target this specific backend instance's isolated
/// state/profile (e.g. an instance-scoped `HOME`), the same way every other
/// Hermes invocation in this file does -- without them the query would run
/// against gah's own ambient environment rather than the instance being
/// asked about.
pub(crate) fn observe_skills_with_executable(
    executable: &Path,
    cwd: Option<&Path>,
    env_vars: &[(String, String)],
    timeout: Duration,
) -> ObservedSkills {
    let mut cmd = Command::new(executable);
    cmd.args(["skills", "list", "--enabled-only"]);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    crate::runner::apply_child_env(&mut cmd, env_vars);
    // Rich truncates cells to terminal width. Pin a wide, colorless output
    // so the table parser receives complete skill IDs from non-interactive
    // subprocesses.
    cmd.env("COLUMNS", "1000")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb");
    match crate::runner::process::run_bounded(cmd, timeout) {
        Some(output) if output.status.success() => {
            parse_skills_list(&output.stdout).unwrap_or(ObservedSkills::Unknown)
        }
        _ => ObservedSkills::Unknown,
    }
}

fn parse_skills_list(stdout: &[u8]) -> Option<ObservedSkills> {
    let text = std::str::from_utf8(stdout).ok()?;
    let mut saw_header = false;
    let mut skills = Vec::new();
    for line in text.lines().filter(|line| {
        let line = line.trim_start();
        line.starts_with('│') || line.starts_with('┃')
    }) {
        let cells: Vec<_> = line.split(['│', '┃']).map(str::trim).collect();
        if cells.len() < 7 {
            continue;
        }
        if cells[1] == "Name" && cells[5] == "Status" {
            saw_header = true;
        } else if cells[5] == "enabled" && !cells[1].is_empty() {
            skills.push(cells[1].to_string());
        }
    }
    saw_header.then_some(ObservedSkills::Skills(skills))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use std::fs;
    // ── run_hermes ───────────────────────────────────────────────────────

    #[test]
    fn run_hermes_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "hermes", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_hermes(
            &f.worktree,
            "hermes task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-hermes"));
        assert!(log.contains("stderr-marker-hermes"));
    }

    #[test]
    fn run_hermes_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "hermes", &f.record_dir, 3);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result =
            run_hermes(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap();

        assert_eq!(result.exit_code, 3);
    }

    #[test]
    fn run_hermes_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "hermes", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_hermes(
            &f.worktree,
            "the hermes task",
            &f.session_dir,
            None,
            &[
                "--custom-flag".to_string(),
                "--skills".to_string(),
                "review".to_string(),
            ],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-z");
        assert!(argv.contains(&"the hermes task".to_string()));
        assert!(argv.contains(&"--yolo".to_string()));
        assert!(argv.contains(&"--accept-hooks".to_string()));
        assert!(argv.contains(&"--custom-flag".to_string()));
        assert!(argv.windows(2).any(|args| args == ["--skills", "review"]));
        assert!(!argv.contains(&"--worktree".to_string()));
    }

    #[test]
    fn run_hermes_binds_the_effective_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "hermes", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_hermes(
            &f.worktree,
            "task",
            &f.session_dir,
            Some("nous-portal/deepseek/deepseek-v4-flash"),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"-m".to_string()));
        assert!(argv.contains(&"nous-portal/deepseek/deepseek-v4-flash".to_string()));
    }

    #[test]
    fn run_hermes_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "hermes", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_hermes(
            &f.worktree,
            "task",
            &f.session_dir,
            Some("gpt-5.4"),
            &[
                "-m".to_string(),
                "stale-model".to_string(),
                "--model=older".to_string(),
                "--yolo".to_string(),
            ],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"-m".to_string()));
        assert!(argv.contains(&"gpt-5.4".to_string()));
        assert!(!argv.contains(&"stale-model".to_string()));
        assert!(!argv.contains(&"--model=older".to_string()));
    }

    #[test]
    fn run_hermes_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "hermes", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "hermes-env-value".to_string()),
        ];

        run_hermes(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=hermes-env-value"));
    }

    #[test]
    fn run_hermes_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err =
            run_hermes(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap_err();

        assert!(err
            .to_string()
            .contains("launching hermes; is it installed"));
    }

    #[test]
    fn run_hermes_kills_process_after_idle_timeout_with_no_new_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "hermes",
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

        let result = run_hermes(&f.worktree, "task", &f.session_dir, None, &[], &envs, 1).unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }

    // ── observe_skills_with_executable (#966) ───────────────────────────

    #[test]
    fn observe_skills_reports_the_self_reported_set() {
        let f = fixture();
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' \
'┏━━━━━━━┳━━━━━━━━━━┳━━━━━━━━┳━━━━━━━┳━━━━━━━━━┓' \
'┃ Name  ┃ Category ┃ Source ┃ Trust ┃ Status  ┃' \
'┡━━━━━━━╇━━━━━━━━━━╇━━━━━━━━╇━━━━━━━╇━━━━━━━━━┩' \
'│ review │          │ local  │ local │ enabled │' \
'│ triage │          │ local  │ local │ enabled │' \
'└───────┴──────────┴────────┴───────┴─────────┘'\n",
            f.record_dir.join("argv.txt").display(),
        );
        make_fake_bin(&f.bin_dir, "hermes", &body);

        let observed = observe_skills_with_executable(
            &f.bin_dir.join("hermes"),
            None,
            &[],
            Duration::from_secs(5),
        );

        assert_eq!(
            observed,
            ObservedSkills::Skills(vec!["review".to_string(), "triage".to_string()])
        );
        assert_eq!(
            recorded_argv(&f.record_dir),
            ["skills", "list", "--enabled-only"]
        );
    }

    #[test]
    fn observe_skills_targets_the_instance_working_directory_and_environment() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "hermes", &f.record_dir, 0);
        let env_vars = vec![("HOME".to_string(), "/instance/isolated/home".to_string())];

        observe_skills_with_executable(
            &f.bin_dir.join("hermes"),
            Some(f.worktree.as_path()),
            &env_vars,
            Duration::from_secs(5),
        );

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("HOME=/instance/isolated/home"));
    }

    #[test]
    fn observe_skills_is_unknown_not_empty_when_the_backend_reports_zero_and_fails() {
        let f = fixture();
        make_fake_bin(&f.bin_dir, "hermes", "#!/bin/sh\nexit 1\n");

        let observed = observe_skills_with_executable(
            &f.bin_dir.join("hermes"),
            None,
            &[],
            Duration::from_secs(5),
        );

        assert_eq!(observed, ObservedSkills::Unknown);
    }

    #[test]
    fn observe_skills_is_unknown_on_malformed_output() {
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "hermes",
            "#!/bin/sh\necho 'not a skills table'\n",
        );

        let observed = observe_skills_with_executable(
            &f.bin_dir.join("hermes"),
            None,
            &[],
            Duration::from_secs(5),
        );

        assert_eq!(observed, ObservedSkills::Unknown);
    }

    #[test]
    fn observe_skills_is_unknown_and_bounded_when_the_backend_hangs() {
        let f = fixture();
        make_fake_bin(&f.bin_dir, "hermes", "#!/bin/sh\nsleep 30\n");

        let started = std::time::Instant::now();
        let observed = observe_skills_with_executable(
            &f.bin_dir.join("hermes"),
            None,
            &[],
            Duration::from_millis(200),
        );
        let elapsed = started.elapsed();

        assert_eq!(observed, ObservedSkills::Unknown);
        assert!(
            elapsed < Duration::from_secs(5),
            "query did not respect the timeout, took {elapsed:?}"
        );
    }
}
