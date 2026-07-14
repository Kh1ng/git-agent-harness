use crate::config::Profile;
use crate::runner::backends::agy::agy_empty_output_diagnosis;
use crate::runner::process::{
    copy_stream_to_file, kill_process_group, prepare_process_group, shutdown_requested,
    write_redacted_task,
};
use crate::runner::resolve::{
    codex_model_args, filtered_codex_args, resolve_backend_executable, ExecutableResolution,
};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewProcessOutcome {
    Success,
    ExecutableUnavailable,
    SpawnFailure,
    NonZeroExit(i32),
    SignalTermination(i32),
    Timeout,
}

#[derive(Debug)]
pub struct ReviewRunResult {
    pub outcome: ReviewProcessOutcome,
    pub duration_secs: f64,
    pub stdout: String,
    pub stderr: String,
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
            };
        }
        ExecutableResolution::UnknownBackend(_) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: format!("unsupported review backend: {backend}"),
            };
        }
    };

    if let Err(err) = fs::File::create(&stdout_path) {
        return ReviewRunResult {
            outcome: ReviewProcessOutcome::SpawnFailure,
            duration_secs: start.elapsed().as_secs_f64(),
            stdout: String::new(),
            stderr: format!("creating {}: {err}", stdout_path.display()),
        };
    }
    if let Err(err) = fs::File::create(&stderr_path) {
        return ReviewRunResult {
            outcome: ReviewProcessOutcome::SpawnFailure,
            duration_secs: start.elapsed().as_secs_f64(),
            stdout: String::new(),
            stderr: format!("creating {}: {err}", stderr_path.display()),
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
            };
        }
    };
    let stdout_thread = child
        .stdout
        .take()
        .map(|stdout| copy_stream_to_file(stdout, stdout_path.clone(), None));
    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| copy_stream_to_file(stderr, stderr_path.clone(), None));

    let timeout = Duration::from_secs(profile.review_timeout_seconds());
    let poll_interval = Duration::from_millis(25);
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
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::SignalTermination(libc::SIGTERM);
                }
                if start.elapsed() >= timeout {
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::Timeout;
                }
                thread::sleep(poll_interval);
            }
            Err(err) => {
                kill_process_group(&mut child);
                let _ = child.wait();
                let mut stderr = read_text_file(&stderr_path);
                if !stderr.is_empty() {
                    stderr.push('\n');
                }
                stderr.push_str(&err.to_string());
                return ReviewRunResult {
                    outcome: ReviewProcessOutcome::SpawnFailure,
                    duration_secs: start.elapsed().as_secs_f64(),
                    stdout: read_text_file(&stdout_path),
                    stderr,
                };
            }
        }
    };
    if let Some(handle) = stdout_thread {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }

    let mut stdout = read_text_file(&stdout_path);
    let outcome = if matches!(backend, "agy" | "agy-main" | "agy-second")
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
        stderr: read_text_file(&stderr_path),
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

        assert_eq!(result.outcome, ReviewProcessOutcome::Timeout);
        assert!(result.stdout.contains("partial review"));
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
