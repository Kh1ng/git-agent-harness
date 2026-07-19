use super::*;

#[test]
fn final_review_handoff_is_not_re_reviewed_each_loop_tick() {
    let mut snapshot = empty_snapshot();
    snapshot.merge_requests.push(mr("gah/42", "NEEDS_REVIEW"));
    snapshot.blocked_work_items.push(Blocker {
        kind: "human_required".into(),
        reason: Some("review_escalation_exhausted".into()),
        message: Some("all configured reviewers were tried".into()),
        backend: None,
        model: None,
        until: None,
        source_reference: Some("TICKET-gah/42".into()),
        reason_code: None,
    });

    assert_eq!(decide_next_action(&snapshot).kind(), "no_op");
}
