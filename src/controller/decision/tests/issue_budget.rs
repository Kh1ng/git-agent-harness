//! `routing.issue_budget`: the controller refuses to spend attempts an
//! issue no longer has, and keeps the escalation reserve for escalation.

use super::*;
use crate::status::{IssueBudgetStatus, WorkWaypointEvidence};

fn budget(
    attempts_used: u32,
    max_attempts: u32,
    escalation_only: bool,
    exhausted: bool,
) -> IssueBudgetStatus {
    IssueBudgetStatus {
        attempts_used,
        max_attempts: Some(max_attempts),
        escalation_reserve_attempts: 1,
        elapsed_minutes: 0,
        max_elapsed_minutes: None,
        manager_rounds_used: 0,
        max_manager_rounds: None,
        escalation_only,
        exhausted,
        hold_reason: exhausted.then(|| "issue budget spent".to_string()),
    }
}

fn with_budget(snapshot: &mut StatusSnapshot, work_id: &str, status: IssueBudgetStatus) {
    snapshot.work_waypoint_evidence.insert(
        work_id.to_string(),
        WorkWaypointEvidence {
            issue_budget: Some(status),
            ..WorkWaypointEvidence::default()
        },
    );
}

fn eligible_backend(snapshot: &mut StatusSnapshot) {
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
}

#[test]
fn reserve_refuses_a_same_tier_retry_of_an_infra_failure() {
    let mut snapshot = empty_snapshot();
    eligible_backend(&mut snapshot);
    snapshot.available_tickets.push(ticket(
        "docs/tickets/TICKET-42.md",
        Some("#42"),
        1,
        Some("harness_error"),
        false,
        false,
    ));
    with_budget(&mut snapshot, "#42", budget(1, 2, true, false));

    let action = decide_next_action(&snapshot);

    assert!(
        matches!(action, NextAction::NoOp { .. }),
        "reserve must refuse a plain retry, got {action:?}"
    );
}

#[test]
fn reserve_still_allows_an_escalation_of_an_agent_failure() {
    let mut snapshot = empty_snapshot();
    eligible_backend(&mut snapshot);
    snapshot.available_tickets.push(ticket(
        "docs/tickets/TICKET-42.md",
        Some("#42"),
        1,
        Some("agent_failure"),
        false,
        false,
    ));
    with_budget(&mut snapshot, "#42", budget(1, 2, true, false));

    match decide_next_action(&snapshot) {
        NextAction::Escalate { work_id, .. } => assert_eq!(work_id, "#42"),
        other => panic!("expected Escalate inside the reserve, got {other:?}"),
    }
}

#[test]
fn spent_budget_refuses_escalation_and_lets_other_work_flow() {
    let mut snapshot = empty_snapshot();
    eligible_backend(&mut snapshot);
    snapshot.available_tickets.push(ticket(
        "docs/tickets/TICKET-42.md",
        Some("#42"),
        2,
        Some("agent_failure"),
        false,
        false,
    ));
    snapshot.available_tickets.push(ticket(
        "docs/tickets/TICKET-43.md",
        Some("#43"),
        0,
        None,
        false,
        false,
    ));
    with_budget(&mut snapshot, "#42", budget(2, 2, false, true));

    match decide_next_action(&snapshot) {
        NextAction::DispatchTicket { work_id, .. } => assert_eq!(work_id.as_deref(), Some("#43")),
        other => panic!("expected the fresh ticket to dispatch, got {other:?}"),
    }
}

#[test]
fn spent_budget_refuses_a_fresh_dispatch_of_the_same_issue() {
    let mut snapshot = empty_snapshot();
    snapshot.available_tickets.push(ticket(
        "docs/tickets/TICKET-42.md",
        Some("#42"),
        0,
        None,
        false,
        false,
    ));
    with_budget(&mut snapshot, "#42", budget(3, 3, false, true));

    let action = decide_next_action(&snapshot);

    assert!(
        matches!(action, NextAction::NoOp { .. }),
        "a spent budget must refuse dispatch, got {action:?}"
    );
}

#[test]
fn a_budget_with_room_changes_nothing() {
    let mut snapshot = empty_snapshot();
    eligible_backend(&mut snapshot);
    snapshot.available_tickets.push(ticket(
        "docs/tickets/TICKET-42.md",
        Some("#42"),
        1,
        Some("harness_error"),
        false,
        false,
    ));
    with_budget(&mut snapshot, "#42", budget(1, 4, false, false));

    match decide_next_action(&snapshot) {
        NextAction::Retry { work_id, .. } => assert_eq!(work_id, "#42"),
        other => panic!("expected Retry with budget to spare, got {other:?}"),
    }
}
