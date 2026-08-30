use super::*;

fn named_helper_captures() -> Vec<std::ffi::OsString> {
    let prefix = format!("gah-codex-helper-{}-", std::process::id());
    fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
        .filter(|name| name.to_string_lossy().starts_with(&prefix))
        .collect()
}

#[test]
fn helper_output_has_no_named_capture_while_the_command_runs() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    fs::write(f.record_dir.join("hang-version"), "").unwrap();
    let before = named_helper_captures();
    let executable = f.bin_dir.join("codex");
    let session_dir = f.record_dir.join("sessions");
    let constructor = std::thread::spawn(move || {
        CodexManagerSession::new_with_session_dir_and_timeout(
            executable,
            session_dir,
            Duration::from_millis(500),
        )
    });
    std::thread::sleep(Duration::from_millis(100));

    assert_eq!(named_helper_captures(), before);
    drop(constructor.join().unwrap().unwrap());
    assert_eq!(named_helper_captures(), before);
}

#[test]
fn successful_helper_exit_reaps_its_background_descendants() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    fs::write(f.record_dir.join("exit-version-with-child"), "").unwrap();

    let started = Instant::now();
    let discovery =
        discover_with_timeout(f.bin_dir.join("codex"), Duration::from_millis(500)).unwrap();
    assert_eq!(discovery.version.as_deref(), Some("codex-cli 1.2.3"));
    assert!(started.elapsed() < Duration::from_secs(1));
    std::thread::sleep(Duration::from_millis(700));
    assert!(!f.record_dir.join("version-helper-survived.marker").exists());
}

#[cfg(unix)]
#[test]
fn detached_capture_holder_is_terminated_by_production_cleanup() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    fs::write(f.record_dir.join("exit-version-with-detached-child"), "").unwrap();

    let discovery = discover_with_timeout(f.bin_dir.join("codex"), Duration::from_millis(500))
        .expect("escaped capture holder should be cleaned up");
    let pid: libc::pid_t = fs::read_to_string(f.record_dir.join("detached-helper.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(discovery.version.as_deref(), Some("codex-cli 1.2.3"));
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert!(!f.record_dir.join("version-helper-survived.marker").exists());
}

#[test]
fn helper_capture_overflow_fails_closed_before_app_server_start() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    fs::write(f.record_dir.join("overflow-version"), "").unwrap();

    let error = CodexManagerSession::new_with_session_dir_and_timeout(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
        Duration::from_millis(500),
    )
    .err()
    .expect("capture overflow must fail construction");

    assert!(format!("{error:#}").contains("exceeded 65536 bytes"));
    assert!(!f.record_dir.join("requests.jsonl").exists());
}

#[test]
fn nonblocking_setup_failure_closes_capture_and_starts_no_app_server() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    FAIL_HELPER_NONBLOCKING.store(true, Ordering::SeqCst);
    let started = Instant::now();

    let error = CodexManagerSession::new_with_session_dir_and_timeout(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
        Duration::from_millis(500),
    )
    .err()
    .expect("nonblocking setup failure must fail construction");

    assert!(format!("{error:#}").contains("injected helper nonblocking failure"));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!f.record_dir.join("requests.jsonl").exists());
}

#[test]
fn helper_commands_are_bounded_and_reap_descendants() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let mut outcomes = Vec::new();
    for (flag, survivor) in [
        ("hang-version", "version-helper-survived.marker"),
        ("hang-login", "login-helper-survived.marker"),
        ("hang-schema", "schema-helper-survived.marker"),
    ] {
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        fs::write(f.record_dir.join(flag), "").unwrap();
        let started = Instant::now();
        match flag {
            "hang-schema" => {
                detect_stable_methods(&f.bin_dir.join("codex"), Duration::from_millis(50)).unwrap();
            }
            _ => {
                discover_with_timeout(f.bin_dir.join("codex"), Duration::from_millis(50)).unwrap();
            }
        }
        outcomes.push((flag, survivor, started.elapsed(), f.record_dir.clone()));
    }
    std::thread::sleep(Duration::from_millis(700));
    for (flag, survivor, elapsed, record_dir) in outcomes {
        assert!(elapsed < Duration::from_secs(1), "{flag} took {elapsed:?}");
        assert!(!record_dir.join(survivor).exists(), "{flag} leaked a child");
    }
}

#[test]
fn unconfirmed_helper_cleanup_prevents_app_server_start() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let mut outcomes = Vec::new();
    for flag in ["hang-version", "hang-login", "hang-schema"] {
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        fs::write(f.record_dir.join(flag), "").unwrap();
        FAIL_HELPER_CLEANUP_AFTER_REAP.store(true, Ordering::SeqCst);
        let result = CodexManagerSession::new_with_session_dir_and_timeout(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
            Duration::from_millis(50),
        );
        let failure = match result {
            Ok(session) => {
                drop(session);
                "constructor started the app-server".to_string()
            }
            Err(error) => format!("{error:#}"),
        };
        outcomes.push((flag, failure, f.record_dir.join("requests.jsonl")));
    }

    for (flag, failure, requests) in outcomes {
        assert!(
            failure.contains("injected unconfirmed helper cleanup"),
            "{flag}: {failure}"
        );
        assert!(!requests.exists(), "{flag} started the app-server");
    }
}

#[test]
fn silent_initialize_times_out_and_terminates_its_transport() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    fs::write(f.record_dir.join("silent-initialize"), "").unwrap();

    let started = Instant::now();
    let error = CodexManagerSession::new_with_session_dir_and_timeout(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
        Duration::from_millis(50),
    )
    .err()
    .expect("silent initialize must time out");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(format!("{error:#}").contains("timed out waiting for Codex app-server response"));
    std::thread::sleep(Duration::from_millis(700));
    assert!(!f.record_dir.join("app-server-survived.marker").exists());
}

#[test]
fn silent_turn_response_times_out_and_stops_remote_work() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("sessions");
    let mut session =
        CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir).unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "hello".into(),
        })
        .unwrap();
    assert_eq!(
        wait_for_terminal(&mut session, &id),
        TerminalStatus::Completed
    );
    session.transport.response_timeout = Duration::from_millis(50);

    let started = Instant::now();
    let error = session.send(&id, "silent-response").unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(error
        .to_string()
        .contains("timed out waiting for Codex app-server response"));
    assert_failed_session_is_observable(&mut session, &id);
    assert!(mapping_path(&session_dir, &id).exists());
}

#[test]
fn unrelated_messages_cannot_extend_a_request_deadline() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("sessions");
    let mut session =
        CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir).unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "hello".into(),
        })
        .unwrap();
    assert_eq!(
        wait_for_terminal(&mut session, &id),
        TerminalStatus::Completed
    );
    session.transport.response_timeout = Duration::from_millis(50);

    let started = Instant::now();
    let error = session.send(&id, "chatty-no-response").unwrap_err();
    assert!(started.elapsed() < Duration::from_millis(250));
    assert!(format!("{error:#}").contains("timed out waiting for Codex app-server response"));
    assert_failed_session_is_observable(&mut session, &id);
    assert!(mapping_path(&session_dir, &id).exists());
}

#[test]
fn request_deadline_includes_delivery_to_a_non_reading_server() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    fs::write(f.record_dir.join("stop-reading-after-thread-start"), "").unwrap();
    let session_dir = f.record_dir.join("sessions");
    let mut session =
        CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir).unwrap();
    session.transport.response_timeout = Duration::from_millis(50);

    let started = Instant::now();
    let error = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "x".repeat(2 * 1024 * 1024),
        })
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(format!("{error:#}").contains("timed out writing Codex app-server request"));
    assert!(session.sessions.is_empty());
    assert!(fs::read_dir(&session_dir).unwrap().next().is_none());
    std::thread::sleep(Duration::from_millis(700));
    assert!(!f.record_dir.join("app-server-survived.marker").exists());
}

#[test]
fn silent_resume_times_out_and_terminates_shared_transport() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let session_dir = f.record_dir.join("sessions");
    let mut session =
        CodexManagerSession::new_with_session_dir(f.bin_dir.join("codex"), &session_dir).unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "slow".into(),
        })
        .unwrap();
    drain_all_pending(&mut session, &id);
    fs::write(f.record_dir.join("silent-resume"), "").unwrap();
    session.transport.response_timeout = Duration::from_millis(50);

    let error = session.resume(&id).unwrap_err();
    assert!(format!("{error:#}").contains("timed out waiting for Codex app-server response"));
    assert!(session.transport.terminated);
    assert_failed_session_is_observable(&mut session, &id);
    assert!(mapping_path(&session_dir, &id).exists());
}

#[test]
fn malformed_terminal_notifications_are_sticky_transport_failures() {
    for instruction in [
        "started-missing-thread-id",
        "started-nonstring-thread-id",
        "started-missing-turn-id",
        "started-nonstring-turn-id",
        "retryable-missing-thread-id",
        "retryable-nonstring-thread-id",
        "retryable-missing-turn-id",
        "retryable-nonstring-turn-id",
        "retryable-missing-message",
        "retryable-nonstring-message",
        "completed-missing-id",
        "completed-missing-status",
        "completed-unknown-status",
        "incomplete-error",
    ] {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_json_rpc_codex(&f.bin_dir, &f.record_dir);
        let mut session = CodexManagerSession::new_with_session_dir(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
        )
        .unwrap();
        let id = session
            .start(StartRequest {
                profile: "profile-a".into(),
                instruction: instruction.into(),
            })
            .unwrap();

        assert!(matches!(
            wait_for_terminal(&mut session, &id),
            TerminalStatus::Failed(_)
        ));
        assert!(session.transport.terminated, "case {instruction}");
        std::thread::sleep(Duration::from_millis(100));
        assert!(matches!(
            session.terminal_status(&id).unwrap(),
            Some(TerminalStatus::Failed(_))
        ));
    }
}

#[test]
fn malformed_transport_output_is_a_sticky_terminal_failure() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let f = fixture();
    make_json_rpc_codex(&f.bin_dir, &f.record_dir);
    let mut session = CodexManagerSession::new_with_session_dir(
        f.bin_dir.join("codex"),
        f.record_dir.join("sessions"),
    )
    .unwrap();
    let id = session
        .start(StartRequest {
            profile: "profile-a".into(),
            instruction: "malformed-output".into(),
        })
        .unwrap();

    let TerminalStatus::Failed(message) = wait_for_terminal(&mut session, &id) else {
        panic!("malformed provider output must terminate the session as failed");
    };
    assert!(message.contains("parsing Codex app-server line"));
    std::thread::sleep(Duration::from_millis(100));
    assert!(matches!(
        session.terminal_status(&id).unwrap(),
        Some(TerminalStatus::Failed(_))
    ));
}
