use super::*;

#[test]
fn format_only_prose_violation_gets_repair_instructions() {
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
    assert!(context
        .repair_instructions(&violation)
        .is_some_and(|text| text.contains("ONLY the inert heading")));
    let repaired = parse_review_verdict_with_context(
        repaired_text,
        &route,
        &usage,
        ReviewerTier::Strong,
        &context,
    )
    .unwrap();
    assert_eq!(repaired.verdict, "APPROVE");
    assert!(context.repair_instructions(&repaired).is_none());
}

const STRONG_APPROVE: &str = r#"{"verdict":"APPROVE","confidence":"high","human_required":false,
    "blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["FILE","ci:passed"]}"#;

fn approve_for(
    file: &str,
    diff: &str,
    hold: bool,
) -> (crate::models::ReviewVerdict, ReviewGateContext) {
    let context = ReviewGateContext::from_diff_bundle(
        &ReviewDiffBundle {
            files: format!("{file}\n"),
            diff: diff.to_string(),
        },
        Some("passed"),
    )
    .with_contract_hold(hold);
    let verdict = parse_review_verdict_with_context(
        &STRONG_APPROVE.replace("FILE", &format!("file:{file}")),
        &route_decision("codex", Some("gpt-6.1-sol"), false),
        &crate::ledger::LedgerUsage::default(),
        ReviewerTier::Strong,
        &context,
    )
    .unwrap();
    (verdict, context)
}

/// #1405: internal `pub` items and server code are not persisted or wire
/// contracts; a strong APPROVE on them must not become a human handoff.
#[test]
fn internal_pub_items_and_server_code_are_not_contract_surfaces() {
    for (file, diff) in [
        (
            "src/controller/runtime.rs",
            "+pub fn report_blocked_item() {}\n",
        ),
        ("src/auth_health.rs", "+pub struct InstanceRow {}\n"),
        (
            "apps/server/src/authHealth.ts",
            "+export function parse() {}\n",
        ),
        ("apps/web/src/api/client.ts", "+  hold: boolean;\n"),
    ] {
        let (verdict, _) = approve_for(file, diff, true);
        assert_eq!(verdict.verdict, "APPROVE", "{file}");
        assert!(!verdict.human_required, "{file}");
        assert!(verdict.safety_gate_reason.is_none(), "{file}");
    }
}

#[test]
fn ledger_contract_change_is_held_and_repairable_only_while_setting_is_on() {
    let diff = "-    pub attempts: u32,\n+    pub attempts: Option<u32>,\n";

    let (held, context) = approve_for("src/ledger/entry.rs", diff, true);
    assert_eq!(held.verdict, "HUMAN_REVIEW");
    let instructions = context.repair_instructions(&held).unwrap();
    assert!(instructions.contains("src/ledger/entry.rs"));
    assert!(instructions.contains("NEEDS_FIX"));

    let (approved, _) = approve_for("src/ledger/entry.rs", diff, false);
    assert_eq!(approved.verdict, "APPROVE");
    assert!(!approved.human_required);
}
