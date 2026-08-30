use super::*;

const CURRENT_REVIEW: &str = "review-v1:current-head:sha256:current-metadata";

fn reviewed_mr(branch: &str, classification: &str, verdict: &str) -> crate::sync::SyncMrJson {
    let mut mr = mr(branch, classification);
    mr.review_generation = Some(CURRENT_REVIEW.into());
    mr.review_verdict = Some(verdict.into());
    mr
}

#[test]
fn needs_review_mr_takes_priority() {
    let mut snapshot = empty_snapshot();
    snapshot
        .merge_requests
        .push(reviewed_mr("gah/real-1", "NEEDS_FIX", "NEEDS_FIX"));
    snapshot
        .merge_requests
        .push(mr("gah/real-2", "NEEDS_REVIEW"));

    match decide_next_action(&snapshot) {
        NextAction::ReviewMr { branch, .. } => assert_eq!(branch, "gah/real-2"),
        other => panic!("expected ReviewMr, got {other:?}"),
    }
}

#[test]
fn ci_failed_mr_with_stale_review_triggers_review() {
    let mut snapshot = empty_snapshot();
    let mut failed = reviewed_mr("gah/real-1", "CI_FAILED", "NEEDS_FIX");
    failed.review_generation_status =
        Some("superseded review because source or provider metadata changed".into());
    snapshot.merge_requests.push(failed);

    match decide_next_action(&snapshot) {
        NextAction::ReviewMr { branch, reason, .. } => {
            assert_eq!(branch, "gah/real-1");
            assert!(reason.contains("no completed current review"));
        }
        other => panic!("expected ReviewMr, got {other:?}"),
    }
}

#[test]
fn ci_failed_mr_with_current_review_triggers_fix() {
    let mut snapshot = empty_snapshot();
    snapshot
        .merge_requests
        .push(reviewed_mr("gah/real-1", "CI_FAILED", "NEEDS_FIX"));

    match decide_next_action(&snapshot) {
        NextAction::FixMr {
            branch,
            review_generation,
            ..
        } => {
            assert_eq!(branch, "gah/real-1");
            assert_eq!(review_generation.as_deref(), Some(CURRENT_REVIEW));
        }
        other => panic!("expected FixMr, got {other:?}"),
    }
}

#[test]
fn needs_fix_mr_without_current_review_triggers_review() {
    let mut snapshot = empty_snapshot();
    snapshot.merge_requests.push(mr("gah/real-1", "NEEDS_FIX"));

    match decide_next_action(&snapshot) {
        NextAction::ReviewMr { branch, reason, .. } => {
            assert_eq!(branch, "gah/real-1");
            assert!(reason.contains("no completed current review"));
        }
        other => panic!("expected ReviewMr, got {other:?}"),
    }
}

#[test]
fn needs_fix_mr_with_current_review_triggers_fix() {
    let mut snapshot = empty_snapshot();
    snapshot
        .merge_requests
        .push(reviewed_mr("gah/real-1", "NEEDS_FIX", "REJECT"));

    match decide_next_action(&snapshot) {
        NextAction::FixMr { branch, .. } => assert_eq!(branch, "gah/real-1"),
        other => panic!("expected FixMr, got {other:?}"),
    }
}
