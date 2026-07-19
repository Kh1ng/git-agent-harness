use super::NextAction;

/// Existing-branch repairs are intentionally launched against the MR's
/// current state, which may already be red for the exact defect the repair is
/// expected to fix. Let that unknown-red result reach the repair backend while
/// preserving the dispatch layer's unconditional stop for harness and
/// environment failures.
///
/// All other action types retain the direct-dispatch default: an unclassified
/// red baseline fails closed unless an operator explicitly overrides it.
pub(super) fn allow_unknown_red_baseline(action: &NextAction) -> bool {
    matches!(action, NextAction::FixMr { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fix_mr_allows_an_unknown_red_baseline_automatically() {
        assert!(allow_unknown_red_baseline(&NextAction::FixMr {
            work_id: Some("#624".into()),
            branch: "gah/repair".into(),
            mr_url: Some("https://example.test/pull/624".into()),
            review_generation: Some("review-v1:source:sha256:metadata".into()),
            reason: "review requested changes".into(),
        }));
        assert!(!allow_unknown_red_baseline(&NextAction::DispatchTicket {
            ticket_path: "#719".into(),
            work_id: Some("#719".into()),
            recommended_backend: None,
            recommended_model: None,
            reason: "new work".into(),
        }));
        assert!(!allow_unknown_red_baseline(&NextAction::Retry {
            work_id: "#719".into(),
            ticket_path: "#719".into(),
            reason: "retry".into(),
        }));
        assert!(!allow_unknown_red_baseline(&NextAction::Escalate {
            work_id: "#719".into(),
            ticket_path: "#719".into(),
            reason: "escalate".into(),
        }));
    }
}
