use super::make_nonblocking;
use anyhow::{anyhow, Context};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::process::{Command, Output, Stdio};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

mod holders;

const CAPTURE_MAX_BYTES: usize = 64 * 1024;
const CLEANUP_GRACE: Duration = Duration::from_millis(250);
#[cfg(test)]
pub(super) static FAIL_HELPER_CLEANUP_AFTER_REAP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
pub(super) static FAIL_HELPER_NONBLOCKING: std::sync::atomic::AtomicBool =
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

fn drain_reader(capture: &mut Capture, reader: &mut Option<impl Read>) -> std::io::Result<()> {
    match reader {
        Some(reader) => capture.drain(reader),
        None => Ok(()),
    }
}

fn cleanup_overflow(stdout: &Capture, stderr: &Capture, context: &str) -> Option<String> {
    let stream = if stdout.overflowed {
        "stdout"
    } else if stderr.overflowed {
        "stderr"
    } else {
        return None;
    };
    Some(format!(
        "{context} {stream} exceeded {CAPTURE_MAX_BYTES} bytes during cleanup"
    ))
}

fn make_helper_pipe_nonblocking(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_HELPER_NONBLOCKING.swap(false, Ordering::SeqCst) {
        return Err(std::io::Error::other("injected helper nonblocking failure"));
    }
    make_nonblocking(fd)
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

fn terminate_group(child: &mut std::process::Child, exited: bool) -> Option<String> {
    let result = unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH)
            && !(exited && error.raw_os_error() == Some(libc::EPERM))
        {
            let _ = child.kill();
            return Some(format!("signaling helper process group: {error}"));
        }
    }
    let _ = child.kill();
    None
}

fn exited_without_reaping(child: &std::process::Child) -> std::io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init().si_pid() } != 0)
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
    let mut stdout_reader = Some(child.stdout.take().expect("piped helper stdout"));
    let mut stderr_reader = Some(child.stderr.take().expect("piped helper stderr"));
    let mut stdout = Capture::default();
    let mut stderr = Capture::default();
    let mut status = None;
    let mut failure: Option<(bool, anyhow::Error)> = None;
    let mut pipes = Vec::new();
    let mut setup_failures = Vec::new();

    for (stream, fd) in [
        (
            "stdout",
            stdout_reader.as_ref().expect("stdout reader").as_raw_fd(),
        ),
        (
            "stderr",
            stderr_reader.as_ref().expect("stderr reader").as_raw_fd(),
        ),
    ] {
        match holders::PipeIdentity::from_fd(fd) {
            Ok(pipe) => pipes.push(pipe),
            Err(error) => setup_failures.push(format!("identifying {context} {stream}: {error}")),
        }
    }
    if let Some(reader) = stdout_reader.as_ref() {
        if let Err(error) = make_helper_pipe_nonblocking(reader.as_raw_fd()) {
            setup_failures.push(format!("making {context} stdout nonblocking: {error}"));
            stdout.read_failed = true;
            stdout_reader = None;
        }
    }
    if let Some(reader) = stderr_reader.as_ref() {
        if let Err(error) = make_helper_pipe_nonblocking(reader.as_raw_fd()) {
            setup_failures.push(format!("making {context} stderr nonblocking: {error}"));
            stderr.read_failed = true;
            stderr_reader = None;
        }
    }
    if !setup_failures.is_empty() {
        failure = Some((true, anyhow!(setup_failures.join("; "))));
    }

    while failure.is_none() {
        if let Err(error) = drain_reader(&mut stdout, &mut stdout_reader) {
            failure = Some((true, anyhow!("reading {context} stdout: {error}")));
            break;
        }
        if let Err(error) = drain_reader(&mut stderr, &mut stderr_reader) {
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
        match exited_without_reaping(&child) {
            Ok(true) => {
                break;
            }
            Ok(false) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(false) => {
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
    let exited = match exited_without_reaping(&child) {
        Ok(exited) => exited,
        Err(error) => {
            cleanup_failures.push(format!("checking {context} before cleanup: {error}"));
            false
        }
    };
    if let Some(error) = terminate_group(&mut child, exited) {
        cleanup_failures.push(error);
    }
    let mut overflow_reported = stdout.overflowed || stderr.overflowed;
    if let Err(error) = drain_reader(&mut stdout, &mut stdout_reader) {
        cleanup_failures.push(format!("reading stdout during cleanup: {error}"));
    }
    if let Err(error) = drain_reader(&mut stderr, &mut stderr_reader) {
        cleanup_failures.push(format!("reading stderr during cleanup: {error}"));
    }
    if !overflow_reported {
        if let Some(error) = cleanup_overflow(&stdout, &stderr, context) {
            cleanup_failures.push(error);
            overflow_reported = true;
        }
    }
    if (!stdout.eof || !stderr.eof) && !pipes.is_empty() {
        if let Err(error) = holders::terminate_pipe_holders(&pipes, cleanup_deadline) {
            cleanup_failures.push(error);
        }
    }
    loop {
        if let Err(error) = drain_reader(&mut stdout, &mut stdout_reader) {
            cleanup_failures.push(format!("reading stdout during cleanup: {error}"));
        }
        if let Err(error) = drain_reader(&mut stderr, &mut stderr_reader) {
            cleanup_failures.push(format!("reading stderr during cleanup: {error}"));
        }
        if !overflow_reported {
            if let Some(error) = cleanup_overflow(&stdout, &stderr, context) {
                cleanup_failures.push(error);
                overflow_reported = true;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waitid_observes_exit_without_reaping_the_group_leader() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        crate::runner::process::prepare_process_group(&mut command);
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !exited_without_reaping(&child).unwrap() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(terminate_group(&mut child, true).is_none());
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn final_cleanup_drain_rechecks_the_capture_limit() {
        let mut capture = Capture {
            bytes: vec![0; CAPTURE_MAX_BYTES],
            ..Capture::default()
        };
        capture.drain(&mut std::io::Cursor::new([1])).unwrap();

        assert_eq!(
            cleanup_overflow(&capture, &Capture::default(), "helper").as_deref(),
            Some("helper stdout exceeded 65536 bytes during cleanup")
        );
    }
}
