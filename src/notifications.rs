//! Native notification hooks for `gah loop` / `gah dispatch`.
//!
//! Issue #84: an operator should not need a separate manager agent or an
//! external wrapper script to learn that GAH needs them. When a profile sets
//! `notify_command`, GAH pipes a single one-line message to that command's
//! stdin (shell-executed, exactly like `validation_commands`) on a small set
//! of high-signal controller/dispatch events:
//!
//!   * `HumanRequired` decided (reason + reference)
//!   * MR/PR created (url, work_id, backend/model)
//!   * Review verdict recorded (verdict + MR url)
//!   * Dispatch failed terminally (failure_class + work_id)
//!
//! Routine events (observation, wait, no-op) deliberately emit no
//! notification, to avoid spam. A failing or missing `notify_command` is
//! logged to stderr and swallowed -- it must never fail the dispatch/loop.
//!
//! The message-formatting logic is separated from shell execution so it can be
//! unit-tested without spawning a shell (see tests at the bottom of this file).
//!
//! Manager wake (operator ask, 2026-07-10): a Telegram ping alone still
//! requires a human to notice it and go start/resume a manager agent
//! session. When a profile explicitly opts in via `manager_wake_autonomy`
//! and `Defaults::current_manager` names a known agent CLI, GAH
//! additionally spawns that CLI headlessly (fire-and-forget, background)
//! with an instruction built from the same event -- so the next actionable
//! event (MR ready, human required, etc.) can get picked up without the
//! operator being the one to trigger it. `WakeAutonomy::Full` (unsupervised
//! merge authority) requires the operator to explicitly opt a specific
//! profile in -- it is never the default for a newly-added profile, and a
//! profile with no `manager_wake_autonomy` set behaves exactly as before
//! this feature existed.

use crate::config::{GahConfig, Profile, WakeAutonomy};

const NOTIFICATION_ERROR_SUMMARY_MAX_BYTES: usize = 300;

/// A notification-worthy controller/dispatch event. All fields are borrowed
/// from the live dispatch/controller state to avoid cloning.
pub enum NotifyEvent<'a> {
    /// A controller action was decided as `HumanRequired`.
    HumanRequired {
        reason: &'a str,
        reference: Option<&'a str>,
        /// TICKET-505: stable reason code for why autonomy stopped.
        reason_code: Option<&'a str>,
        failure_class: &'a str,
        failure_stage: Option<&'a str>,
        error_summary: Option<&'a str>,
        attempt_count: Option<u32>,
        mr_url: Option<&'a str>,
    },
    /// A draft MR/PR was created/pushed.
    MrCreated {
        url: &'a str,
        work_id: &'a str,
        backend: &'a str,
        model: &'a str,
    },
    /// A review verdict was recorded.
    ReviewVerdict { verdict: &'a str, mr_url: &'a str },
    /// TICKET-127: an MR/PR was auto-merged.
    MrMerged { url: &'a str, work_id: &'a str },
    /// A dispatch failed terminally (retries exhausted).
    DispatchFailed {
        failure_class: &'a str,
        failure_stage: Option<&'a str>,
        work_id: &'a str,
        attempt_count: Option<u32>,
        error_summary: Option<&'a str>,
        mr_url: Option<&'a str>,
    },
    /// A backend was killed by GAH's idle watchdog. This is actionable even
    /// when the dispatch still has another route to try.
    BackendStalled {
        work_id: &'a str,
        backend: &'a str,
        model: &'a str,
        duration_seconds: f64,
    },
}

/// Render a `backend`/`model` pair for a human-facing message. Some
/// backends (opencode) name their own models with the backend as a
/// namespace prefix already (e.g. `opencode/hy3-free`) -- naively
/// prepending `{backend}/` again produced `opencode/opencode/hy3-free`
/// (live-observed). Collapses to just `model` when it already starts with
/// `{backend}/`.
fn route_label(backend: &str, model: &str) -> String {
    if model.starts_with(&format!("{backend}/")) {
        model.to_string()
    } else {
        format!("{backend}/{model}")
    }
}

/// Render a `NotifyEvent` into the single-line message GAH pipes to
/// `notify_command`. Pure and allocation-light; unit-tested below.
pub fn format_message(event: &NotifyEvent) -> String {
    match event {
        NotifyEvent::HumanRequired {
            reason,
            reference,
            reason_code,
            failure_class,
            failure_stage,
            error_summary,
            attempt_count,
            mr_url,
        } => {
            let mut msg = format!(
                "[gah] human required: {reason} [class={failure_class}] [stage={}] [attempts={}]",
                failure_stage.unwrap_or("unknown"),
                format_attempt_count(*attempt_count),
            );
            if let Some(reference) = reference.or(*mr_url) {
                msg.push_str(&format!(" ({reference})"));
            }
            if let Some(code) = reason_code {
                msg.push_str(&format!(" [code={code}]"));
            }
            if let Some(summary) = summarize_error_summary(*error_summary) {
                msg.push_str(&format!(" summary={summary}"));
            }
            msg
        }
        NotifyEvent::MrCreated {
            url,
            work_id,
            backend,
            model,
        } => {
            format!(
                "[gah] MR created {url} (work_id={work_id}, {})",
                route_label(backend, model)
            )
        }
        NotifyEvent::ReviewVerdict { verdict, mr_url } => {
            format!("[gah] review {verdict} on {mr_url}")
        }
        NotifyEvent::MrMerged { url, work_id } => {
            format!("[gah] auto-merged {url} (work_id={work_id})")
        }
        NotifyEvent::DispatchFailed {
            failure_class,
            failure_stage,
            work_id,
            attempt_count,
            error_summary,
            mr_url,
        } => {
            let mut msg = format!(
                "[gah] dispatch failed [class={failure_class}] [stage={}] [attempts={}] work_id={work_id}",
                failure_stage.unwrap_or("unknown"),
                format_attempt_count(*attempt_count),
            );
            if let Some(mr_url) = mr_url {
                msg.push_str(&format!(" ref={mr_url}"));
            }
            if let Some(summary) = summarize_error_summary(*error_summary) {
                msg.push_str(&format!(" summary={summary}"));
            }
            msg
        }
        NotifyEvent::BackendStalled {
            work_id,
            backend,
            model,
            duration_seconds,
        } => format!(
            "[gah] backend stalled work_id={work_id} route={} duration={duration_seconds:.0}s; rerouting",
            route_label(backend, model)
        ),
    }
}

/// Execute `command` via `sh -c`, piping `message` (plus a trailing newline)
/// to its stdin. Any failure (missing executable, non-zero exit, spawn error)
/// is returned so the caller can swallow it -- this function never aborts the
/// surrounding dispatch/loop on its own.
fn run_notify_command(command: &str, message: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Write the one-line message to stdin. A broken pipe here just means the
    // command exited before reading; treat it as non-fatal.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(message.as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let _ = child.wait()?;
    Ok(())
}

/// Build the instruction text for a woken manager agent, given the same
/// event that drove `format_message` and the profile's configured
/// autonomy. `None` means no wake should happen: autonomy is `Off`, or the
/// event has nothing left to act on (e.g. `MrMerged` already resolved
/// itself, there's no decision left to make).
pub fn format_wake_instruction(event: &NotifyEvent, autonomy: WakeAutonomy) -> Option<String> {
    let action = match autonomy {
        WakeAutonomy::Off => return None,
        WakeAutonomy::Full => {
            "Review this yourself against this repo's standing manager authorization (see CLAUDE.md / manager memory). If it's a PR ready to merge, review the diff and merge it if CI is green and review passed. If it's a failure or a block, investigate and fix or escalate. Stay scoped to this profile/repo only -- no destructive git operations, no unrelated scope creep. Report a one-line outcome at the end."
        }
        WakeAutonomy::ReviewOnly => {
            "Review this and post your findings as a comment on the relevant PR/issue. Do not merge or take any other write action -- a human will act on your review."
        }
    };

    let context = match event {
        NotifyEvent::MrCreated {
            url,
            work_id,
            backend,
            model,
        } => format!(
            "A draft PR/MR is ready: {url} (work_id={work_id}, dispatched via {}).",
            route_label(backend, model)
        ),
        NotifyEvent::HumanRequired {
            reason,
            reference,
            reason_code,
            failure_class,
            failure_stage,
            error_summary,
            attempt_count,
            mr_url,
        } => {
            let mut context = format!(
                "gah loop is blocked and needs judgment: [class={failure_class}] [stage={}] [attempts={}] {reason}.",
                failure_stage.unwrap_or("unknown"),
                format_attempt_count(*attempt_count),
            );
            if let Some(code) = reason_code {
                context.push_str(&format!(" [code={code}]"));
            }
            if let Some(reference) = reference.or(*mr_url) {
                context.push_str(&format!(" Reference: {reference}."));
            }
            if let Some(summary) = summarize_error_summary(*error_summary) {
                context.push_str(&format!(" summary={summary}"));
            }
            context
        }
        NotifyEvent::ReviewVerdict { verdict, mr_url } => {
            format!("A review verdict was recorded: {verdict} on {mr_url}.")
        }
        NotifyEvent::DispatchFailed {
            failure_class,
            failure_stage,
            work_id,
            attempt_count,
            error_summary,
            mr_url,
        } => {
            let mut context = format!(
                "A dispatch failed terminally: [class={failure_class}] [stage={}] [attempts={}] work_id={work_id}.",
                failure_stage.unwrap_or("unknown"),
                format_attempt_count(*attempt_count),
            );
            if let Some(mr_url) = mr_url {
                context.push_str(&format!(" ref={mr_url}"));
            }
            if let Some(summary) = summarize_error_summary(*error_summary) {
                context.push_str(&format!(" summary={summary}"));
            }
            context
        }
        NotifyEvent::BackendStalled {
            work_id,
            backend,
            model,
            duration_seconds,
        } => format!(
            "Backend stalled for work_id={work_id} on {} after {duration_seconds:.0}s; GAH is rerouting.",
            route_label(backend, model)
        ),
        // Already resolved -- nothing for a woken agent to act on.
        NotifyEvent::MrMerged { .. } => return None,
    };

    Some(format!("[gah manager wake] {context} {action}"))
}

/// Spawn the configured manager CLI headlessly (fire-and-forget, background)
/// with `instruction` passed as a single argv argument -- never shell-
/// interpolated, so there is no quoting/injection concern. stdout/stderr are
/// captured to a timestamped audit log under `log_dir` (see
/// `Defaults::manager_wake_log_dir`), not discarded, so a wake is always
/// inspectable after the fact. Live-hit: the first real wake (autonomy Full)
/// ran completely unobserved -- stdout/stderr both went to `/dev/null`, so
/// there was no way to see what an unsupervised headless agent instance
/// actually did or why it might be making network connections, right when
/// an operator was specifically asking about unexpected outbound traffic.
/// Any failure to spawn is logged to stderr and swallowed, exactly like
/// `run_notify_command`: this must never fail the caller's dispatch/loop.
fn spawn_manager_wake(manager: &str, instruction: &str, log_dir: &std::path::Path) {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};

    fn copy_redacted_manager_output<R: Read>(mut reader: R, log: Arc<Mutex<std::fs::File>>) {
        let mut buf = [0_u8; 8192];
        let mut pending = Vec::new();
        while let Ok(read) = reader.read(&mut buf) {
            if read == 0 {
                break;
            }
            pending.extend_from_slice(&buf[..read]);
            while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<_> = pending.drain(..=newline).collect();
                if let Ok(mut file) = log.lock() {
                    let _ = file.write_all(
                        crate::redact::redact(&String::from_utf8_lossy(&line)).as_bytes(),
                    );
                }
            }
        }
        if !pending.is_empty() {
            if let Ok(mut file) = log.lock() {
                let _ = file.write_all(
                    crate::redact::redact(&String::from_utf8_lossy(&pending)).as_bytes(),
                );
            }
        }
    }

    let mut cmd = match manager {
        "claude" => {
            let mut c = Command::new("claude");
            c.arg("-p").arg(instruction);
            c
        }
        "codex" => {
            let mut c = Command::new("codex");
            c.arg("exec").arg(instruction);
            c
        }
        "hermes" => {
            let mut c = Command::new("hermes");
            c.args([
                "-p",
                "gah-manager",
                "chat",
                "--worktree",
                "-Q",
                "-q",
                instruction,
            ]);
            c
        }
        other => {
            eprintln!("[gah] manager_wake: unknown current_manager '{other}', skipping wake");
            return;
        }
    };

    if let Err(err) = std::fs::create_dir_all(log_dir) {
        eprintln!("[gah] manager_wake: failed to create log dir (swallowed): {err:#}");
    }
    let ts = time::OffsetDateTime::now_utc().unix_timestamp();
    let log_path = log_dir.join(format!("{ts}-{}-{manager}.log", std::process::id()));

    let log_file = match std::fs::File::create(&log_path) {
        Ok(mut log_file) => {
            let _ = writeln!(log_file, "instruction: {instruction}\n---");
            Some(Arc::new(Mutex::new(log_file)))
        }
        Err(err) => {
            eprintln!(
                "[gah] manager_wake: failed to open log file {} (swallowed, output will be discarded): {err:#}",
                log_path.display()
            );
            None
        }
    };
    cmd.stdin(Stdio::null());
    if log_file.is_some() {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }

    // Deliberately fire-and-forget: this must not block the dispatch/loop
    // that triggered it. But `gah loop` (unlike a one-shot `--once` call) is
    // a long-running daemon that can keep running for hours after this --
    // it does NOT exit soon after, so the child is never reparented to init
    // for reaping. Dropping the `Child` handle without ever waiting on it
    // left it as a `[claude] <defunct>` zombie, owned by the still-running
    // gah loop process, until that process itself eventually exited. A
    // background thread that just waits on it keeps this non-blocking for
    // the caller while still reaping the child whenever it finishes.
    match cmd.spawn() {
        Ok(mut child) => {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            std::thread::spawn(move || {
                let stdout_thread = stdout.and_then(|stream| {
                    log_file.as_ref().map(|log| {
                        let log = Arc::clone(log);
                        std::thread::spawn(move || copy_redacted_manager_output(stream, log))
                    })
                });
                let stderr_thread = stderr.and_then(|stream| {
                    log_file.as_ref().map(|log| {
                        let log = Arc::clone(log);
                        std::thread::spawn(move || copy_redacted_manager_output(stream, log))
                    })
                });
                let _ = child.wait();
                if let Some(thread) = stdout_thread {
                    let _ = thread.join();
                }
                if let Some(thread) = stderr_thread {
                    let _ = thread.join();
                }
            });
        }
        Err(err) => {
            eprintln!("[gah] manager_wake: failed to spawn '{manager}' (swallowed): {err:#}");
        }
    }
}

fn format_attempt_count(attempt_count: Option<u32>) -> String {
    attempt_count.map_or_else(|| "unknown".to_string(), |count| count.to_string())
}

fn summarize_error_summary(summary: Option<&str>) -> Option<String> {
    summary.and_then(|summary| {
        let sanitized = crate::redact::redact(&strip_ansi(summary));
        // Notification hooks promise one logical line. Backend errors often
        // contain command output with newlines/tabs; collapse all whitespace
        // before truncation so one failure cannot forge extra alert lines.
        let sanitized = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
        let sanitized = sanitized.trim();
        if sanitized.is_empty() {
            return None;
        }
        let truncated =
            crate::dispatch::utf8_safe_prefix(sanitized, NOTIFICATION_ERROR_SUMMARY_MAX_BYTES);
        let truncated = truncated.trim();
        if truncated.is_empty() {
            None
        } else {
            Some(truncated.to_string())
        }
    })
}

fn strip_ansi(text: &str) -> String {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    let osc = regex::Regex::new(r"\x1b\].*?(?:\x07|\x1b\\)").unwrap();
    let ansi_text = ansi.replace_all(text, "");
    osc.replace_all(&ansi_text, "").into_owned()
}

/// Fire a notification for `event`: the existing `notify_command` hook (if
/// the profile defines one), and additionally wake the configured manager
/// agent (if the profile opts in via `manager_wake_autonomy` and
/// `cfg.defaults.current_manager` names a known agent CLI).
///
/// This is the single public entry point. It is infallible by design: any
/// error from either path is logged to stderr and swallowed so the
/// caller's flow continues exactly as if no hook existed.
pub fn notify_event(cfg: &GahConfig, profile: &Profile, event: NotifyEvent) {
    if let Some(command) = &profile.notify_command {
        let message = crate::redact::redact(&format_message(&event));
        if let Err(err) = run_notify_command(command, &message) {
            eprintln!("[gah] notify_command failed (swallowed): {err:#}");
        }
    }

    if let Some(instruction) = format_wake_instruction(&event, profile.manager_wake_autonomy) {
        let instruction = crate::redact::redact(&instruction);
        if let Some(manager) = cfg.defaults.current_manager.as_deref() {
            spawn_manager_wake(manager, &instruction, &cfg.defaults.manager_wake_log_dir());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_required_includes_reason_and_reference() {
        let msg = format_message(&NotifyEvent::HumanRequired {
            reason: "MR ready for human decision",
            reference: Some("https://github.com/owner/repo/pull/7"),
            reason_code: Some("merge_policy"),
            failure_class: "human_blocked",
            failure_stage: Some("review"),
            error_summary: None,
            attempt_count: Some(2),
            mr_url: None,
        });
        assert_eq!(
            msg,
            "[gah] human required: MR ready for human decision [class=human_blocked] [stage=review] [attempts=2] (https://github.com/owner/repo/pull/7) [code=merge_policy]"
        );
    }

    #[test]
    fn human_required_without_reference() {
        let msg = format_message(&NotifyEvent::HumanRequired {
            reason: "waiting on operator",
            reference: None,
            reason_code: None,
            failure_class: "human_blocked",
            failure_stage: Some("review"),
            error_summary: None,
            attempt_count: Some(2),
            mr_url: Some("https://github.com/owner/repo/branch/feature"),
        });
        assert_eq!(
            msg,
            "[gah] human required: waiting on operator [class=human_blocked] [stage=review] [attempts=2] (https://github.com/owner/repo/branch/feature)"
        );
    }

    #[test]
    fn mr_created_includes_url_work_id_and_route() {
        let msg = format_message(&NotifyEvent::MrCreated {
            url: "https://example.com/mr/1",
            work_id: "WORK-X",
            backend: "agy",
            model: "opus",
        });
        assert_eq!(
            msg,
            "[gah] MR created https://example.com/mr/1 (work_id=WORK-X, agy/opus)"
        );
    }

    #[test]
    fn mr_created_collapses_duplicate_backend_prefix_in_model_name() {
        // Live-observed: opencode's own model names already carry
        // "opencode/" as a namespace prefix (e.g. "opencode/hy3-free"),
        // producing "opencode/opencode/hy3-free" if backend/model were
        // naively concatenated.
        let msg = format_message(&NotifyEvent::MrCreated {
            url: "https://example.com/mr/1",
            work_id: "WORK-X",
            backend: "opencode",
            model: "opencode/hy3-free",
        });
        assert_eq!(
            msg,
            "[gah] MR created https://example.com/mr/1 (work_id=WORK-X, opencode/hy3-free)"
        );
    }

    #[test]
    fn review_verdict_includes_verdict_and_url() {
        let msg = format_message(&NotifyEvent::ReviewVerdict {
            verdict: "APPROVE",
            mr_url: "https://example.com/mr/2",
        });
        assert_eq!(msg, "[gah] review APPROVE on https://example.com/mr/2");
    }

    #[test]
    fn dispatch_failed_includes_failure_class_and_work_id() {
        let msg = format_message(&NotifyEvent::DispatchFailed {
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            work_id: "WORK-Y",
            attempt_count: Some(3),
            error_summary: None,
            mr_url: Some("https://example.com/mr/4"),
        });
        assert_eq!(
            msg,
            "[gah] dispatch failed [class=validation_failure] [stage=agent_run] [attempts=3] work_id=WORK-Y ref=https://example.com/mr/4"
        );
    }

    #[test]
    fn dispatch_failed_without_summary_renders_without_none() {
        let msg = format_message(&NotifyEvent::DispatchFailed {
            failure_class: "unknown",
            failure_stage: None,
            work_id: "WORK-Z",
            attempt_count: None,
            error_summary: None,
            mr_url: None,
        });
        assert_eq!(
            msg,
            "[gah] dispatch failed [class=unknown] [stage=unknown] [attempts=unknown] work_id=WORK-Z"
        );
        assert!(!msg.contains("None"));
    }

    #[test]
    fn dispatch_failed_truncates_and_strips_ansi_from_summary() {
        let long_summary = format!(
            "\u{1b}[31m{}\nsecond\tline\rthird\u{1b}[0m",
            "x".repeat(400)
        );
        let msg = format_message(&NotifyEvent::DispatchFailed {
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            work_id: "WORK-Y",
            attempt_count: None,
            error_summary: Some(&long_summary),
            mr_url: None,
        });
        let summary = msg
            .split(" summary=")
            .nth(1)
            .expect("summary field should be present");
        assert!(!summary.contains('\u{1b}'));
        assert!(!summary.contains(['\n', '\r', '\t']));
        assert!(summary.len() <= 300);
    }

    fn test_gah_config(current_manager: Option<&str>) -> GahConfig {
        GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: current_manager.map(String::from),
                ..Default::default()
            },
            profiles: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn notify_event_is_a_noop_when_command_unset() {
        // No command -> no spawn, no panic, no output.
        let profile = crate::config::tests::test_profile_for_notifications();
        let cfg = test_gah_config(None);
        notify_event(
            &cfg,
            &profile,
            NotifyEvent::HumanRequired {
                reason: "x",
                reference: None,
                reason_code: None,
                failure_class: "human_blocked",
                failure_stage: Some("review"),
                error_summary: None,
                attempt_count: None,
                mr_url: None,
            },
        );
    }

    #[test]
    fn notify_event_pipes_message_and_swallows_failure() {
        // A real capture command must receive the one-line message; a missing
        // command must be swallowed rather than propagated (verified by the
        // noop test above). Order: write to a temp file via `cat > out`.
        let out = std::env::temp_dir().join(format!("gah-notify-test-{}.txt", std::process::id()));
        let command = format!("cat > {}", out.display());
        let mut profile = crate::config::tests::test_profile_for_notifications();
        profile.notify_command = Some(command);
        let cfg = test_gah_config(None);
        notify_event(
            &cfg,
            &profile,
            NotifyEvent::MrCreated {
                url: "https://example.com/mr/1",
                work_id: "WORK-X",
                backend: "agy",
                model: "opus",
            },
        );
        let got = std::fs::read_to_string(&out).unwrap_or_default();
        std::fs::remove_file(&out).ok();
        assert!(
            got.contains("[gah] MR created https://example.com/mr/1 (work_id=WORK-X, agy/opus)")
        );
    }

    // ── format_wake_instruction ──────────────────────────────────────────

    #[test]
    fn wake_instruction_is_none_when_autonomy_off() {
        for event in [
            NotifyEvent::HumanRequired {
                reason: "x",
                reference: None,
                reason_code: None,
                failure_class: "human_blocked",
                failure_stage: Some("review"),
                error_summary: None,
                attempt_count: None,
                mr_url: None,
            },
            NotifyEvent::MrCreated {
                url: "u",
                work_id: "w",
                backend: "b",
                model: "m",
            },
            NotifyEvent::ReviewVerdict {
                verdict: "APPROVE",
                mr_url: "u",
            },
            NotifyEvent::DispatchFailed {
                failure_class: "c",
                failure_stage: Some("agent_run"),
                work_id: "w",
                attempt_count: Some(1),
                error_summary: None,
                mr_url: None,
            },
        ] {
            assert!(format_wake_instruction(&event, WakeAutonomy::Off).is_none());
        }
    }

    #[test]
    fn wake_instruction_is_none_for_mr_merged_regardless_of_autonomy() {
        let event = NotifyEvent::MrMerged {
            url: "u",
            work_id: "w",
        };
        assert!(format_wake_instruction(&event, WakeAutonomy::Full).is_none());
        assert!(format_wake_instruction(&event, WakeAutonomy::ReviewOnly).is_none());
    }

    #[test]
    fn wake_instruction_full_includes_merge_authorization() {
        let event = NotifyEvent::MrCreated {
            url: "https://example.com/mr/1",
            work_id: "WORK-X",
            backend: "agy",
            model: "opus",
        };
        let instruction = format_wake_instruction(&event, WakeAutonomy::Full).unwrap();
        assert!(instruction.contains("https://example.com/mr/1"));
        assert!(instruction.contains("merge it if CI is green"));
    }

    #[test]
    fn wake_instruction_collapses_duplicate_backend_prefix_in_model_name() {
        let event = NotifyEvent::MrCreated {
            url: "https://example.com/mr/1",
            work_id: "WORK-X",
            backend: "opencode",
            model: "opencode/hy3-free",
        };
        let instruction = format_wake_instruction(&event, WakeAutonomy::Full).unwrap();
        assert!(instruction.contains("dispatched via opencode/hy3-free"));
        assert!(!instruction.contains("opencode/opencode"));
    }

    #[test]
    fn wake_instruction_review_only_forbids_merge() {
        let event = NotifyEvent::MrCreated {
            url: "https://example.com/mr/1",
            work_id: "WORK-X",
            backend: "agy",
            model: "opus",
        };
        let instruction = format_wake_instruction(&event, WakeAutonomy::ReviewOnly).unwrap();
        assert!(instruction.contains("https://example.com/mr/1"));
        assert!(instruction.contains("Do not merge"));
    }

    #[test]
    fn wake_instruction_human_required_includes_reason_and_reference() {
        let event = NotifyEvent::HumanRequired {
            reason: "MR ready for human decision",
            reference: Some("https://example.com/mr/7"),
            reason_code: Some("merge_policy"),
            failure_class: "human_blocked",
            failure_stage: Some("review"),
            error_summary: None,
            attempt_count: Some(1),
            mr_url: None,
        };
        let instruction = format_wake_instruction(&event, WakeAutonomy::Full).unwrap();
        assert!(instruction.contains("MR ready for human decision"));
        assert!(instruction.contains("https://example.com/mr/7"));
        assert!(instruction.contains("[code=merge_policy]"));
    }

    // ── manager wake integration (real spawn via a fake `claude` binary) ──

    fn make_fake_wake_bin(dir: &std::path::Path, name: &str, capture: &std::path::Path) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$2\" > '{}'\n",
                capture.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
    }

    #[test]
    fn notify_event_wakes_configured_manager_when_autonomy_set() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        let capture = tmp.path().join("captured-instruction.txt");
        make_fake_wake_bin(&bin_dir, "claude", &capture);
        let _path_guard = crate::test_support::PathGuard::set(&bin_dir);

        let mut profile = crate::config::tests::test_profile_for_notifications();
        profile.manager_wake_autonomy = WakeAutonomy::Full;
        let mut cfg = test_gah_config(Some("claude"));
        cfg.defaults.artifact_root = tmp.path().to_string_lossy().to_string();

        notify_event(
            &cfg,
            &profile,
            NotifyEvent::MrCreated {
                url: "https://example.com/mr/9",
                work_id: "WORK-9",
                backend: "codex",
                model: "gpt",
            },
        );

        // Fire-and-forget: give the fake binary a moment to write its capture.
        for _ in 0..50 {
            if capture.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let got = std::fs::read_to_string(&capture).unwrap_or_default();
        assert!(
            got.contains("https://example.com/mr/9"),
            "expected woken claude to receive the instruction, got: {got:?}"
        );

        // The audit log (this fix's whole point) must exist and contain the
        // instruction -- a wake must never again be unobservable.
        let log_dir = cfg.defaults.manager_wake_log_dir();
        let log_entries: Vec<_> = std::fs::read_dir(&log_dir)
            .unwrap_or_else(|err| panic!("expected log dir {log_dir:?} to exist: {err:#}"))
            .collect();
        assert_eq!(
            log_entries.len(),
            1,
            "expected exactly one wake log file in {log_dir:?}"
        );
        let log_contents = std::fs::read_to_string(log_entries[0].as_ref().unwrap().path())
            .expect("read wake log file");
        assert!(
            log_contents.contains("https://example.com/mr/9"),
            "expected wake log to record the instruction, got: {log_contents:?}"
        );
    }

    /// Regression: the spawned manager-wake child must actually be reaped,
    /// not left as a `[claude] <defunct>` zombie under the still-running
    /// caller. The fake binary reports its own pid before exiting; once it
    /// has exited we poll `/proc/<pid>` -- a reaped process's entry
    /// disappears entirely, while an un-waited zombie keeps a `Z` (zombie)
    /// stat entry around indefinitely (until *this test process* exits).
    #[test]
    #[cfg(target_os = "linux")]
    fn notify_event_manager_wake_does_not_leave_a_zombie() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let pid_file = tmp.path().join("child.pid");
        let bin_path = bin_dir.join("claude");
        std::fs::write(
            &bin_path,
            format!("#!/bin/sh\necho $$ > '{}'\nexit 0\n", pid_file.display()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&bin_path, perms).unwrap();
        }
        let _path_guard = crate::test_support::PathGuard::set(&bin_dir);

        let mut profile = crate::config::tests::test_profile_for_notifications();
        profile.manager_wake_autonomy = WakeAutonomy::Full;
        let mut cfg = test_gah_config(Some("claude"));
        cfg.defaults.artifact_root = tmp.path().to_string_lossy().to_string();

        notify_event(
            &cfg,
            &profile,
            NotifyEvent::MrCreated {
                url: "https://example.com/mr/9",
                work_id: "WORK-9",
                backend: "codex",
                model: "gpt",
            },
        );

        // Wait for the fake binary to report its pid and exit.
        let mut pid = None;
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                if let Ok(p) = text.trim().parse::<i32>() {
                    pid = Some(p);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let pid = pid.expect("fake claude binary never reported its pid");

        // Give the background reaper thread a moment to call wait() after
        // the child exits, then confirm the process is fully gone -- not
        // lingering as a zombie (stat state 'Z').
        let proc_path = format!("/proc/{pid}");
        let mut reaped = false;
        for _ in 0..100 {
            match std::fs::read_to_string(format!("{proc_path}/stat")) {
                Ok(stat) if stat.contains(") Z ") => {
                    // still a zombie, keep polling
                }
                Ok(_) => {
                    // Unlikely (would mean the pid got reused already), but
                    // not a zombie either way -- treat as reaped.
                    reaped = true;
                    break;
                }
                Err(_) => {
                    reaped = true; // /proc entry gone entirely: fully reaped
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            reaped,
            "expected child pid {pid} to be reaped (no zombie), but {proc_path} is still a zombie"
        );
    }

    #[test]
    fn notify_event_does_not_wake_when_autonomy_off() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        let capture = tmp.path().join("captured-instruction.txt");
        make_fake_wake_bin(&bin_dir, "claude", &capture);
        let _path_guard = crate::test_support::PathGuard::set(&bin_dir);

        // Default profile has manager_wake_autonomy == Off.
        let profile = crate::config::tests::test_profile_for_notifications();
        let cfg = test_gah_config(Some("claude"));

        notify_event(
            &cfg,
            &profile,
            NotifyEvent::MrCreated {
                url: "https://example.com/mr/9",
                work_id: "WORK-9",
                backend: "codex",
                model: "gpt",
            },
        );

        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            !capture.exists(),
            "autonomy Off must never spawn the manager wake"
        );
    }

    #[test]
    fn notify_event_does_not_wake_when_current_manager_unset() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        let capture = tmp.path().join("captured-instruction.txt");
        make_fake_wake_bin(&bin_dir, "claude", &capture);
        let _path_guard = crate::test_support::PathGuard::set(&bin_dir);

        let mut profile = crate::config::tests::test_profile_for_notifications();
        profile.manager_wake_autonomy = WakeAutonomy::Full;
        // No current_manager configured at all.
        let cfg = test_gah_config(None);

        notify_event(
            &cfg,
            &profile,
            NotifyEvent::MrCreated {
                url: "https://example.com/mr/9",
                work_id: "WORK-9",
                backend: "codex",
                model: "gpt",
            },
        );

        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            !capture.exists(),
            "autonomy set but no current_manager configured must not spawn a wake"
        );
    }
}
