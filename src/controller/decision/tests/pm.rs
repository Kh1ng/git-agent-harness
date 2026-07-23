use super::{decide_next_action, empty_snapshot, NextAction};

fn planning_rejection() -> crate::models::IssueIntakeRejection {
    crate::models::IssueIntakeRejection {
        ticket_path: "#561".into(),
        work_id: Some("#561".into()),
        title: Some("Large story".into()),
        provider: "github".into(),
        author_login: Some("owner".into()),
        author_kind: Some("human".into()),
        reason_code: "planning".into(),
        reason: "planning label present".into(),
        labels: vec!["planning".into(), "exec:autonomous".into()],
    }
}

#[test]
fn planning_issue_selects_bounded_decomposition_not_implementation() {
    let mut snapshot = empty_snapshot();
    snapshot.issue_intake_rejections.push(planning_rejection());
    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::DecomposeIssue { work_id, .. } if work_id == "#561"
    ));
}

#[test]
fn published_parent_is_not_replanned_while_children_are_open() {
    let mut snapshot = empty_snapshot();
    snapshot.issue_intake_rejections.push(planning_rejection());
    snapshot
        .pm_parent_states
        .push(crate::status::PmParentStatus {
            work_id: "#561".into(),
            source_issue_number: "561".into(),
            plan_fingerprint: "plan-a".into(),
            child_issue_numbers: vec!["600".into()],
            open_child_count: 1,
            completed: false,
            reconciled: false,
        });
    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::NoOp { .. }
    ));
}

#[test]
fn terminal_children_select_reconciliation_without_closing_parent() {
    let mut snapshot = empty_snapshot();
    snapshot
        .pm_parent_states
        .push(crate::status::PmParentStatus {
            work_id: "#561".into(),
            source_issue_number: "561".into(),
            plan_fingerprint: "plan-a".into(),
            child_issue_numbers: vec!["600".into(), "601".into()],
            open_child_count: 0,
            completed: true,
            reconciled: false,
        });
    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::ReconcilePmParent { work_id, .. } if work_id == "#561"
    ));
}

#[test]
fn active_claim_prevents_parallel_decomposition() {
    let mut snapshot = empty_snapshot();
    snapshot.issue_intake_rejections.push(planning_rejection());
    snapshot
        .active_claims
        .push(crate::status::ActiveClaimSnapshot {
            work_id: "#561".into(),
            pid: 42,
            scope: "real@real".into(),
            hostname: "host".into(),
            claimed_at: "2026-07-19T00:00:00Z".into(),
            age_seconds: 1,
        });
    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::NoOp { .. }
    ));
}

#[test]
fn exhausted_pm_parent_does_not_block_unrelated_agent_ready_work() {
    let mut snapshot = empty_snapshot();
    snapshot.issue_intake_rejections.push(planning_rejection());
    snapshot
        .pm_decomposition_attempt_counts
        .insert("#561".into(), 2);
    snapshot
        .available_tickets
        .push(crate::models::AvailableTicket {
            ticket_path: "#562".into(),
            work_id: Some("#562".into()),
            normalized_work_identity: crate::work_claim::normalize_work_identity("#562"),
            source: crate::models::CandidateSource::LegacyTicket,
            execution_policy: crate::models::CandidateExecutionPolicy {
                intake_mode: "canonical_autonomous_only".into(),
                explicit_autonomy_required: true,
                autonomous_metadata_present: true,
                dispatchable_now: true,
                exclusion_reason_code: None,
                exclusion_reason: None,
            },
            title: Some("Independent fix".into()),
            recommended_backend: None,
            recommended_model: None,
            prior_attempt_count: 0,
            genuine_agent_failure_count: 0,
            last_failure_class: None,
            has_active_mr: false,
            human_required: false,
            human_required_reason_code: None,
            has_active_claim: false,
        });

    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::DispatchTicket { work_id: Some(work_id), .. } if work_id == "#562"
    ));
}
