use crate::config::Profile;
use crate::runner::backends::agy::agy_empty_output_diagnosis;
use crate::runner::process::{
    copy_stream_to_file, kill_process_group, prepare_process_group,
    process_group_activity_advanced, process_group_activity_snapshot, shutdown_requested,
    worktree_progress_snapshot, write_redacted_task,
};
use crate::runner::resolve::{
    codex_model_args, filtered_codex_args, resolve_backend_executable, ExecutableResolution,
};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewProcessOutcome {
    Success,
    ExecutableUnavailable,
    SpawnFailure,
    NonZeroExit(i32),
    SignalTermination(i32),
    /// GAH could not prove that every backend descendant was terminated.
    /// This is a harness containment failure, not a backend or timeout error.
    CleanupFailure(String),
    /// The reviewer went silent for the configured idle budget (no stdout/stderr
    /// activity, review artifact updates, or backend-specific structured
    /// progress). This is a stall, classified separately from `HardTimeout`.
    IdleTimeout,
    /// The reviewer was still making progress but exceeded the optional hard
    /// wall-clock safety ceiling. Distinct from `IdleTimeout` so a healthy
    /// reviewer that merely outran the old flat clock is *not* treated as a
    /// backend failure that warrants retry/escalation (issue #540).
    HardTimeout,
}

#[derive(Debug)]
pub struct ReviewRunResult {
    pub outcome: ReviewProcessOutcome,
    pub duration_secs: f64,
    pub stdout: String,
    pub stderr: String,
    /// Idle supervision budget that was configured (seconds).
    pub idle_timeout_seconds: u64,
    /// Optional hard wall-clock ceiling that was configured (seconds), if any.
    pub hard_timeout_seconds: Option<u64>,
    /// Elapsed seconds since dispatch start at the moment of last observed
    /// progress (stdout/stderr activity, worktree update, or child process
    /// CPU/I/O). `None` when the reviewer never produced progress.
    pub last_progress_secs: Option<f64>,
}

pub fn run_review_backend(
    profile: &Profile,
    backend: &str,
    worktree: &Path,
    prompt: &str,
    session_dir: &Path,
    effective_model: Option<&str>,
    env_vars: &[(String, String)],
) -> ReviewRunResult {
    let start = Instant::now();
    let hard_timeout_seconds = profile
        .review_hard_timeout_seconds
        .map(|seconds| seconds.max(1));
    let stdout_path = session_dir.join("review-stdout.log");
    let stderr_path = session_dir.join("review-stderr.log");
    let _ = write_redacted_task(session_dir, prompt);

    let executable = match resolve_backend_executable(profile, backend) {
        ExecutableResolution::Found(path) => path,
        ExecutableResolution::MissingExplicitPath(_) | ExecutableResolution::MissingFromPath(_) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::ExecutableUnavailable,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: String::new(),
                idle_timeout_seconds: profile.review_timeout_seconds(),
                hard_timeout_seconds,
                last_progress_secs: None,
            };
        }
        ExecutableResolution::UnknownBackend(_) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: format!("unsupported review backend: {backend}"),
                idle_timeout_seconds: profile.review_timeout_seconds(),
                hard_timeout_seconds,
                last_progress_secs: None,
            };
        }
    };

    if let Err(err) = fs::File::create(&stdout_path) {
        return ReviewRunResult {
            outcome: ReviewProcessOutcome::SpawnFailure,
            duration_secs: start.elapsed().as_secs_f64(),
            stdout: String::new(),
            stderr: format!("creating {}: {err}", stdout_path.display()),
            idle_timeout_seconds: profile.review_timeout_seconds(),
            hard_timeout_seconds,
            last_progress_secs: None,
        };
    }
    if let Err(err) = fs::File::create(&stderr_path) {
        return ReviewRunResult {
            outcome: ReviewProcessOutcome::SpawnFailure,
            duration_secs: start.elapsed().as_secs_f64(),
            stdout: String::new(),
            stderr: format!("creating {}: {err}", stderr_path.display()),
            idle_timeout_seconds: profile.review_timeout_seconds(),
            hard_timeout_seconds,
            last_progress_secs: None,
        };
    }

    let mut cmd = Command::new(&executable);
    match backend {
        "claude" => {
            cmd.args(["-p", prompt]).args(&profile.claude_args);
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
        }
        "codex" => {
            cmd.arg("exec")
                .arg(prompt)
                .args(filtered_codex_args(&profile.codex_args))
                .args(codex_model_args(effective_model));
        }
        "agy" | "agy-main" | "agy-second" => {
            cmd.arg("--print").arg(prompt);
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
            cmd.arg("--dangerously-skip-permissions");
        }
        "vibe" => {
            cmd.arg("-p").arg(prompt);
            cmd.arg("--output").arg("text");
            cmd.arg("--trust");
            cmd.arg("--auto-approve");
        }
        "opencode" => {
            cmd.arg("run");
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
            cmd.arg(prompt);
        }
        _ => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: format!("unsupported review backend: {backend}"),
                idle_timeout_seconds: profile.review_timeout_seconds(),
                hard_timeout_seconds,
                last_progress_secs: None,
            };
        }
    }
    cmd.current_dir(worktree)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    prepare_process_group(&mut cmd);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    if backend == "vibe" {
        if let Some(model) = effective_model {
            cmd.env("VIBE_ACTIVE_MODEL", model);
        }
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: err.to_string(),
                idle_timeout_seconds: profile.review_timeout_seconds(),
                hard_timeout_seconds,
                last_progress_secs: None,
            };
        }
    };
    let (progress_tx, progress_rx) = mpsc::channel();
    let stdout_thread = child
        .stdout
        .take()
        .map(|stdout| copy_stream_to_file(stdout, stdout_path.clone(), Some(progress_tx.clone())));
    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| copy_stream_to_file(stderr, stderr_path.clone(), Some(progress_tx.clone())));
    drop(progress_tx);

    let idle_timeout_seconds = profile.review_timeout_seconds();
    let idle_timeout = Duration::from_secs(idle_timeout_seconds);
    let hard_timeout = hard_timeout_seconds.map(Duration::from_secs);
    // A backend that emits no output is already indistinguishable from a stalled
    // launch after one idle window. Do not double the stall budget.
    let startup_grace = idle_timeout;
    let poll_interval = Duration::from_millis(25);
    let worktree_poll_interval = Duration::from_secs(1);
    let process_group = child.id();
    let mut last_stdout_len = 0u64;
    let mut last_stderr_len = 0u64;
    let mut last_worktree_snapshot = worktree_progress_snapshot(worktree);
    let mut last_process_activity = process_group_activity_snapshot(process_group);
    let mut last_worktree_poll = Instant::now();
    let mut last_progress_at = Instant::now();
    let mut last_progress_elapsed_secs = None;
    let mut saw_progress = false;
    let mut cleanup_error = None;
    let mut supplemental_stderr = None;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    break ReviewProcessOutcome::Success;
                }
                if let Some(code) = status.code() {
                    break ReviewProcessOutcome::NonZeroExit(code);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(signal) = status.signal() {
                        break ReviewProcessOutcome::SignalTermination(signal);
                    }
                }
                break ReviewProcessOutcome::SpawnFailure;
            }
            Ok(None) => {
                if shutdown_requested() {
                    cleanup_error = kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::SignalTermination(libc::SIGTERM);
                }
                // Progress is any of: new stdout/stderr bytes (stream activity or
                // review-artifact writes), worktree changes (a reviewer editing
                // source), or descendant CPU/I/O (e.g. reading a large diff or
                // running tests).
                let stdout_len = fs::metadata(&stdout_path).map(|m| m.len()).unwrap_or(0);
                let stderr_len = fs::metadata(&stderr_path).map(|m| m.len()).unwrap_or(0);
                let stream_grew = progress_rx.try_iter().last().is_some()
                    || stdout_len > last_stdout_len
                    || stderr_len > last_stderr_len;
                if stream_grew {
                    last_stdout_len = stdout_len;
                    last_stderr_len = stderr_len;
                    last_progress_at = Instant::now();
                    last_progress_elapsed_secs = Some(start.elapsed().as_secs_f64());
                    saw_progress = true;
                }
                if last_worktree_poll.elapsed() >= worktree_poll_interval {
                    if let Some(snapshot) = worktree_progress_snapshot(worktree) {
                        if last_worktree_snapshot.as_ref() != Some(&snapshot) {
                            last_worktree_snapshot = Some(snapshot);
                            last_progress_at = Instant::now();
                            last_progress_elapsed_secs = Some(start.elapsed().as_secs_f64());
                            saw_progress = true;
                        }
                    }
                    if let Some(activity) = process_group_activity_snapshot(process_group) {
                        if process_group_activity_advanced(
                            last_process_activity.as_deref(),
                            &activity,
                        ) {
                            last_progress_at = Instant::now();
                            last_progress_elapsed_secs = Some(start.elapsed().as_secs_f64());
                            saw_progress = true;
                        }
                        last_process_activity = Some(activity);
                    }
                    last_worktree_poll = Instant::now();
                }
                // A silent reviewer is killed only after the idle budget elapses
                // with no progress. A busy reviewer that keeps producing output
                // completes regardless of how long it runs.
                let stalled = if saw_progress {
                    last_progress_at.elapsed() >= idle_timeout
                } else {
                    start.elapsed() >= startup_grace
                };
                if stalled {
                    cleanup_error = kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::IdleTimeout;
                }
                // Optional hard ceiling: a separate explicit safety policy. Even
                // a continuously-active reviewer is killed here -- but classified
                // as HardTimeout, never as a backend failure.
                if hard_timeout.is_some_and(|timeout| start.elapsed() >= timeout) {
                    cleanup_error = kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::HardTimeout;
                }
                thread::sleep(poll_interval);
            }
            Err(err) => {
                cleanup_error = kill_process_group(&mut child);
                let _ = child.wait();
                supplemental_stderr = Some(err.to_string());
                break ReviewProcessOutcome::SpawnFailure;
            }
        }
    };
    // A surviving descendant may still own the inherited pipe descriptors.
    // Do not let joining the stream-copy threads turn a bounded cleanup
    // failure into another hang; dropping the handles detaches them.
    if cleanup_error.is_none() {
        if let Some(handle) = stdout_thread {
            let _ = handle.join();
        }
        if let Some(handle) = stderr_thread {
            let _ = handle.join();
        }
    }
    if progress_rx.try_iter().last().is_some() {
        last_progress_elapsed_secs = Some(start.elapsed().as_secs_f64());
    }

    let mut stdout = read_text_file(&stdout_path);
    let mut stderr = read_text_file(&stderr_path);
    if let Some(extra) = supplemental_stderr {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str(&extra);
    }
    let outcome = if let Some(error) = cleanup_error {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str("GAH: harness process cleanup failed: ");
        stderr.push_str(&error);
        ReviewProcessOutcome::CleanupFailure(error)
    } else if matches!(backend, "agy" | "agy-main" | "agy-second")
        && matches!(outcome, ReviewProcessOutcome::Success)
        && stdout.trim().is_empty()
    {
        stdout = agy_empty_output_diagnosis(env_vars, &executable);
        ReviewProcessOutcome::NonZeroExit(-1)
    } else {
        outcome
    };

    ReviewRunResult {
        outcome,
        duration_secs: start.elapsed().as_secs_f64(),
        stdout,
        stderr,
        idle_timeout_seconds,
        hard_timeout_seconds,
        last_progress_secs: last_progress_elapsed_secs,
    }
}

fn read_text_file(path: &Path) -> String {
    let mut buf = Vec::new();
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;
    use crate::test_support::PathGuard;

    #[test]
    fn run_review_backend_times_out_and_preserves_partial_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\necho 'partial review'\nsleep 2\necho 'late stderr' >&2\n",
        );
        let mut profile = test_profile();
        profile.review_timeout_seconds = Some(1);
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::IdleTimeout);
        assert!(result.stdout.contains("partial review"));
    }

    #[test]
    fn run_review_backend_active_reviewer_completes_beyond_idle_window() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Produces output every 0.1s for ~2s: well beyond the 1s idle budget,
        // but never silent, so it must run to completion (issue #540).
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\nfor i in $(seq 1 20); do echo \"line $i\"; sleep 0.1; done\n",
        );
        let mut profile = test_profile();
        profile.review_timeout_seconds = Some(1);
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        assert!(result.duration_secs >= 1.0);
        assert!(result.last_progress_secs.is_some());
        assert_eq!(result.hard_timeout_seconds, None);
    }

    #[test]
    fn run_review_backend_respects_hard_timeout_while_active() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\nwhile true; do echo 'busy'; sleep 0.1; done\n",
        );
        let mut profile = test_profile();
        profile.review_timeout_seconds = Some(300); // long idle budget
        profile.review_hard_timeout_seconds = Some(1);
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::HardTimeout);
        assert!(result.stdout.contains("busy"));
        assert_eq!(result.hard_timeout_seconds, Some(1));
        assert!(result.last_progress_secs.is_some());
    }

    #[test]
    fn run_review_backend_kills_process_group_on_idle() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Spawns a long-lived descendant then goes silent. The whole process
        // group must be killed at idle, not just the parent (which would leave
        // the child running and hang the run for 30s).
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\nsleep 30 &\necho 'started'\nwait\n",
        );
        let mut profile = test_profile();
        profile.review_timeout_seconds = Some(1);
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::IdleTimeout);
        assert!(result.duration_secs < 5.0);
    }

    #[test]
    fn run_review_backend_supports_agy_with_model_and_env() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "agy",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("Claude Sonnet 4.6 (Thinking)"),
            &[("FROM_ENV_FILE".into(), "agy-review-env".into())],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        assert!(result.stdout.contains("stdout-marker-agy"));
        assert!(result.stderr.contains("stderr-marker-agy"));

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "--print");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"Claude Sonnet 4.6 (Thinking)".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=agy-review-env"));
    }

    #[test]
    fn run_review_backend_supports_vibe_without_model_flag() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "vibe",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("mistral-medium-3.5"),
            &[("FROM_ENV_FILE".into(), "vibe-review-env".into())],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        assert!(result.stdout.contains("stdout-marker-vibe"));
        assert!(result.stderr.contains("stderr-marker-vibe"));

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"text".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        assert!(!argv.contains(&"--model".to_string()));
        assert!(!argv.contains(&"mistral-medium-3.5".to_string()));

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=vibe-review-env"));
        assert!(env.contains("VIBE_ACTIVE_MODEL=mistral-medium-3.5"));
    }

    #[test]
    fn run_review_backend_binds_claude_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("haiku"),
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
    }

    #[test]
    fn run_review_backend_vibe_command_construction() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "vibe",
            &f.worktree,
            "Review this code: int main() { return 0; }",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"text".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        assert!(!argv.contains(&"review".to_string()));
        assert!(!argv.contains(&"--model".to_string()));
    }
}
