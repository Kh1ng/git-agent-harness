use super::*;

fn external_criterion_four_context() -> ReviewGateContext {
    ReviewGateContext::from_diff_bundle(
        &ReviewDiffBundle {
            files: "docs/queue.md\n".to_string(),
            diff: "+- #146 is ready\n".to_string(),
        },
        Some("passed"),
    )
    .with_source_acceptance(
        vec![
            "Mirror the review contract".to_string(),
            "Document the result".to_string(),
            "Keep the source snapshot stable".to_string(),
            "List the current live GitLab issue queue".to_string(),
        ],
        "gitlab",
    )
}

#[test]
fn live_acceptance_criterion_four_is_approved_with_provider_grounding() {
    let json = r#"{
        "verdict":"APPROVE",
        "confidence":"high",
        "human_required":false,
        "blocking_findings":[],
        "non_blocking_findings":[],
        "risk_notes":[],
        "evidence":[
            "file:docs/queue.md",
            "ac:1:file:docs/queue.md",
            "ac:2:file:docs/queue.md",
            "ac:3:file:docs/queue.md",
            "ac:4:provider:gitlab:GET /projects/5/issues?state=opened returned #146"
        ]
    }"#;
    let verdict = parse_review_verdict_with_context(
        json,
        &route_decision("claude", Some("sonnet"), false),
        &crate::ledger::LedgerUsage::default(),
        ReviewerTier::Strong,
        &external_criterion_four_context(),
    )
    .unwrap();

    assert_eq!(verdict.verdict, "APPROVE");
    assert!(!verdict.human_required);
}

#[test]
fn live_acceptance_criterion_four_rejects_file_only_grounding() {
    let json = r#"{
        "verdict":"APPROVE",
        "confidence":"high",
        "human_required":false,
        "blocking_findings":[],
        "non_blocking_findings":[],
        "risk_notes":[],
        "evidence":[
            "file:docs/queue.md",
            "ac:1:file:docs/queue.md",
            "ac:2:file:docs/queue.md",
            "ac:3:file:docs/queue.md",
            "ac:4:file:docs/queue.md"
        ]
    }"#;
    let verdict = parse_review_verdict_with_context(
        json,
        &route_decision("claude", Some("sonnet"), false),
        &crate::ledger::LedgerUsage::default(),
        ReviewerTier::Strong,
        &external_criterion_four_context(),
    )
    .unwrap();

    assert_eq!(verdict.verdict, "NEEDS_FIX");
    assert!(verdict
        .safety_gate_reason
        .as_deref()
        .unwrap_or_default()
        .contains("direct provider evidence or a testable changed snapshot"));
}
