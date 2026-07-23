use crate::models::TicketPriority;
use crate::status::ScopeStatusJson;

use super::support::empty_snapshot;
use super::{decide_next_action, NextAction};
use crate::models::AvailableTicket;

fn ticket_with_priority(
    path: &str,
    work_id: Option<&str>,
    prior_attempt_count: usize,
    last_failure_class: Option<&str>,
    has_active_mr: bool,
    human_required: bool,
    priority: TicketPriority,
) -> AvailableTicket {
    let mut ticket = super::ticket(
        path,
        work_id,
        prior_attempt_count,
        last_failure_class,
        has_active_mr,
        human_required,
    );
    ticket.priority = priority;
    ticket
}

#[test]
fn undispatched_tickets_are_ordered_by_priority_then_numeric_issue_identity() {
    let mut snapshot = empty_snapshot();
    snapshot.available_tickets.push(ticket_with_priority(
        "1",
        Some("#1"),
        0,
        None,
        false,
        false,
        TicketPriority::P2,
    ));
    snapshot.available_tickets.push(ticket_with_priority(
        "10",
        Some("#10"),
        0,
        None,
        false,
        false,
        TicketPriority::P1,
    ));
    snapshot.available_tickets.push(ticket_with_priority(
        "9",
        Some("#9"),
        0,
        None,
        false,
        false,
        TicketPriority::P1,
    ));

    let action = decide_next_action(&snapshot);
    match action {
        NextAction::DispatchTicket { work_id, .. } => assert_eq!(work_id.as_deref(), Some("#9")),
        other => panic!("expected DispatchTicket for #9, got {other:?}"),
    }
}

#[test]
fn undispatched_non_numeric_ticket_paths_fall_back_to_lexical_ordering() {
    let mut snapshot = empty_snapshot();
    snapshot.available_tickets.push(ticket_with_priority(
        "docs/tickets/TICKET-beta.md",
        Some("TICKET-beta"),
        0,
        None,
        false,
        false,
        TicketPriority::P1,
    ));
    snapshot.available_tickets.push(ticket_with_priority(
        "docs/tickets/TICKET-alpha.md",
        Some("TICKET-alpha"),
        0,
        None,
        false,
        false,
        TicketPriority::P1,
    ));

    let action = decide_next_action(&snapshot);
    match action {
        NextAction::DispatchTicket {
            work_id,
            ticket_path,
            ..
        } => {
            assert_eq!(work_id.as_deref(), Some("TICKET-alpha"));
            assert_eq!(ticket_path, "docs/tickets/TICKET-alpha.md");
        }
        other => panic!("expected DispatchTicket for lexical fallback, got {other:?}"),
    }
}

#[test]
fn missing_priority_does_not_preempt_explicit_p0() {
    let mut snapshot = empty_snapshot();
    snapshot.available_tickets.push(ticket_with_priority(
        "7",
        Some("#7"),
        0,
        None,
        false,
        false,
        TicketPriority::P0,
    ));
    snapshot.available_tickets.push(ticket_with_priority(
        "8",
        Some("#8"),
        0,
        None,
        false,
        false,
        TicketPriority::Unspecified,
    ));

    let action = decide_next_action(&snapshot);
    match action {
        NextAction::DispatchTicket { work_id, .. } => assert_eq!(work_id.as_deref(), Some("#7")),
        other => panic!("expected DispatchTicket for #7, got {other:?}"),
    }
}

#[test]
fn escalation_candidates_are_priority_ordered() {
    let mut snapshot = empty_snapshot();
    snapshot.available_tickets.push(ticket_with_priority(
        "12",
        Some("#12"),
        1,
        Some("agent_no_progress"),
        false,
        false,
        TicketPriority::P2,
    ));
    snapshot.available_tickets.push(ticket_with_priority(
        "10",
        Some("#10"),
        1,
        Some("agent_no_progress"),
        false,
        false,
        TicketPriority::P1,
    ));

    let action = decide_next_action(&snapshot);
    match action {
        NextAction::Escalate { work_id, .. } => assert_eq!(work_id.as_str(), "#10"),
        other => panic!("expected Escalate for #10, got {other:?}"),
    }
}

#[test]
fn retry_candidates_are_priority_ordered() {
    let mut snapshot = empty_snapshot();
    snapshot.available_tickets.push(ticket_with_priority(
        "12",
        Some("#12"),
        1,
        Some("backend_error"),
        false,
        false,
        TicketPriority::P2,
    ));
    snapshot.available_tickets.push(ticket_with_priority(
        "10",
        Some("#10"),
        1,
        Some("backend_error"),
        false,
        false,
        TicketPriority::P1,
    ));
    snapshot.availability.push(ScopeStatusJson {
        backend_instance: None,
        backend: "codex".into(),
        model: None,
        quota_pool: None,
        eligible_now: true,
        reason: None,
        unavailable_until: None,
        source: None,
        last_error_summary: None,
        observed_at: None,
        scope: None,
    });

    let action = decide_next_action(&snapshot);
    match action {
        NextAction::Retry { work_id, .. } => assert_eq!(work_id.as_str(), "#10"),
        other => panic!("expected Retry for #10, got {other:?}"),
    }
}
