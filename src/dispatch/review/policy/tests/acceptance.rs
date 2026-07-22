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

fn gitlab_provider_auth_evidence_context() -> ReviewGateContext {
    ReviewGateContext::from_diff_bundle(
        &ReviewDiffBundle {
            files: "tests/doctor_provider_auth.rs\n".to_string(),
            diff: "+// GitLab provider auth passes via glab CLI session\n".to_string(),
        },
        Some("passed"),
    )
    .with_source_acceptance(
        vec![
            "GitLab profiles authenticated via glab CLI session pass doctor without a PAT"
                .to_string(),
        ],
        "gitlab",
    )
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
fn approval_accepts_provider_grounded_gitlab_acceptance_evidence() {
    let json = r#"{
        "verdict":"APPROVE",
        "confidence":"high",
        "human_required":false,
        "blocking_findings":[],
        "non_blocking_findings":[],
        "risk_notes":[],
        "evidence":[
            "file:tests/doctor_provider_auth.rs",
            "ac:1:provider:gitlab:glab api /projects/42 returned authenticated session"
        ]
    }"#;
    let route = route_decision("agy", Some("Claude Sonnet 4.6 (Thinking)"), false);

    let verdict = parse_review_verdict_with_context(
        json,
        &route,
        &crate::ledger::LedgerUsage::default(),
        ReviewerTier::Strong,
        &gitlab_provider_auth_evidence_context(),
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
