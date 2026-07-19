use super::*;

#[test]
fn approved_green_draft_finishes_lifecycle_before_starting_another_review() {
    let mut snapshot = empty_snapshot();
    snapshot
        .merge_requests
        .push(mr("gah/review", "NEEDS_REVIEW"));
    let mut approved = mr_with_ci("gah/approved", "READY_FOR_HUMAN", true);
    approved.draft = true;
    snapshot.merge_requests.push(approved);

    let action = decide_next_action(&snapshot);
    match action {
        NextAction::MarkReadyForReview { branch, .. } => assert_eq!(branch, "gah/approved"),
        other => panic!("expected MarkReadyForReview, got {other:?}"),
    }
}

#[test]
fn approved_green_mr_merges_before_starting_another_review() {
    let mut snapshot = empty_snapshot();
    snapshot
        .merge_requests
        .push(mr("gah/review", "NEEDS_REVIEW"));
    let mut approved = mr_with_ci("gah/approved", "READY_FOR_HUMAN", true);
    approved.review_generation = Some("review-v1:source:sha256:metadata".into());
    snapshot.merge_requests.push(approved);

    let action = decide_next_action(&snapshot);
    match action {
        NextAction::MergeMr {
            branch,
            review_generation,
            ..
        } => {
            assert_eq!(branch, "gah/approved");
            assert_eq!(
                review_generation.as_deref(),
                Some("review-v1:source:sha256:metadata")
            );
        }
        other => panic!("expected MergeMr, got {other:?}"),
    }
}
