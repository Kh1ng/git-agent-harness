//! The contract every `ManagerSession` adapter must satisfy, exercised here
//! against `fake::FakeManagerSession` and intended to run unchanged against
//! a real adapter once #816/#817/#818 land -- that's the whole point of
//! keeping this behind `&mut dyn ManagerSession` rather than anything
//! fake-specific.

use super::{unsupported_capability, ManagerSession, StartRequest};
use std::time::Duration;

fn wait_for_update(
    session: &mut dyn ManagerSession,
    id: &super::GahSessionId,
) -> Vec<super::SessionUpdate> {
    for _ in 0..100 {
        let updates = session.stream(id).expect("stream must succeed");
        if !updates.is_empty() {
            return updates;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    vec![]
}

pub(crate) fn run_contract_suite(session: &mut dyn ManagerSession) {
    assert_start_produces_a_session_and_an_update(session);
    assert_send_produces_a_further_update(session);
    assert_fresh_session_has_no_failure(session);
    assert_interrupt_terminates_when_supported(session);
    assert_unsupported_capabilities_return_a_typed_error(session);
}

fn start(session: &mut dyn ManagerSession, instruction: &str) -> super::GahSessionId {
    session
        .start(StartRequest {
            profile: "contract-suite".to_string(),
            instruction: instruction.to_string(),
        })
        .expect("start must succeed for a fresh session")
}

fn assert_start_produces_a_session_and_an_update(session: &mut dyn ManagerSession) {
    let id = start(session, "hello");
    let updates = wait_for_update(session, &id);
    assert!(
        !updates.is_empty(),
        "start must produce at least one observable update, or a caller has no way to \
         know the session is alive"
    );
}

fn assert_send_produces_a_further_update(session: &mut dyn ManagerSession) {
    let id = start(session, "hello");
    session
        .stream(&id)
        .expect("draining the start update must succeed");
    session
        .send(&id, "follow up")
        .expect("send must succeed against an active session");
    let updates = wait_for_update(session, &id);
    assert!(
        !updates.is_empty(),
        "send must produce at least one further observable update"
    );
}

fn assert_fresh_session_has_no_failure(session: &mut dyn ManagerSession) {
    let id = start(session, "hello");
    let status = session.terminal_status(&id).unwrap();
    assert!(
        matches!(status, None | Some(super::TerminalStatus::Completed)),
        "a fresh session may finish immediately, but must not fail or report interruption"
    );
}

fn assert_interrupt_terminates_when_supported(session: &mut dyn ManagerSession) {
    if !session.capabilities().interrupt {
        return;
    }
    let id = start(session, "hello");
    session
        .interrupt(&id)
        .expect("interrupt must succeed when the adapter declares support for it");
    assert!(
        session.terminal_status(&id).unwrap().is_some(),
        "an interrupted session must report a terminal status"
    );
}

/// Issue #815 AC: an adapter that declares a capability unsupported must
/// fail closed with the typed error, not silently no-op or panic.
fn assert_unsupported_capabilities_return_a_typed_error(session: &mut dyn ManagerSession) {
    let capabilities = session.capabilities();
    let id = start(session, "hello");

    if !capabilities.resume {
        let err = session
            .resume(&id)
            .expect_err("resume must fail when unsupported, not silently no-op");
        assert!(
            unsupported_capability(&err).is_some(),
            "an unsupported-resume failure must be the typed UnsupportedCapability error"
        );
    }
    if !capabilities.interrupt {
        let err = session
            .interrupt(&id)
            .expect_err("interrupt must fail when unsupported, not silently no-op");
        assert!(unsupported_capability(&err).is_some());
    }
    if !capabilities.inspect {
        let err = session
            .inspect(&id)
            .expect_err("inspect must fail when unsupported, not silently no-op");
        assert!(unsupported_capability(&err).is_some());
    }
}
