use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const VALIDATION_COMMAND_TIMEOUT_EXIT_CODE: i32 = 124;

fn is_validation_timeout_error(error: &str) -> bool {
    error.contains("timed out after")
}

/// Run validation commands sequentially in the worktree and surface the first
/// command failure with its combined output.
pub(crate) fn validate(
    commands: &[String],
    worktree: &Path,
    env_vars: &[(String, String)],
    timeout: Duration,
) -> Result<()> {
    for command in commands {
        if command.trim().is_empty() {
            continue;
        }
        println!("  Validating: {command}");
        let output = run_shell_command(command, worktree, env_vars, timeout)
            .with_context(|| format!("failed to run '{command}'"))?;
        if !output.status.success() {
            bail!(
                "$ {}\n{}{}",
                command,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
    Ok(())
}

/// Run validation while preserving the first failing command's exit code for
/// baseline failure classification.
pub(crate) fn validate_with_exit_code(
    commands: &[String],
    worktree: &Path,
    env_vars: &[(String, String)],
    timeout: Duration,
) -> Result<(), (String, Option<i32>)> {
    for command in commands {
        if command.trim().is_empty() {
            continue;
        }
        println!("  Validating: {command}");
        let output = run_shell_command(command, worktree, env_vars, timeout).map_err(|error| {
            let error_text = format!("failed to run '{command}': {error:#}");
            let exit_code = is_validation_timeout_error(&error_text)
                .then_some(VALIDATION_COMMAND_TIMEOUT_EXIT_CODE);
            (error_text, exit_code)
        })?;
        if !output.status.success() {
            return Err((
                format!(
                    "$ {}\n{}{}",
                    command,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ),
                output.status.code(),
            ));
        }
    }
    Ok(())
}

fn run_shell_command(
    command: &str,
    worktree: &Path,
    env_vars: &[(String, String)],
    timeout: Duration,
) -> Result<Output> {
    run_shell_command_with_shutdown(command, worktree, env_vars, timeout, || {
        crate::runner::shutdown_requested()
    })
}

fn run_shell_command_with_shutdown(
    command_text: &str,
    worktree: &Path,
    env_vars: &[(String, String)],
    timeout: Duration,
    shutdown_requested: impl Fn() -> bool,
) -> Result<Output> {
    if shutdown_requested() {
        bail!("shutdown requested before validation command '{command_text}'");
    }

    let mut command = Command::new("sh");
    command
        .args(["-c", command_text])
        .current_dir(worktree)
        // Validation is always unattended. Inheriting a manager/dispatch PTY
        // makes this newly-created process group a background terminal group;
        // a nested PTY helper such as `script(1)` can then receive SIGTTOU and
        // stop the entire validation tree indefinitely.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env_vars {
        command.env(key, value);
    }
    crate::runner::prepare_process_group(&mut command);

    let mut child = command
        .spawn()
        .with_context(|| format!("spawning validation command '{command_text}'"))?;
    let mut stdout = child.stdout.take().context("capturing validation stdout")?;
    let mut stderr = child.stderr.take().context("capturing validation stderr")?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("waiting for validation command '{command_text}'"))?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            crate::runner::kill_process_group(&mut child);
            let _ = child.wait();
            let _ = stdout_reader
                .join()
                .map_err(|_| anyhow::anyhow!("validation stdout reader panicked"))??;
            let _ = stderr_reader
                .join()
                .map_err(|_| anyhow::anyhow!("validation stderr reader panicked"))??;
            bail!(
                "validation command '{command_text}' timed out after {:.1}s (configured timeout {:.1}s)",
                started.elapsed().as_secs_f64(),
                timeout.as_secs_f64(),
            );
        }
        if shutdown_requested() {
            crate::runner::kill_process_group(&mut child);
            let _ = child.wait();
            let _ = stdout_reader
                .join()
                .map_err(|_| anyhow::anyhow!("validation stdout reader panicked"))??;
            let _ = stderr_reader
                .join()
                .map_err(|_| anyhow::anyhow!("validation stderr reader panicked"))??;
            bail!("shutdown requested during validation command '{command_text}'");
        }
        thread::sleep(Duration::from_millis(50));
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("validation stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("validation stderr reader panicked"))??;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::{run_shell_command_with_shutdown, validate};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn supports_shell_syntax() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        validate(
            &["cd sub && true".into()],
            tmp.path(),
            &[],
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    }

    #[test]
    fn reports_failing_command_output() {
        let tmp = tempfile::tempdir().unwrap();
        let error = validate(
            &["echo oops >&2 && false".into()],
            tmp.path(),
            &[],
            std::time::Duration::from_secs(30),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("oops"));
    }

    #[test]
    fn propagates_validation_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let observed = tmp.path().join("observed-target");
        let expected = tmp.path().join("shared-cargo-target");
        let commands = vec![format!(
            "printf '%s' \"$CARGO_TARGET_DIR\" > {}",
            observed.display()
        )];
        let environment = vec![(
            "CARGO_TARGET_DIR".to_string(),
            expected.to_string_lossy().into_owned(),
        )];

        validate(
            &commands,
            tmp.path(),
            &environment,
            std::time::Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(observed).unwrap(),
            expected.display().to_string()
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn validation_commands_receive_noninteractive_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let output = run_shell_command_with_shutdown(
            "readlink /proc/self/fd/0",
            tmp.path(),
            &[],
            std::time::Duration::from_secs(30),
            || false,
        )
        .unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "/dev/null");
    }

    #[test]
    fn shutdown_kills_the_command_process_group_promptly() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("orphan-marker");
        let command = format!("(sleep 2; printf orphaned > '{}') & wait", marker.display());
        let shutdown = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&shutdown);
        let trigger_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            trigger.store(true, Ordering::SeqCst);
        });

        let started = Instant::now();
        let error = run_shell_command_with_shutdown(
            &command,
            tmp.path(),
            &[],
            std::time::Duration::from_secs(1),
            || shutdown.load(Ordering::SeqCst),
        )
        .unwrap_err();
        trigger_thread.join().unwrap();

        assert!(error.to_string().contains("shutdown requested"));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "validation shutdown should not wait for the shell command"
        );
        std::thread::sleep(Duration::from_millis(2100));
        assert!(
            !marker.exists(),
            "the validation command's background child must die with its process group"
        );
    }

    #[test]
    fn timeout_kills_the_validation_command_process_group() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("timeout-marker");
        let command = format!(
            "(sleep 1; printf timed_out > '{}') & wait",
            marker.display()
        );
        let started = Instant::now();
        let error = run_shell_command_with_shutdown(
            &command,
            tmp.path(),
            &[],
            std::time::Duration::from_millis(150),
            || false,
        )
        .unwrap_err();

        assert!(error.to_string().contains("timed out after"));
        assert!(started.elapsed() >= std::time::Duration::from_millis(150));
        assert!(
            error.to_string().contains("timed out after"),
            "timeout error should identify elapsed timeout"
        );
        assert!(
            error.to_string().contains(&command),
            "timeout error should identify the command"
        );
        std::thread::sleep(Duration::from_millis(250));
        assert!(
            !marker.exists(),
            "the validation command's background child must die with its process group"
        );
    }
}
