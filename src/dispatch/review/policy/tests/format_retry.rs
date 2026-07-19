use super::*;

#[test]
fn format_only_prose_violation_is_retryable_once_then_escalates_on_repeat() {
    let review_text = "Found a worrying edge case.\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src/dispatch.rs\",\"ci:passed\"]}";
    let repaired_text =
        "{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src/dispatch.rs\"]}";
    let usage = crate::ledger::LedgerUsage::default();
    let route = route_decision("claude", Some("sonnet"), false);
    let context = ReviewGateContext::from_diff_bundle(
        &ReviewDiffBundle {
            files: "src/dispatch.rs\n".to_string(),
            diff: "+fn hardened_review() {}\n".to_string(),
        },
        Some("passed"),
    );

    let violation = parse_review_verdict_with_context(
        review_text,
        &route,
        &usage,
        ReviewerTier::Strong,
        &context,
    )
    .unwrap();
    assert!(is_retryable_format_only_violation(&violation, false));
    assert!(!is_retryable_format_only_violation(&violation, true));
    let repaired = parse_review_verdict_with_context(
        repaired_text,
        &route,
        &usage,
        ReviewerTier::Strong,
        &context,
    )
    .unwrap();
    assert_eq!(repaired.verdict, "APPROVE");
    assert!(!is_retryable_format_only_violation(&repaired, false));
}
