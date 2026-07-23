use super::*;

fn add_fresh_ticket(snapshot: &mut StatusSnapshot) {
    snapshot.available_tickets.push(ticket(
        "docs/tickets/next.md",
        Some("#709"),
        0,
        None,
        false,
        false,
    ));
}

#[test]
fn managed_mr_limit_pauses_fresh_implementation_intake() {
    let mut snapshot = empty_snapshot();
    snapshot.profile.max_open_managed_mrs = 3;
    snapshot.open_managed_mr_count = 3;
    snapshot.implementation_intake_paused = true;
    add_fresh_ticket(&mut snapshot);

    let action = decide_next_action(&snapshot);

    let NextAction::NoOp { reason } = action else {
        panic!("expected drain-mode no-op, got {action:?}");
    };
    assert!(reason.contains("3 open managed MR(s)"));
    assert!(reason.contains("limit 3"));
}

#[test]
fn in_flight_implementation_consumes_the_last_intake_slot() {
    let mut snapshot = empty_snapshot();
    snapshot.profile.max_open_managed_mrs = 3;
    snapshot.open_managed_mr_count = 2;
    snapshot.inflight_implementation_count = 1;
    snapshot.implementation_intake_paused = true;
    add_fresh_ticket(&mut snapshot);

    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::NoOp { .. }
    ));
}

#[test]
fn lifecycle_work_still_drains_at_the_managed_mr_limit() {
    let mut snapshot = empty_snapshot();
    snapshot.profile.max_open_managed_mrs = 1;
    snapshot.open_managed_mr_count = 1;
    snapshot.implementation_intake_paused = true;
    snapshot
        .merge_requests
        .push(mr("gah/review", "NEEDS_REVIEW"));
    add_fresh_ticket(&mut snapshot);

    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::ReviewMr { .. }
    ));
}

#[test]
fn managed_mr_limit_reports_the_durable_gate_instead_of_blaming_backpressure() {
    let mut snapshot = empty_snapshot();
    snapshot.profile.max_open_managed_mrs = 1;
    snapshot.open_managed_mr_count = 1;
    snapshot.implementation_intake_paused = true;
    snapshot
        .merge_requests
        .push(mr("gah/review", "NEEDS_REVIEW"));
    snapshot.blocked_work_items.push(crate::status::Blocker {
        kind: "human_required".into(),
        reason: Some("stuck_loop_gate".into()),
        message: Some("review selected repeatedly without progress".into()),
        backend: None,
        model: None,
        until: None,
        source_reference: Some("TICKET-gah/review".into()),
        reason_code: Some("stuck_loop_gate".into()),
        remediation_plan: None,
    });
    add_fresh_ticket(&mut snapshot);

    let action = decide_next_action(&snapshot);

    assert!(matches!(
        action,
        NextAction::HumanRequired {
            reason,
            reason_code: Some(ref code),
            ..
        } if code == "stuck_loop_gate"
            && reason == "review selected repeatedly without progress"
    ));
}

#[test]
fn managed_mr_limit_surfaces_active_review_hold_instead_of_backpressure_noop() {
    let mut snapshot = empty_snapshot();
    snapshot.profile.max_open_managed_mrs = 1;
    snapshot.open_managed_mr_count = 1;
    snapshot.implementation_intake_paused = true;
    snapshot
        .merge_requests
        .push(mr_with_ci("gah/held", "READY_FOR_HUMAN", true));
    snapshot
        .review_held_work_ids
        .insert("TICKET-gah/held".into());
    add_fresh_ticket(&mut snapshot);

    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::WaitUntil { reason, .. }
            if reason.contains("active manager review hold")
                && reason.contains("gah/held")
    ));
}

#[test]
fn implementation_resumes_below_the_managed_mr_limit() {
    let mut snapshot = empty_snapshot();
    snapshot.profile.max_open_managed_mrs = 3;
    snapshot.open_managed_mr_count = 2;
    add_fresh_ticket(&mut snapshot);

    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::DispatchTicket { .. }
    ));
}
