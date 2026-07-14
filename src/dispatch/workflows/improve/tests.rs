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
