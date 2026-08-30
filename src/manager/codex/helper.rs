use super::make_nonblocking;
use anyhow::{anyhow, Context};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::process::{Command, Output, Stdio};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

const CAPTURE_MAX_BYTES: usize = 64 * 1024;
const CLEANUP_GRACE: Duration = Duration::from_millis(250);
#[cfg(test)]
pub(super) static FAIL_HELPER_CLEANUP_AFTER_REAP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    eof: bool,
    read_failed: bool,
    overflowed: bool,
}

impl Capture {
    fn drain(&mut self, reader: &mut impl Read) -> std::io::Result<()> {
        if self.eof || self.read_failed {
            return Ok(());
        }
        let mut chunk = [0_u8; 8192];
        match reader.read(&mut chunk) {
            Ok(0) => self.eof = true,
            Ok(read) => {
                let retained = CAPTURE_MAX_BYTES.saturating_sub(self.bytes.len());
                self.bytes.extend_from_slice(&chunk[..read.min(retained)]);
                self.overflowed |= read > retained;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => {
                self.read_failed = true;
                return Err(error);
            }
        }
        Ok(())
    }

    fn finished(&self) -> bool {
        self.eof || self.read_failed
    }
}

pub(super) enum HelperCommandFailure {
    Command,
    Fatal(anyhow::Error),
}

impl From<anyhow::Error> for HelperCommandFailure {
    fn from(_: anyhow::Error) -> Self {
        Self::Command
    }
}

fn terminate_group(child: &mut std::process::Child) -> Option<String> {
    let result = unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            let _ = child.kill();
            return Some(format!("signaling helper process group: {error}"));
        }
    }
    let _ = child.kill();
    None
}

pub(super) fn bounded_command_output(
    command: &mut Command,
    context: &str,
    timeout: Duration,
) -> Result<Output, HelperCommandFailure> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::runner::process::prepare_process_group(command);
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {context}"))?;
    let deadline = Instant::now() + timeout;
    let cleanup_deadline = deadline + CLEANUP_GRACE;
    let mut stdout_reader = child.stdout.take().expect("piped helper stdout");
    let mut stderr_reader = child.stderr.take().expect("piped helper stderr");
    let mut stdout = Capture::default();
    let mut stderr = Capture::default();
    let mut status = None;
    let mut failure: Option<(bool, anyhow::Error)> = None;

    if let Err(error) = make_nonblocking(stdout_reader.as_raw_fd()) {
        failure = Some((
            true,
            anyhow!("making {context} stdout nonblocking: {error}"),
        ));
    } else if let Err(error) = make_nonblocking(stderr_reader.as_raw_fd()) {
        failure = Some((
            true,
            anyhow!("making {context} stderr nonblocking: {error}"),
        ));
    }

    while failure.is_none() {
        if let Err(error) = stdout.drain(&mut stdout_reader) {
            failure = Some((true, anyhow!("reading {context} stdout: {error}")));
            break;
        }
        if let Err(error) = stderr.drain(&mut stderr_reader) {
            failure = Some((true, anyhow!("reading {context} stderr: {error}")));
            break;
        }
        if stdout.overflowed || stderr.overflowed {
            let stream = if stdout.overflowed {
                "stdout"
            } else {
                "stderr"
            };
            failure = Some((
                true,
                anyhow!("{context} {stream} exceeded {CAPTURE_MAX_BYTES} bytes"),
            ));
            break;
        }
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                failure = Some((false, anyhow!("{context} timed out after {timeout:?}")));
                break;
            }
            Err(error) => {
                failure = Some((false, anyhow!("waiting for {context}: {error}")));
                break;
            }
        }
    }

    let mut cleanup_failures = Vec::new();
    if let Some(error) = terminate_group(&mut child) {
        cleanup_failures.push(error);
    }
    loop {
        if let Err(error) = stdout.drain(&mut stdout_reader) {
            cleanup_failures.push(format!("reading stdout during cleanup: {error}"));
        }
        if let Err(error) = stderr.drain(&mut stderr_reader) {
            cleanup_failures.push(format!("reading stderr during cleanup: {error}"));
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => status = Some(exit),
                Ok(None) => {}
                Err(error) => {
                    cleanup_failures.push(format!("reaping helper: {error}"));
                    break;
                }
            }
        }
        if status.is_some() && stdout.finished() && stderr.finished() {
            break;
        }
        if Instant::now() >= cleanup_deadline {
            if status.is_none() {
                cleanup_failures.push("helper was not reaped before cleanup deadline".into());
            }
            if !stdout.finished() || !stderr.finished() {
                cleanup_failures.push("capture pipe remained open after helper cleanup".into());
            }
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    #[cfg(test)]
    if FAIL_HELPER_CLEANUP_AFTER_REAP.swap(false, Ordering::SeqCst) {
        cleanup_failures.push("injected unconfirmed helper cleanup".into());
    }
    if !cleanup_failures.is_empty() {
        let cleanup = cleanup_failures.join("; ");
        let error = match failure {
            Some((_, failure)) => anyhow!("{failure:#}; helper cleanup failed: {cleanup}"),
            None => anyhow!("{context} exited but helper cleanup failed: {cleanup}"),
        };
        return Err(HelperCommandFailure::Fatal(error));
    }
    if let Some((fatal, failure)) = failure {
        return Err(if fatal {
            HelperCommandFailure::Fatal(failure)
        } else {
            let _ = failure;
            HelperCommandFailure::Command
        });
    }
    Ok(Output {
        status: status.expect("successful helper was reaped"),
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}
