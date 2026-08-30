use super::*;

#[test]
fn constructor_helper_commands_are_bounded_and_reap_descendants() {
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
        let session = CodexManagerSession::new_with_session_dir_and_timeout(
            f.bin_dir.join("codex"),
            f.record_dir.join("sessions"),
            Duration::from_millis(50),
        )
        .unwrap();
        outcomes.push((flag, survivor, started.elapsed(), f.record_dir.clone()));
        drop(session);
    }
    std::thread::sleep(Duration::from_millis(700));
    for (flag, survivor, elapsed, record_dir) in outcomes {
        assert!(elapsed < Duration::from_secs(1), "{flag} took {elapsed:?}");
        assert!(!record_dir.join(survivor).exists(), "{flag} leaked a child");
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
