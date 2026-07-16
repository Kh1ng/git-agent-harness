use crate::dispatch::issues::TicketMetadata;
use crate::ledger::LedgerEntry;

/// Apply an authoritative external identity, retain a controller-supplied
/// FixMr identity, or fall back to the unique dispatch branch.
pub(super) fn apply_authoritative_work_identity(
    ledger: &mut LedgerEntry,
    ticket: Option<&TicketMetadata>,
    fallback_work_id: &str,
) {
    if let Some(ticket) = ticket {
        ledger.task_class = ticket.task_class.clone();
        ledger.difficulty = ticket.difficulty.clone();
    }
    match ticket {
        Some(ticket) if ticket.is_authoritative => {
            ledger.work_id = ticket.work_id.clone().or_else(|| ticket.ticket_id.clone());
            ledger.source_issue_number = ticket.issue_number.clone();
            ledger.work_title = ticket.title.clone();
        }
        _ if ledger
            .work_id
            .as_deref()
            .is_some_and(|work_id| !work_id.trim().is_empty()) => {}
        _ => ledger.work_id = Some(fallback_work_id.to_string()),
    }
}
