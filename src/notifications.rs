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
use crate::events;
use serde_json::json;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const NOTIFICATION_ERROR_SUMMARY_MAX_BYTES: usize = 300;
const TERMINAL_FAILURE_EVENT_DEDUPE_SECONDS: i64 = 900;
const TERMINAL_FAILURE_EVENT_DEDUPE_SECONDS_ENV: &str = "GAH_TERMINAL_FAILURE_DEDUPE_SECONDS";

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
    /// A reviewer completed but did not provide machine-safe repair context.
    /// This is an automatic reroute signal, not a terminal human request.
    ReviewOutputInvalid {
        mr_url: &'a str,
        backend: &'a str,
        model: &'a str,
        reason: &'a str,
    },
    /// TICKET-127: an MR/PR was auto-merged.
    MrMerged { url: &'a str, work_id: &'a str },
    /// A dispatch failed terminally (retries exhausted).
    DispatchFailed {
        timestamp: &'a str,
        profile: &'a str,
        failure_class: &'a str,
        failure_stage: Option<&'a str>,
        run_id: &'a str,
        work_id: &'a str,
        attempt_count: Option<u32>,
        error_summary: Option<&'a str>,
        mr_url: Option<&'a str>,
    },
    /// A terminal dispatch failure for the same `(profile, work_id)` was resolved
    /// by later merge/close/reconcile activity.
    DispatchFailureResolved {
        timestamp: &'a str,
        profile: &'a str,
        failure_class: &'a str,
        failure_stage: Option<&'a str>,
        work_id: &'a str,
        run_id: &'a str,
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
        NotifyEvent::ReviewOutputInvalid {
            mr_url,
            backend,
            model,
            reason,
        } => format!(
            "[gah] review output invalid on {mr_url} route={} summary={}; rerouting",
            route_label(backend, model),
            summarize_error_summary(Some(reason))
                .unwrap_or_else(|| "invalid structured review output".to_string())
        ),
        NotifyEvent::MrMerged { url, work_id } => {
            format!("[gah] auto-merged {url} (work_id={work_id})")
        }
        NotifyEvent::DispatchFailed {
            timestamp,
            profile,
            failure_class,
            failure_stage,
            run_id,
            work_id,
            attempt_count,
            error_summary,
            mr_url,
        } => {
            let mut msg = format!(
                "[gah] dispatch terminal failure [ts={timestamp}] [profile={profile}] [class={failure_class}] [stage={}] [run_id={run_id}] [attempts={}] work_id={work_id}",
                failure_stage.unwrap_or("unknown"),
                format_attempt_count(*attempt_count),
            );
            if let Some(mr_url) = mr_url {
                msg.push_str(&format!(" ref={mr_url}"));
            }
            if *failure_class == "human_blocked" && failure_stage == &Some("route") {
                msg.push_str(" [state=paused_non_spending]");
            }
            if let Some(summary) = summarize_error_summary(*error_summary) {
                msg.push_str(&format!(" summary={summary}"));
            }
            msg
        }
        NotifyEvent::DispatchFailureResolved {
            timestamp,
            profile,
            failure_class,
            failure_stage,
            work_id,
            run_id,
        } => format!(
            "[gah] terminal failure resolved [ts={timestamp}] [profile={profile}] [class={failure_class}] [stage={}] [run_id={run_id}] work_id={work_id}",
            failure_stage.unwrap_or("unknown"),
        ),
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
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "notify command exited with status: {status}"
        )))
    }
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
        // The controller owns this bounded reroute. Waking a manager for each
        // malformed intermediate opinion would recreate the notification
        // spam this event is designed to explain.
        NotifyEvent::ReviewOutputInvalid { .. } => return None,
        NotifyEvent::DispatchFailed {
            failure_class,
            failure_stage,
            work_id,
            attempt_count,
            error_summary,
            mr_url,
            ..
        } => {
            let mut context = format!(
                "A dispatch failed terminally: [class={failure_class}] [stage={}] [attempts={}] work_id={work_id}.",
                failure_stage.unwrap_or("unknown"),
                format_attempt_count(*attempt_count),
            );
            if *failure_class == "human_blocked" && failure_stage == &Some("route") {
                context.push_str(" [state=paused_non_spending]");
            }
            if let Some(mr_url) = mr_url {
                context.push_str(&format!(" ref={mr_url}"));
            }
            if let Some(summary) = summarize_error_summary(*error_summary) {
                context.push_str(&format!(" summary={summary}"));
            }
            context
        }
        NotifyEvent::DispatchFailureResolved { .. } => return None,
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

#[derive(Debug)]
struct TerminalFailureRecord {
    _profile: String,
    _work_id: String,
    run_id: String,
    failure_class: String,
    failure_stage: Option<String>,
    attempt_count: Option<u32>,
    error_summary: Option<String>,
    timestamp: String,
}

#[derive(Debug)]
struct TerminalFailureState<'a> {
    profile: &'a str,
    work_id: &'a str,
    run_id: &'a str,
    failure_class: &'a str,
    failure_stage: Option<&'a str>,
    attempt_count: Option<u32>,
    error_summary: Option<&'a str>,
    timestamp: &'a str,
    mr_url: Option<&'a str>,
}

#[derive(Debug)]
pub(crate) struct TerminalFailurePayload<'a> {
    pub(crate) profile: &'a str,
    pub(crate) work_id: &'a str,
    pub(crate) run_id: &'a str,
    pub(crate) failure_class: &'a str,
    pub(crate) failure_stage: Option<&'a str>,
    pub(crate) attempt_count: Option<u32>,
    pub(crate) error_summary: Option<&'a str>,
    pub(crate) mr_url: Option<&'a str>,
}

fn terminal_failure_dedupe_window() -> time::Duration {
    let configured = std::env::var(TERMINAL_FAILURE_EVENT_DEDUPE_SECONDS_ENV)
        .ok()
        .and_then(|value| value.parse::<i64>().ok());
    time::Duration::seconds(configured.unwrap_or(TERMINAL_FAILURE_EVENT_DEDUPE_SECONDS))
}

fn terminal_failure_timestamp_now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn parse_terminal_failure_timestamp(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw, &Rfc3339).ok().or_else(|| {
        raw.parse::<i64>()
            .ok()
            .and_then(|secs| OffsetDateTime::from_unix_timestamp(secs).ok())
    })
}

fn terminal_failure_from_event(event: &events::ControllerEvent) -> Option<TerminalFailureRecord> {
    let value = serde_json::from_str::<serde_json::Value>(&event.details).ok()?;
    Some(TerminalFailureRecord {
        _profile: value
            .get("profile")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        _work_id: value
            .get("work_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        run_id: value
            .get("run_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        failure_class: value
            .get("failure_class")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        failure_stage: value
            .get("failure_stage")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        attempt_count: value
            .get("attempt_count")
            .and_then(|v| v.as_u64().and_then(|v| v.try_into().ok())),
        error_summary: value
            .get("error_summary")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        timestamp: event.timestamp.clone(),
    })
}

fn latest_unresolved_terminal_failure_record(
    cfg: &GahConfig,
    profile: &str,
    work_id: &str,
) -> Option<TerminalFailureRecord> {
    let events = events::read_events(cfg).ok()?;
    for event in events.iter().rev() {
        if event.profile.as_deref() != Some(profile) || event.work_id.as_deref() != Some(work_id) {
            continue;
        }
        if event.event_type == events::EventType::TerminalFailureResolved.as_str() {
            return None;
        }
        if event.event_type == events::EventType::TerminalFailure.as_str() {
            return terminal_failure_from_event(event);
        }
    }
    None
}

fn should_emit_terminal_failure(cfg: &GahConfig, current: &TerminalFailureState<'_>) -> bool {
    let Some(prior) =
        latest_unresolved_terminal_failure_record(cfg, current.profile, current.work_id)
    else {
        return true;
    };

    if prior.failure_class != current.failure_class {
        return true;
    }
    if prior.failure_stage.as_deref() != current.failure_stage {
        return true;
    }
    if prior.attempt_count != current.attempt_count {
        return true;
    }
    if prior.error_summary.as_deref() != current.error_summary {
        return true;
    }

    let now = match parse_terminal_failure_timestamp(current.timestamp) {
        Some(ts) => ts,
        None => return true,
    };
    let prior_ts = match parse_terminal_failure_timestamp(&prior.timestamp) {
        Some(ts) => ts,
        None => return true,
    };
    now - prior_ts > terminal_failure_dedupe_window()
}

fn record_terminal_failure_event(cfg: &GahConfig, failure: &TerminalFailureState<'_>) {
    let event_profile = failure.profile;
    let event_work_id = failure.work_id;
    let event_run_id = failure.run_id;
    let event_failure_class = failure.failure_class;
    let event_failure_stage = failure.failure_stage;
    let event_attempt_count = failure.attempt_count;
    let event_error_summary = failure.error_summary;
    let event_mr_url = failure.mr_url;
    let _ = events::record(
        cfg,
        events::EventType::TerminalFailure,
        Some(failure.profile),
        Some(failure.work_id),
        json!({
            "profile": event_profile,
            "work_id": event_work_id,
            "run_id": event_run_id,
            "failure_class": event_failure_class,
            "failure_stage": event_failure_stage,
            "attempt_count": event_attempt_count,
            "error_summary": event_error_summary,
            "mr_url": event_mr_url,
        })
        .to_string(),
    );
}

fn record_terminal_failure_resolved(
    cfg: &GahConfig,
    profile: &str,
    work_id: &str,
    resolved_run_id: &str,
    failure_class: &str,
    failure_stage: Option<&str>,
) {
    let _ = events::record(
        cfg,
        events::EventType::TerminalFailureResolved,
        Some(profile),
        Some(work_id),
        json!({
            "profile": profile,
            "work_id": work_id,
            "resolved_run_id": resolved_run_id,
            "failure_class": failure_class,
            "failure_stage": failure_stage,
        })
        .to_string(),
    );
}

fn event_name(event: &NotifyEvent<'_>) -> &'static str {
    match event {
        NotifyEvent::HumanRequired { .. } => "human_required",
        NotifyEvent::MrCreated { .. } => "mr_created",
        NotifyEvent::ReviewVerdict { .. } => "review_verdict",
        NotifyEvent::MrMerged { .. } => "mr_merged",
        NotifyEvent::DispatchFailed { .. } => "dispatch_failed",
        NotifyEvent::DispatchFailureResolved { .. } => "dispatch_failure_resolved",
        NotifyEvent::BackendStalled { .. } => "backend_stalled",
    }
}

fn event_work_id<'a>(event: &'a NotifyEvent<'a>) -> Option<&'a str> {
    match event {
        NotifyEvent::MrMerged { work_id, .. } => Some(work_id),
        NotifyEvent::DispatchFailed { work_id, .. } => Some(work_id),
        NotifyEvent::DispatchFailureResolved { work_id, .. } => Some(work_id),
        _ => None,
    }
}

fn event_run_id<'a>(event: &'a NotifyEvent<'a>) -> Option<&'a str> {
    match event {
        NotifyEvent::DispatchFailed { run_id, .. }
        | NotifyEvent::DispatchFailureResolved { run_id, .. } => Some(run_id),
        _ => None,
    }
}

fn event_profile<'a>(event: &'a NotifyEvent<'a>) -> Option<&'a str> {
    match event {
        NotifyEvent::DispatchFailed { profile, .. }
        | NotifyEvent::DispatchFailureResolved { profile, .. } => Some(profile),
        _ => None,
    }
}

pub(crate) fn notify_terminal_failure(
    cfg: &GahConfig,
    profile: &Profile,
    input: TerminalFailurePayload<'_>,
) {
    let timestamp = terminal_failure_timestamp_now();
    let safe_summary = summarize_error_summary(input.error_summary);
    let terminal_failure = TerminalFailureState {
        profile: input.profile,
        work_id: input.work_id,
        run_id: input.run_id,
        failure_class: input.failure_class,
        failure_stage: input.failure_stage,
        attempt_count: input.attempt_count,
        error_summary: safe_summary.as_deref(),
        timestamp: &timestamp,
        mr_url: input.mr_url,
    };
    if !should_emit_terminal_failure(cfg, &terminal_failure) {
        return;
    }

    record_terminal_failure_event(cfg, &terminal_failure);

    notify_event(
        cfg,
        profile,
        NotifyEvent::DispatchFailed {
            timestamp: &timestamp,
            profile: terminal_failure.profile,
            failure_class: terminal_failure.failure_class,
            failure_stage: terminal_failure.failure_stage,
            run_id: terminal_failure.run_id,
            work_id: terminal_failure.work_id,
            attempt_count: terminal_failure.attempt_count,
            error_summary: terminal_failure.error_summary,
            mr_url: terminal_failure.mr_url,
        },
    );
}

pub(crate) fn notify_terminal_failure_resolved(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    work_id: &str,
) {
    let Some(failure) = latest_unresolved_terminal_failure_record(cfg, profile_name, work_id)
    else {
        return;
    };
    let timestamp = terminal_failure_timestamp_now();

    let resolved_run_id = failure.run_id.as_str();
    record_terminal_failure_resolved(
        cfg,
        profile_name,
        work_id,
        resolved_run_id,
        &failure.failure_class,
        failure.failure_stage.as_deref(),
    );
    notify_event(
        cfg,
        profile,
        NotifyEvent::DispatchFailureResolved {
            timestamp: &timestamp,
            profile: profile_name,
            failure_class: failure.failure_class.as_str(),
            failure_stage: failure.failure_stage.as_deref(),
            work_id,
            run_id: resolved_run_id,
        },
    );
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
            let _ = events::record(
                cfg,
                events::EventType::NotificationDeliveryFailed,
                Some(event_profile(&event).unwrap_or(profile.display_name.as_str())),
                event_work_id(&event),
                json!({
                    "event_name": event_name(&event),
                    "profile": event_profile(&event).unwrap_or(profile.display_name.as_str()),
                    "work_id": event_work_id(&event),
                    "run_id": event_run_id(&event),
                    "error": format!("{err:#}"),
                })
                .to_string(),
            );
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
mod tests;
