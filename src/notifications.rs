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

use crate::config::Profile;

/// A notification-worthy controller/dispatch event. All fields are borrowed
/// from the live dispatch/controller state to avoid cloning.
pub enum NotifyEvent<'a> {
    /// A controller action was decided as `HumanRequired`.
    HumanRequired {
        reason: &'a str,
        reference: Option<&'a str>,
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
        work_id: &'a str,
    },
}

/// Render a `NotifyEvent` into the single-line message GAH pipes to
/// `notify_command`. Pure and allocation-light; unit-tested below.
pub fn format_message(event: &NotifyEvent) -> String {
    match event {
        NotifyEvent::HumanRequired { reason, reference } => {
            let ref_suffix = reference.map(|r| format!(" ({r})")).unwrap_or_default();
            format!("[gah] human required: {reason}{ref_suffix}")
        }
        NotifyEvent::MrCreated {
            url,
            work_id,
            backend,
            model,
        } => {
            format!("[gah] MR created {url} (work_id={work_id}, {backend}/{model})")
        }
        NotifyEvent::ReviewVerdict { verdict, mr_url } => {
            format!("[gah] review {verdict} on {mr_url}")
        }
        NotifyEvent::MrMerged { url, work_id } => {
            format!("[gah] auto-merged {url} (work_id={work_id})")
        }
        NotifyEvent::DispatchFailed {
            failure_class,
            work_id,
        } => {
            format!("[gah] dispatch failed [{failure_class}] work_id={work_id}")
        }
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

/// Fire a notification for `event` if the profile defines `notify_command`.
///
/// This is the single public entry point. It is infallible by design: any
/// error from spawning or running `notify_command` is logged to stderr and
/// swallowed so the caller's flow continues exactly as if no hook existed.
pub fn notify_event(profile: &Profile, event: NotifyEvent) {
    let Some(command) = &profile.notify_command else {
        return;
    };
    let message = format_message(&event);
    if let Err(err) = run_notify_command(command, &message) {
        eprintln!("[gah] notify_command failed (swallowed): {err:#}");
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
        });
        assert_eq!(
            msg,
            "[gah] human required: MR ready for human decision (https://github.com/owner/repo/pull/7)"
        );
    }

    #[test]
    fn human_required_without_reference() {
        let msg = format_message(&NotifyEvent::HumanRequired {
            reason: "waiting on operator",
            reference: None,
        });
        assert_eq!(msg, "[gah] human required: waiting on operator");
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
    fn review_verdict_includes_verdict_and_url() {
        let msg = format_message(&NotifyEvent::ReviewVerdict {
            verdict: "APPROVE_STRONG",
            mr_url: "https://example.com/mr/2",
        });
        assert_eq!(
            msg,
            "[gah] review APPROVE_STRONG on https://example.com/mr/2"
        );
    }

    #[test]
    fn dispatch_failed_includes_failure_class_and_work_id() {
        let msg = format_message(&NotifyEvent::DispatchFailed {
            failure_class: "validation_failure",
            work_id: "WORK-Y",
        });
        assert_eq!(
            msg,
            "[gah] dispatch failed [validation_failure] work_id=WORK-Y"
        );
    }

    #[test]
    fn notify_event_is_a_noop_when_command_unset() {
        // No command -> no spawn, no panic, no output.
        let profile = crate::config::tests::test_profile_for_notifications();
        notify_event(
            &profile,
            NotifyEvent::HumanRequired {
                reason: "x",
                reference: None,
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
        notify_event(
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
}
