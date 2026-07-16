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
fn invalid_review_output_is_reported_as_rerouting_not_dispatch_failure() {
    let msg = format_message(&NotifyEvent::ReviewOutputInvalid {
        mr_url: "https://example.com/mr/2",
        backend: "agy-second",
        model: "sonnet",
        reason: "actionable finding 1 was explicitly withdrawn",
    });
    assert_eq!(
        msg,
        "[gah] review output invalid on https://example.com/mr/2 route=agy-second/sonnet summary=actionable finding 1 was explicitly withdrawn; rerouting"
    );
}

#[test]
fn dispatch_failed_includes_failure_class_and_work_id() {
    let msg = format_message(&NotifyEvent::DispatchFailed {
        timestamp: "2026-07-01T00:00:00Z",
        profile: "p",
        failure_class: "validation_failure",
        failure_stage: Some("agent_run"),
        run_id: "run-1",
        work_id: "WORK-Y",
        attempt_count: Some(3),
        error_summary: None,
        mr_url: Some("https://example.com/mr/4"),
    });
    assert_eq!(
            msg,
            "[gah] dispatch terminal failure [ts=2026-07-01T00:00:00Z] [profile=p] [class=validation_failure] [stage=agent_run] [run_id=run-1] [attempts=3] work_id=WORK-Y ref=https://example.com/mr/4"
        );
}

#[test]
fn dispatch_failed_without_summary_renders_without_none() {
    let msg = format_message(&NotifyEvent::DispatchFailed {
        timestamp: "2026-07-01T00:00:00Z",
        profile: "p",
        failure_class: "unknown",
        failure_stage: None,
        run_id: "run-1",
        work_id: "WORK-Z",
        attempt_count: None,
        error_summary: None,
        mr_url: None,
    });
    assert_eq!(
            msg,
            "[gah] dispatch terminal failure [ts=2026-07-01T00:00:00Z] [profile=p] [class=unknown] [stage=unknown] [run_id=run-1] [attempts=unknown] work_id=WORK-Z"
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
        timestamp: "2026-07-01T00:00:00Z",
        profile: "p",
        failure_class: "validation_failure",
        failure_stage: Some("agent_run"),
        run_id: "run-1",
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

#[test]
fn dispatch_failed_marks_human_route_failure_as_paused_non_spending() {
    let msg = format_message(&NotifyEvent::DispatchFailed {
        timestamp: "2026-07-01T00:00:00Z",
        profile: "p",
        failure_class: "human_blocked",
        failure_stage: Some("route"),
        run_id: "run-1",
        work_id: "WORK-HR",
        attempt_count: Some(1),
        error_summary: None,
        mr_url: None,
    });
    assert!(msg.contains("[state=paused_non_spending]"));
}

#[test]
fn dispatch_failure_resolved_message_includes_profile_and_work_id() {
    let msg = format_message(&NotifyEvent::DispatchFailureResolved {
        timestamp: "2026-07-01T00:00:00Z",
        profile: "p",
        failure_class: "validation_failure",
        failure_stage: Some("agent_run"),
        work_id: "WORK-Z",
        run_id: "run-2",
    });
    assert_eq!(
            msg,
            "[gah] terminal failure resolved [ts=2026-07-01T00:00:00Z] [profile=p] [class=validation_failure] [stage=agent_run] [run_id=run-2] work_id=WORK-Z"
        );
}

#[test]
fn notify_terminal_failure_events_are_recorded_with_dedupe() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out.txt");
    let mut profile = crate::config::tests::test_profile_for_notifications();
    profile.notify_command = Some(format!("cat > {}", out.display()));
    let mut cfg = test_gah_config(None);
    cfg.defaults.artifact_root = tmp.path().to_string_lossy().to_string();
    clear_terminal_failure_cache_for_test(&cfg, "profile-a", "WORK-1");

    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-a",
            work_id: "WORK-1",
            run_id: "run-1",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("same summary"),
            mr_url: None,
        },
    );
    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-a",
            work_id: "WORK-1",
            run_id: "run-1",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("same summary"),
            mr_url: None,
        },
    );
    let events = crate::events::read_events(&cfg).unwrap();
    let terminal_count = events
        .iter()
        .filter(|event| event.event_type == crate::events::EventType::TerminalFailure.as_str())
        .count();
    assert_eq!(
        terminal_count, 1,
        "identical terminal failures should dedupe in window"
    );
    let got = std::fs::read_to_string(&out).unwrap_or_default();
    assert_eq!(got.lines().count(), 1);
}

#[test]
fn notify_terminal_failure_dedupe_falls_back_to_memory_when_event_log_is_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out.txt");
    let mut profile = crate::config::tests::test_profile_for_notifications();
    profile.notify_command = Some(format!("cat >> {}", out.display()));
    let mut cfg = test_gah_config(None);
    cfg.defaults.artifact_root = tmp.path().to_string_lossy().to_string();
    let events_path = cfg.defaults.events_path();
    clear_terminal_failure_cache_for_test(&cfg, "profile-cache", "WORK-CACHE");
    std::fs::create_dir_all(&events_path).unwrap();

    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-cache",
            work_id: "WORK-CACHE",
            run_id: "run-1",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("same summary"),
            mr_url: None,
        },
    );
    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-cache",
            work_id: "WORK-CACHE",
            run_id: "run-1",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("same summary"),
            mr_url: None,
        },
    );
    let got = std::fs::read_to_string(&out).unwrap_or_default();
    assert_eq!(
        got.lines().count(),
        1,
        "I/O-failed event log read should still dedupe via in-memory cache"
    );
}

#[test]
fn notify_terminal_failure_reemits_after_dedupe_window_has_elapsed() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out.txt");
    let mut profile = crate::config::tests::test_profile_for_notifications();
    profile.notify_command = Some(format!("cat >> {}", out.display()));
    let mut cfg = test_gah_config(None);
    cfg.defaults.artifact_root = tmp.path().to_string_lossy().to_string();
    clear_terminal_failure_cache_for_test(&cfg, "profile-window", "WORK-WINDOW");

    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-window",
            work_id: "WORK-WINDOW",
            run_id: "run-1",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("same summary"),
            mr_url: None,
        },
    );

    let mut events = crate::events::read_events(&cfg).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == crate::events::EventType::TerminalFailure.as_str())
            .count(),
        1
    );
    let stale_timestamp = (time::OffsetDateTime::now_utc() - time::Duration::seconds(901))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let mut rewrote = false;
    for event in &mut events {
        if event.event_type == crate::events::EventType::TerminalFailure.as_str() {
            event.timestamp = stale_timestamp.to_string();
            rewrote = true;
        }
    }
    assert!(
        rewrote,
        "test setup failed to capture emitted terminal failure event"
    );
    let serialized = events
        .into_iter()
        .map(|event| serde_json::to_string(&event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(cfg.defaults.events_path(), format!("{serialized}\n")).unwrap();

    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-window",
            work_id: "WORK-WINDOW",
            run_id: "run-1",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("same summary"),
            mr_url: None,
        },
    );

    let events = crate::events::read_events(&cfg).unwrap();
    let terminal_count = events
        .iter()
        .filter(|event| event.event_type == crate::events::EventType::TerminalFailure.as_str())
        .count();
    assert_eq!(
        terminal_count, 2,
        "stale terminal failure should emit again after window"
    );
    let got = std::fs::read_to_string(&out).unwrap_or_default();
    assert_eq!(got.lines().count(), 2);
}

#[test]
fn notify_terminal_failure_distinct_class_generates_additional_notification() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out.txt");
    let mut profile = crate::config::tests::test_profile_for_notifications();
    profile.notify_command = Some(format!("cat >> {}", out.display()));
    let mut cfg = test_gah_config(None);
    cfg.defaults.artifact_root = tmp.path().to_string_lossy().to_string();
    clear_terminal_failure_cache_for_test(&cfg, "profile-b", "WORK-2");

    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-b",
            work_id: "WORK-2",
            run_id: "run-2",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("summary"),
            mr_url: None,
        },
    );
    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-b",
            work_id: "WORK-2",
            run_id: "run-2",
            failure_class: "human_blocked",
            failure_stage: Some("route"),
            attempt_count: Some(1),
            error_summary: Some("summary"),
            mr_url: None,
        },
    );
    let events = crate::events::read_events(&cfg).unwrap();
    let terminal_count = events
        .iter()
        .filter(|event| event.event_type == crate::events::EventType::TerminalFailure.as_str())
        .count();
    assert_eq!(
        terminal_count, 2,
        "distinct terminal failures should not dedupe"
    );
    let got = std::fs::read_to_string(&out).unwrap_or_default();
    assert_eq!(got.lines().count(), 2);
}

#[test]
fn notify_terminal_failure_resolved_emits_once() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out.txt");
    let mut profile = crate::config::tests::test_profile_for_notifications();
    profile.notify_command = Some(format!("cat >> {}", out.display()));
    let mut cfg = test_gah_config(None);
    cfg.defaults.artifact_root = tmp.path().to_string_lossy().to_string();
    clear_terminal_failure_cache_for_test(&cfg, "profile-c", "WORK-3");

    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-c",
            work_id: "WORK-3",
            run_id: "run-3",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("summary"),
            mr_url: None,
        },
    );
    notify_terminal_failure_resolved(&cfg, &profile, "profile-c", "WORK-3");
    notify_terminal_failure_resolved(&cfg, &profile, "profile-c", "WORK-3");
    let events = crate::events::read_events(&cfg).unwrap();
    let terminal_count = events
        .iter()
        .filter(|event| event.event_type == crate::events::EventType::TerminalFailure.as_str())
        .count();
    let resolved_count = events
        .iter()
        .filter(|event| {
            event.event_type == crate::events::EventType::TerminalFailureResolved.as_str()
        })
        .count();
    assert_eq!(terminal_count, 1);
    assert_eq!(resolved_count, 1);
    assert_eq!(
        std::fs::read_to_string(&out)
            .unwrap_or_default()
            .lines()
            .count(),
        2,
        "failure and resolved messages should be visible"
    );
}

#[test]
fn notify_command_failure_is_recorded_as_observable_event() {
    let tmp = tempfile::tempdir().unwrap();
    let mut profile = crate::config::tests::test_profile_for_notifications();
    profile.notify_command = Some("does-not-exist-123".to_string());
    let mut cfg = test_gah_config(None);
    cfg.defaults.artifact_root = tmp.path().to_string_lossy().to_string();
    clear_terminal_failure_cache_for_test(&cfg, "profile-d", "WORK-4");

    notify_terminal_failure(
        &cfg,
        &profile,
        TerminalFailurePayload {
            profile: "profile-d",
            work_id: "WORK-4",
            run_id: "run-4",
            failure_class: "validation_failure",
            failure_stage: Some("agent_run"),
            attempt_count: Some(1),
            error_summary: Some("summary"),
            mr_url: None,
        },
    );
    let events = crate::events::read_events(&cfg).unwrap();
    let failure = events.iter().any(|event| {
        event.event_type == crate::events::EventType::NotificationDeliveryFailed.as_str()
    });
    assert!(
        failure,
        "notify command failures should be observable in events"
    );
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
    assert!(got.contains("[gah] MR created https://example.com/mr/1 (work_id=WORK-X, agy/opus)"));
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
            timestamp: "2026-07-01T00:00:00Z",
            profile: "p",
            failure_class: "c",
            failure_stage: Some("agent_run"),
            run_id: "run-1",
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
fn wake_instruction_includes_paused_non_spending_for_human_route_failures() {
    let event = NotifyEvent::DispatchFailed {
        timestamp: "2026-07-01T00:00:00Z",
        profile: "p",
        failure_class: "human_blocked",
        failure_stage: Some("route"),
        run_id: "run-1",
        work_id: "WORK-HR",
        attempt_count: Some(1),
        error_summary: None,
        mr_url: None,
    };
    let instruction = format_wake_instruction(&event, WakeAutonomy::ReviewOnly).unwrap();
    assert!(instruction.contains("[state=paused_non_spending]"));
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
