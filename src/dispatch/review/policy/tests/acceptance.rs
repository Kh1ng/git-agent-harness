use super::*;

fn annotated_evidence_context() -> ReviewGateContext {
    ReviewGateContext::from_diff_bundle(
        &ReviewDiffBundle {
            files: "docs/morning_command_sheet.md\n".to_string(),
            diff: "+# Morning command sheet\n".to_string(),
        },
        Some("pending"),
    )
    .with_source_acceptance(vec!["Commands are copy/pasteable".to_string()], "gitlab")
}

#[test]
fn approval_accepts_annotated_changed_file_acceptance_evidence() {
    let json = r#"{
        "verdict":"APPROVE",
        "confidence":"high",
        "human_required":false,
        "blocking_findings":[],
        "non_blocking_findings":[],
        "risk_notes":[],
        "evidence":[
            "file:docs/morning_command_sheet.md",
            "ac:1:file:docs/morning_command_sheet.md — commands are copy/pasteable"
        ]
    }"#;
    let route = route_decision("agy", Some("Claude Sonnet 4.6 (Thinking)"), false);

    let verdict = parse_review_verdict_with_context(
        json,
        &route,
        &crate::ledger::LedgerUsage::default(),
        ReviewerTier::Strong,
        &annotated_evidence_context(),
    )
    .unwrap();

    assert_eq!(verdict.verdict, "APPROVE");
    assert!(verdict.safety_gate_reason.is_none());
}

#[test]
fn approval_rejects_changed_file_prefix_spoofing_in_acceptance_evidence() {
    let json = r#"{
        "verdict":"APPROVE",
        "confidence":"high",
        "human_required":false,
        "blocking_findings":[],
        "non_blocking_findings":[],
        "risk_notes":[],
        "evidence":[
            "file:docs/morning_command_sheet.md",
            "ac:1:file:docs/morning_command_sheet.md.untracked"
        ]
    }"#;
    let route = route_decision("agy", Some("Claude Sonnet 4.6 (Thinking)"), false);

    let verdict = parse_review_verdict_with_context(
        json,
        &route,
        &crate::ledger::LedgerUsage::default(),
        ReviewerTier::Strong,
        &annotated_evidence_context(),
    )
    .unwrap();

    assert_eq!(verdict.verdict, "NEEDS_FIX");
    assert!(verdict
        .safety_gate_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("was not grounded")));
}

/// Regression: a criterion that merely *names* the provider while describing
/// test coverage (e.g. "Unit tests cover GitHub and GitLab issue bodies...")
/// must not be forced into requiring `provider:`/`snapshot:` evidence just
/// because it contains the word "github"/"gitlab" -- in a tool whose purpose
/// is GitHub/GitLab integration, that would misclassify nearly every
/// criterion in a provider-facing ticket, rejecting perfectly good
/// `test:`-grounded evidence as ungrounded.
#[test]
fn criterion_naming_a_provider_for_test_coverage_is_not_external_state() {
    let json = r#"{
        "verdict":"APPROVE",
        "confidence":"high",
        "human_required":false,
        "blocking_findings":[],
        "non_blocking_findings":[],
        "risk_notes":[],
        "evidence":[
            "file:src/parser.rs",
            "ac:1:test:cargo test github_issue_response_survives_parsing -> ok"
        ]
    }"#;
    let route = route_decision("claude", Some("sonnet"), false);
    let context = ReviewGateContext::from_diff_bundle(
        &ReviewDiffBundle {
            files: "src/parser.rs\n".to_string(),
            diff: "+fn parse() {}\n".to_string(),
        },
        Some("passed"),
    )
    .with_source_acceptance(
        vec![
            "Unit tests cover GitHub and GitLab issue bodies with numbered acceptance criteria"
                .to_string(),
        ],
        "github",
    );

    let verdict = parse_review_verdict_with_context(
        json,
        &route,
        &crate::ledger::LedgerUsage::default(),
        ReviewerTier::Strong,
        &context,
    )
    .unwrap();

    assert_eq!(verdict.verdict, "APPROVE");
    assert!(!verdict.human_required);
}
