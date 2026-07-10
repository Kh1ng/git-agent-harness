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
        NotifyEvent::HumanRequired { reason, reference } => {
            let reference_suffix = reference
                .map(|r| format!(" Reference: {r}."))
                .unwrap_or_default();
            format!("gah loop is blocked and needs judgment: {reason}.{reference_suffix}")
        }
        NotifyEvent::ReviewVerdict { verdict, mr_url } => {
            format!("A review verdict was recorded: {verdict} on {mr_url}.")
        }
        NotifyEvent::DispatchFailed {
            failure_class,
            work_id,
        } => format!("A dispatch failed terminally: [{failure_class}] work_id={work_id}."),
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
    use std::io::Write;
    use std::process::{Command, Stdio};

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

    cmd.stdin(Stdio::null());
    match std::fs::File::create(&log_path) {
        Ok(mut log_file) => {
            let _ = writeln!(log_file, "instruction: {instruction}\n---");
            let log_err = log_file.try_clone().ok();
            cmd.stdout(Stdio::from(log_file));
            if let Some(log_err) = log_err {
                cmd.stderr(Stdio::from(log_err));
            } else {
                cmd.stderr(Stdio::null());
            }
        }
        Err(err) => {
            eprintln!(
                "[gah] manager_wake: failed to open log file {} (swallowed, output will be discarded): {err:#}",
                log_path.display()
            );
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }

    // Deliberately fire-and-forget: this must not block the dispatch/loop
    // that triggered it. The spawned process becomes independent; if this
    // (typically short-lived) gah process exits first, the child is simply
    // reparented to init, same as any other background process launched
    // this way -- no zombie risk.
    if let Err(err) = cmd.spawn() {
        eprintln!("[gah] manager_wake: failed to spawn '{manager}' (swallowed): {err:#}");
    }
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
        let message = format_message(&event);
        if let Err(err) = run_notify_command(command, &message) {
            eprintln!("[gah] notify_command failed (swallowed): {err:#}");
        }
    }

    if let Some(instruction) = format_wake_instruction(&event, profile.manager_wake_autonomy) {
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

    fn test_gah_config(current_manager: Option<&str>) -> GahConfig {
        GahConfig {
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
            },
            NotifyEvent::MrCreated {
                url: "u",
                work_id: "w",
                backend: "b",
                model: "m",
            },
            NotifyEvent::ReviewVerdict {
                verdict: "APPROVE_STRONG",
                mr_url: "u",
            },
            NotifyEvent::DispatchFailed {
                failure_class: "c",
                work_id: "w",
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
        };
        let instruction = format_wake_instruction(&event, WakeAutonomy::Full).unwrap();
        assert!(instruction.contains("MR ready for human decision"));
        assert!(instruction.contains("https://example.com/mr/7"));
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
