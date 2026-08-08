use super::codex::CodexManagerSession;
use super::contract::run_contract_suite;
use super::fake::FakeManagerSession;
use super::*;

#[test]
fn fake_adapter_satisfies_the_contract_suite_with_every_optional_capability_on() {
    let mut session = FakeManagerSession::new(SessionCapabilities {
        resume: true,
        interrupt: true,
        inspect: true,
    });
    run_contract_suite(&mut session);
}

#[test]
fn fake_adapter_satisfies_the_contract_suite_with_every_optional_capability_off() {
    // Exercises the contract suite's UnsupportedCapability assertions --
    // without this, that branch of contract.rs would never actually run.
    let mut session = FakeManagerSession::new(SessionCapabilities::default());
    run_contract_suite(&mut session);
}

// Run the contract suite against the real CodexManagerSession adapter code
// (not FakeManagerSession), but backed by a fake `codex` executable rather
// than a real install -- see codex.rs's write_fake_codex doc comment for
// why that's still a real exercise of the adapter (discovery is the only
// part that touches a process at all).

#[test]
fn codex_adapter_satisfies_the_contract_suite_with_every_optional_capability_on() {
    let dir = tempfile::tempdir().unwrap();
    let codex_path = super::codex::write_fake_codex(dir.path());
    let mut session = CodexManagerSession::new(
        Some(&codex_path),
        Some(SessionCapabilities {
            resume: true,
            interrupt: true,
            inspect: true,
        }),
    )
    .unwrap();
    run_contract_suite(&mut session);
}

#[test]
fn codex_adapter_satisfies_the_contract_suite_with_every_optional_capability_off() {
    let dir = tempfile::tempdir().unwrap();
    let codex_path = super::codex::write_fake_codex(dir.path());
    let mut session =
        CodexManagerSession::new(Some(&codex_path), Some(SessionCapabilities::default())).unwrap();
    run_contract_suite(&mut session);
}

#[test]
fn stream_drains_pending_updates_and_does_not_repeat_them() {
    let mut session = FakeManagerSession::new(SessionCapabilities::default());
    let id = session
        .start(StartRequest {
            profile: "p".into(),
            instruction: "hi".into(),
        })
        .unwrap();

    let first = session.stream(&id).unwrap();
    assert!(!first.is_empty());
    let second = session.stream(&id).unwrap();
    assert!(
        second.is_empty(),
        "a second stream call with no new activity must return nothing"
    );
}

#[test]
fn operations_against_an_unknown_session_id_fail_closed() {
    let mut session = FakeManagerSession::new(SessionCapabilities::default());
    let unknown = GahSessionId::new("nonexistent");

    assert!(session.send(&unknown, "hi").is_err());
    assert!(session.stream(&unknown).is_err());
    assert!(session.terminal_status(&unknown).is_err());
}

#[test]
fn session_ids_are_namespaced_and_unique_per_start() {
    let mut session = FakeManagerSession::new(SessionCapabilities::default());
    let a = session
        .start(StartRequest {
            profile: "p".into(),
            instruction: "hi".into(),
        })
        .unwrap();
    let b = session
        .start(StartRequest {
            profile: "p".into(),
            instruction: "hi".into(),
        })
        .unwrap();

    assert_ne!(a, b);
    assert!(a.as_str().starts_with("gah:manager:p:"));
}
