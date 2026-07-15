use super::*;
use crate::dispatch::test_util::profile;

#[test]
fn authoritative_ticket_metadata_populates_ledger_work_identity() {
    let ticket = TicketMetadata {
        ticket_id: Some("TICKET-095".into()),
        work_id: Some("TICKET-095".into()),
        title: Some("Ledger work identity propagation".into()),
        is_authoritative: true,
        ..TicketMetadata::default()
    };
    let tmp = tempfile::tempdir().unwrap();
    let mut ledger = LedgerEntry::new(
        "real",
        &profile(tmp.path()),
        "codex",
        "fix",
        "target",
        Some("session-1".into()),
        None,
    );

    apply_authoritative_work_identity(&mut ledger, Some(&ticket), "gah/real-123");

    assert_eq!(ledger.work_id.as_deref(), Some("TICKET-095"));
    assert_eq!(
        ledger.work_title.as_deref(),
        Some("Ledger work identity propagation")
    );
}

#[test]
fn non_authoritative_ticket_metadata_falls_back_to_synthetic_work_id() {
    // TICKET-091 AC4: no authoritative external ticket -> generate an
    // internal ID (the branch name) rather than leaving work_id unset.
    let ticket = TicketMetadata {
        ticket_id: Some("TICKET-095".into()),
        work_id: Some("TICKET-095".into()),
        title: Some("Ledger work identity propagation".into()),
        is_authoritative: false,
        ..TicketMetadata::default()
    };
    let tmp = tempfile::tempdir().unwrap();
    let mut ledger = LedgerEntry::new(
        "real",
        &profile(tmp.path()),
        "codex",
        "fix",
        "target",
        Some("session-1".into()),
        None,
    );

    apply_authoritative_work_identity(&mut ledger, Some(&ticket), "gah/real-123");

    assert_eq!(ledger.work_id.as_deref(), Some("gah/real-123"));
    assert_eq!(ledger.work_title, None);
}

#[test]
fn no_ticket_falls_back_to_synthetic_work_id() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ledger = LedgerEntry::new(
        "real",
        &profile(tmp.path()),
        "codex",
        "fix",
        "target",
        Some("session-1".into()),
        None,
    );

    apply_authoritative_work_identity(&mut ledger, None, "gah/real-456");

    assert_eq!(ledger.work_id.as_deref(), Some("gah/real-456"));
}

#[test]
fn controller_work_id_survives_existing_branch_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ledger = LedgerEntry::new(
        "real",
        &profile(tmp.path()),
        "codex",
        "fix",
        "gah/existing-branch",
        Some("session-1".into()),
        None,
    );
    ledger.work_id = Some("#437".into());

    apply_authoritative_work_identity(&mut ledger, None, "gah/existing-branch");

    assert_eq!(ledger.work_id.as_deref(), Some("#437"));
}

#[test]
fn bounded_validation_failure_preserves_command_and_failure_tail() {
    let text = format!(
        "$ cargo test\n{}\nfailures:\n    sync_provider_fails_then_recovers\ntest result: FAILED",
        "routine passing output\n".repeat(500)
    );

    let bounded = bounded_validation_failure(&text, 500);

    assert!(bounded.len() <= 500);
    assert!(bounded.starts_with("$ cargo test"));
    assert!(bounded.contains("failure evidence"));
    assert!(bounded.contains("sync_provider_fails_then_recovers"));
    assert!(bounded.ends_with("test result: FAILED"));
}

#[test]
fn bounded_validation_failure_handles_multibyte_boundaries() {
    let text = format!("start {} end failure", "🚀".repeat(300));
    let bounded = bounded_validation_failure(&text, 101);

    assert!(bounded.len() <= 101);
    assert!(bounded.starts_with("start"));
    assert!(bounded.ends_with("end failure"));
}
