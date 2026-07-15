//! Generic child-process lifecycle and watchdogs, extracted from the runner
//! facade.
//!
//! This module owns the backend-agnostic mechanics of launching a child
//! process group, copying/redacting its streams to a log, snapshotting
//! worktree and descendant CPU/I/O progress, and killing the group on idle
//! stall or graceful shutdown. Backend-specific argv construction, executable
//! resolution, usage/log discovery, and review invocation stay in the facade.

use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Set by the loop/dispatch process's SIGINT/SIGTERM handler. Backend runners
/// poll it and terminate their dedicated process group, allowing the caller to
/// write the normal terminal event and ledger record before exiting.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn install_shutdown_handler() -> Result<()> {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    ctrlc::set_handler(|| {
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    })
    .context("installing graceful shutdown handler")
}

pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

#[cfg(unix)]
pub(crate) fn prepare_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub(crate) fn prepare_process_group(_cmd: &mut Command) {}

pub(crate) fn kill_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
}

pub(crate) fn copy_stream_to_file<R: Read + Send + 'static>(
    mut reader: R,
    path: PathBuf,
    progress_tx: Option<mpsc::Sender<()>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let mut buf = [0_u8; 8192];
        let mut pending = Vec::new();
        while let Ok(read) = reader.read(&mut buf) {
            if read == 0 {
                break;
            }
            pending.extend_from_slice(&buf[..read]);
            while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<_> = pending.drain(..=newline).collect();
                let text = String::from_utf8_lossy(&line);
                if file
                    .write_all(crate::redact::redact(&text).as_bytes())
                    .is_err()
                {
                    return;
                }
            }
            let _ = file.flush();
            if let Some(tx) = &progress_tx {
                let _ = tx.send(());
            }
        }
        if !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending);
            let _ = file.write_all(crate::redact::redact(&text).as_bytes());
            let _ = file.flush();
        }
    })
}

pub(crate) fn write_redacted_task(session_dir: &Path, task: &str) -> Result<()> {
    fs::write(session_dir.join("task.md"), crate::redact::redact(task))
        .context("writing redacted task artifact")
}

/// Return a content-sensitive snapshot of a worktree's tracked changes.
///
/// Several subscription CLIs perform tool calls without forwarding their
/// progress to stdout. Treating that silence as a hang kills an agent that is
/// still editing source. `git diff` makes those edits observable without
/// walking ignored build output such as `target/` or `node_modules/`.
fn worktree_progress_snapshot(worktree: &Path) -> Option<Vec<u8>> {
    let diff = Command::new("git")
        .args(["diff", "--no-ext-diff", "--binary", "HEAD", "--"])
        .current_dir(worktree)
        .output()
        .ok()?;
    if !diff.status.success() {
        return None;
    }
    let staged = Command::new("git")
        .args([
            "diff",
            "--cached",
            "--no-ext-diff",
            "--binary",
            "HEAD",
            "--",
        ])
        .current_dir(worktree)
        .output()
        .ok()?;
    if !staged.status.success() {
        return None;
    }
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(worktree)
        .output()
        .ok()?;
    if !status.status.success() {
        return None;
    }

    let mut snapshot = diff.stdout;
    snapshot.extend_from_slice(&staged.stdout);
    snapshot.extend_from_slice(&status.stdout);
    Some(snapshot)
}

/// Linux process-group activity that is meaningful even when a backend is
/// quiet and its source tree is stable. Build and test commands commonly run
/// for minutes without writing either stream or touching tracked files. CPU
/// or I/O activity proves that the backend is still executing; mere process
/// existence or idle child churn does not, so a sleeping/hung backend expires.
#[cfg(target_os = "linux")]
fn process_group_activity_snapshot(process_group: u32) -> Option<Vec<(u32, u64, u64, u64)>> {
    let mut members = Vec::new();
    for entry in fs::read_dir("/proc").ok()? {
        let Ok(entry) = entry else { continue };
        let pid = entry.file_name().to_string_lossy().parse::<u32>().ok();
        let Some(pid) = pid else { continue };
        // Backend activity itself is already represented by output/worktree
        // progress. Only descendants prove a quiet tool/verification command
        // is executing; counting a spinning backend root would mask a hang.
        if pid == process_group {
            continue;
        }
        let stat = fs::read_to_string(entry.path().join("stat")).ok();
        let Some(stat) = stat else { continue };
        // `comm` is parenthesized and may contain spaces or parentheses, so
        // split only after its final closing delimiter. The remaining fields
        // start at field 3 (`state`).
        let Some(fields) = stat
            .rfind(") ")
            .map(|end| stat[end + 2..].split_whitespace().collect::<Vec<_>>())
        else {
            continue;
        };
        let Some(pgrp) = fields.get(2).and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        if pgrp != process_group {
            continue;
        }
        let user_ticks = fields
            .get(11)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let system_ticks = fields
            .get(12)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let io_bytes = fs::read_to_string(entry.path().join("io"))
            .ok()
            .map(|io| {
                io.lines()
                    .filter_map(|line| line.split_once(':'))
                    .filter(|(name, _)| {
                        matches!(
                            name.trim(),
                            "rchar" | "wchar" | "read_bytes" | "write_bytes"
                        )
                    })
                    .filter_map(|(_, value)| value.trim().parse::<u64>().ok())
                    .sum()
            })
            .unwrap_or(0);
        members.push((pid, user_ticks, system_ticks, io_bytes));
    }
    members.sort_unstable();
    Some(members)
}

#[cfg(not(target_os = "linux"))]
fn process_group_activity_snapshot(_process_group: u32) -> Option<Vec<(u32, u64, u64, u64)>> {
    None
}

fn process_group_activity_advanced(
    previous: Option<&[(u32, u64, u64, u64)]>,
    current: &[(u32, u64, u64, u64)],
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    current.iter().any(|&(pid, user, system, io)| {
        previous
            .iter()
            .find(|&&(previous_pid, _, _, _)| previous_pid == pid)
            .map(|&(_, previous_user, previous_system, previous_io)| {
                user > previous_user || system > previous_system || io > previous_io
            })
            // Process creation alone is not progress: a stalled shell can
            // churn `sleep` children forever. A new descendant only counts
            // after it has consumed measurable CPU, as real compilers and
            // test processes do.
            .unwrap_or(user > 0 || system > 0)
    })
}

/// Spawn `cmd` (stdout/stderr are set to piped by this helper) and drive it
/// to completion, killing it only once both its log and worktree have gone
/// genuinely quiet for `idle_timeout_seconds` -- never on a flat wall-clock
/// budget. Shared by every backend invocation that needs hang protection
/// (agy, opencode, openhands, vibe, codex, claude) -- extracted after the
/// third copy-paste of this exact loop (issues #170/#87) made the
/// duplication no longer defensible.
///
/// `spawn_context` labels a spawn failure (e.g. "launching vibe; is it
/// installed and on PATH?"). Returns `(exit_code, duration_secs)`; on an
/// idle kill, exit_code is -1 and a trailing note is appended to the log.
pub(crate) fn spawn_with_idle_watch(
    cmd: Command,
    log_path: &Path,
    worktree: &Path,
    idle_timeout_seconds: u64,
    spawn_context: &str,
) -> Result<(i32, f64)> {
    spawn_with_idle_watch_with_shutdown(
        cmd,
        log_path,
        worktree,
        idle_timeout_seconds,
        spawn_context,
        &SHUTDOWN_REQUESTED,
        true,
    )
}

/// Run a backend under a semantic-progress watch. Unlike the generic idle
/// watch, arbitrary stdout/stderr does not reset this window: only a durable
/// worktree change does. This is for CLIs such as OpenCode that can stream
/// malformed tool-call chatter indefinitely without actually executing work.
pub(crate) fn spawn_with_worktree_progress_watch(
    cmd: Command,
    log_path: &Path,
    worktree: &Path,
    idle_timeout_seconds: u64,
    spawn_context: &str,
) -> Result<(i32, f64)> {
    spawn_with_idle_watch_with_shutdown(
        cmd,
        log_path,
        worktree,
        idle_timeout_seconds,
        spawn_context,
        &SHUTDOWN_REQUESTED,
        false,
    )
}

fn spawn_with_idle_watch_with_shutdown(
    mut cmd: Command,
    log_path: &Path,
    worktree: &Path,
    idle_timeout_seconds: u64,
    spawn_context: &str,
    shutdown_requested: &AtomicBool,
    output_counts_as_progress: bool,
) -> Result<(i32, f64)> {
    let start = Instant::now();
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    prepare_process_group(&mut cmd);

    let mut child = cmd.spawn().with_context(|| spawn_context.to_string())?;
    let (progress_tx, progress_rx) = mpsc::channel();
    let stdout_thread = child.stdout.take().map(|stdout| {
        copy_stream_to_file(stdout, log_path.to_path_buf(), Some(progress_tx.clone()))
    });
    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| copy_stream_to_file(stderr, log_path.to_path_buf(), Some(progress_tx)));

    let idle_timeout = Duration::from_secs(idle_timeout_seconds);
    // A backend that emits no output is already indistinguishable from a
    // stalled launch after one idle window. Do not double the stall budget:
    // that kept three workers occupied for ten minutes before the controller
    // could reroute them.
    let startup_grace = idle_timeout;
    let poll_interval = Duration::from_millis(500);
    let worktree_poll_interval = Duration::from_secs(1);
    let mut last_seen_len = fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
    let mut last_worktree_snapshot = worktree_progress_snapshot(worktree);
    let process_group = child.id();
    let mut last_process_activity = process_group_activity_snapshot(process_group);
    let mut last_worktree_poll = Instant::now();
    let mut last_progress_at = Instant::now();
    let mut saw_progress = false;
    let mut killed_for_idle = false;
    let mut killed_for_shutdown = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                if shutdown_requested.load(Ordering::SeqCst) {
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    killed_for_shutdown = true;
                    break -2;
                }
                while progress_rx.try_recv().is_ok() {
                    let current_len = fs::metadata(log_path)
                        .map(|m| m.len())
                        .unwrap_or(last_seen_len);
                    if output_counts_as_progress {
                        last_progress_at = Instant::now();
                        saw_progress = true;
                    }
                    last_seen_len = current_len;
                }
                let current_len = fs::metadata(log_path)
                    .map(|m| m.len())
                    .unwrap_or(last_seen_len);
                if current_len != last_seen_len {
                    last_seen_len = current_len;
                    if output_counts_as_progress {
                        last_progress_at = Instant::now();
                        saw_progress = true;
                    }
                }
                if last_worktree_poll.elapsed() >= worktree_poll_interval {
                    if let Some(snapshot) = worktree_progress_snapshot(worktree) {
                        if last_worktree_snapshot.as_ref() != Some(&snapshot) {
                            last_worktree_snapshot = Some(snapshot);
                            last_progress_at = Instant::now();
                            saw_progress = true;
                        }
                    }
                    if let Some(activity) = process_group_activity_snapshot(process_group) {
                        if process_group_activity_advanced(
                            last_process_activity.as_deref(),
                            &activity,
                        ) {
                            last_progress_at = Instant::now();
                            saw_progress = true;
                        }
                        last_process_activity = Some(activity);
                    }
                    last_worktree_poll = Instant::now();
                }
                let stalled = if saw_progress {
                    last_progress_at.elapsed() >= idle_timeout
                } else {
                    start.elapsed() >= startup_grace
                };
                if stalled {
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    killed_for_idle = true;
                    break -1;
                }
                thread::sleep(poll_interval);
            }
            Err(_) => {
                // try_wait() itself erroring is rare, but the child may
                // still be alive -- kill and reap rather than risk leaking
                // it (same pattern as the idle-kill branch above).
                kill_process_group(&mut child);
                let _ = child.wait();
                break -1;
            }
        }
    };
    let duration = start.elapsed();
    if let Some(handle) = stdout_thread {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }

    if killed_for_idle {
        if let Ok(mut file) = fs::OpenOptions::new().append(true).open(log_path) {
            let progress_description = if output_counts_as_progress {
                "backend output or worktree progress"
            } else {
                "worktree progress"
            };
            let _ = writeln!(
                file,
                "GAH: killed after {idle_timeout_seconds}s with no new {progress_description} (stalled, not just slow)."
            );
        }
    }
    if killed_for_shutdown {
        if let Ok(mut file) = fs::OpenOptions::new().append(true).open(log_path) {
            let _ = writeln!(
                file,
                "GAH: shutdown requested; terminated backend process group."
            );
        }
    }

    Ok((exit_code, duration.as_secs_f64()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::{fixture, make_fake_bin};
    use crate::test_support::ExecGuard;

    #[test]
    fn idle_watch_terminates_process_group_on_shutdown_request() {
        let _exec_guard = ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "backend",
            "#!/bin/sh\necho 'started'\nsleep 30\necho 'should not complete'\n",
        );
        let shutdown = std::sync::Arc::new(AtomicBool::new(false));
        let trigger = std::sync::Arc::clone(&shutdown);
        let trigger_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            trigger.store(true, Ordering::SeqCst);
        });
        let log_path = f.session_dir.join("backend-output.log");

        let result = spawn_with_idle_watch_with_shutdown(
            Command::new(f.bin_dir.join("backend")),
            &log_path,
            &f.worktree,
            60,
            "launching test backend",
            &shutdown,
            true,
        )
        .unwrap();

        trigger_thread.join().unwrap();
        assert_eq!(result.0, -2);
        let log = fs::read_to_string(log_path).unwrap();
        assert!(log.contains("started"));
        assert!(!log.contains("should not complete"));
        assert!(log.contains("shutdown requested; terminated backend process group"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn idle_watch_allows_quiet_active_descendant_verification() {
        for attempt in 1..=8 {
            let _exec_guard = ExecGuard::new();
            let f = fixture();
            make_fake_bin(
                &f.bin_dir,
                "backend",
                "#!/bin/sh\n# Deterministic, quiet parent with active descendant.\n/bin/yes >/dev/null &\nbackend_descendant=$!\nsleep 2\n/bin/kill -TERM \"$backend_descendant\" >/dev/null 2>&1 || true\nwait \"$backend_descendant\" 2>/dev/null || true\n",
            );
            let log_path = f.session_dir.join("backend-output.log");
            let shutdown = AtomicBool::new(false);

            let result = spawn_with_idle_watch_with_shutdown(
                Command::new(f.bin_dir.join("backend")),
                &log_path,
                &f.worktree,
                1,
                "launching quiet verification backend",
                &shutdown,
                true,
            )
            .unwrap();

            assert_eq!(result.0, 0, "attempt {attempt}");
            assert!(result.1 >= 1.0, "attempt {attempt} ended too quickly");
            let log = fs::read_to_string(log_path).unwrap_or_default();
            assert!(
                !log.contains("GAH: killed after"),
                "attempt {attempt} got log: {log}"
            );
        }
    }

    #[test]
    fn process_activity_ignores_idle_child_churn_but_detects_real_work() {
        let previous = vec![(10, 1, 0, 20), (11, 0, 0, 0)];

        assert!(!process_group_activity_advanced(
            Some(&previous),
            &[(10, 1, 0, 20), (12, 0, 0, 0)]
        ));
        assert!(process_group_activity_advanced(
            Some(&previous),
            &[(10, 2, 0, 20)]
        ));
        assert!(process_group_activity_advanced(
            Some(&previous),
            &[(13, 1, 0, 0)]
        ));
    }
}
