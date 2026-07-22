use super::*;

fn grounded_context() -> ReviewGateContext {
    ReviewGateContext::from_diff_bundle(
        &ReviewDiffBundle {
            files: "src/lib.rs\n".to_string(),
            diff: "+pub fn guarded_retry() {}\n".to_string(),
        },
        Some("success"),
    )
}

fn parse_grounded(json: &str) -> anyhow::Result<crate::models::ReviewVerdict> {
    parse_review_verdict_with_context(
        json,
        &route_decision("agy", Some("Claude Sonnet 4.6 (Thinking)"), false),
        &crate::ledger::LedgerUsage::default(),
        ReviewerTier::Strong,
        &grounded_context(),
    )
}

#[test]
fn confirmed_actionable_finding_becomes_repair_context() {
    let verdict = parse_grounded(
        r#"{
          "verdict":"NEEDS_FIX",
          "confidence":"high",
          "human_required":false,
          "actionable_findings":[{
            "summary":"retry counter is incremented after the terminal check",
            "file":"src/lib.rs",
            "line":"42",
            "status":"confirmed",
            "evidence":["diff:src/lib.rs:the added increment follows the return guard"]
          }],
          "non_blocking_findings":[],
          "risk_notes":[],
          "evidence":["file:src/lib.rs"],
          "compatibility_evidence":[]
        }"#,
    )
    .unwrap();

    assert_eq!(verdict.actionable_findings.len(), 1);
    assert_eq!(
        verdict.blocking_findings,
        ["src/lib.rs:42: retry counter is incremented after the terminal check"]
    );
}

#[test]
fn repair_verdict_without_typed_actionable_findings_is_invalid() {
    let err = parse_grounded(
        r#"{
          "verdict":"NEEDS_FIX",
          "confidence":"high",
          "human_required":false,
          "blocking_findings":["src/lib.rs: maybe broken"],
          "non_blocking_findings":[],
          "risk_notes":[],
          "evidence":["file:src/lib.rs"]
        }"#,
    )
    .unwrap_err();

    assert_eq!(
        review_output_invalid_error(&err).unwrap().reason(),
        "NEEDS_FIX/REJECT omitted actionable_findings"
    );
}

#[test]
fn live_self_refuting_findings_cannot_drive_repairs() {
    for summary in [
        "Re-examining this path: it is actually fine.",
        "This finding is withdrawn — NOT blocking.",
        "This is an unverified risk that cannot be confirmed from the diff alone.",
    ] {
        let json = serde_json::json!({
            "verdict": "NEEDS_FIX",
            "confidence": "high",
            "human_required": false,
            "actionable_findings": [{
                "summary": summary,
                "file": "src/lib.rs",
                "line": "42",
                "status": "confirmed",
                "evidence": ["diff:src/lib.rs:the changed branch was inspected"]
            }],
            "non_blocking_findings": [],
            "risk_notes": [],
            "evidence": ["file:src/lib.rs"]
        });
        let err = parse_grounded(&json.to_string()).unwrap_err();
        assert!(review_output_invalid_error(&err)
            .unwrap()
            .reason()
            .contains("withdrew, contradicted, or left the finding unverified"));
    }
}

#[test]
fn actionable_finding_requires_exact_changed_file_and_direct_diff_evidence() {
    let wrong_file = r#"{
      "verdict":"REJECT","confidence":"high","human_required":false,
      "actionable_findings":[{
        "summary":"terminal state is skipped","file":"src/other.rs","line":null,
        "status":"confirmed","evidence":["diff:src/other.rs:guard is absent"]
      }]
    }"#;
    assert!(
        review_output_invalid_error(&parse_grounded(wrong_file).unwrap_err())
            .unwrap()
            .reason()
            .contains("exact changed file")
    );

    let ungrounded = r#"{
      "verdict":"NEEDS_FIX","confidence":"high","human_required":false,
      "actionable_findings":[{
        "summary":"terminal state is skipped","file":"src/lib.rs","line":null,
        "status":"confirmed","evidence":["test:cargo test:failed"]
      }]
    }"#;
    assert!(
        review_output_invalid_error(&parse_grounded(ungrounded).unwrap_err())
            .unwrap()
            .reason()
            .contains("direct diff:src/lib.rs")
    );
}

#[test]
fn invalid_output_advances_through_ordered_review_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let candidates = vec![
        CandidateConfig {
            backend: "agy".into(),
            model: Some("sonnet".into()),
            ..CandidateConfig::default()
        },
        CandidateConfig {
            backend: "agy-second".into(),
            model: Some("sonnet".into()),
            ..CandidateConfig::default()
        },
        CandidateConfig {
            backend: "claude".into(),
            model: Some("sonnet".into()),
            ..CandidateConfig::default()
        },
    ];
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            review_candidates: Some(candidates),
            ..RoutingPolicy::default()
        },
    );
    let mut prof = profile(tmp.path());
    prof.routing = RoutingPolicy::default();

    let mut first = review_ledger_entry(
        "test",
        &prof,
        "gah/review-output",
        "review_output_invalid",
        "unknown",
    );
    first.effective_backend = "agy".into();
    first.effective_model = Some("sonnet".into());
    first.review_verdict = Some("REVIEW_OUTPUT_INVALID".into());
    crate::ledger::append(&cfg, &first).unwrap();

    assert_eq!(
        review_escalation_reason(&cfg, &prof, "test", "gah/review-output"),
        Some("review_output_invalid")
    );
    let second = next_review_candidate(&cfg, &prof, "test", "gah/review-output", None).unwrap();
    assert_eq!(second.backend, "agy-second");

    let mut second_entry = first.clone();
    second_entry.timestamp = "2099-01-01T00:00:00Z".into();
    second_entry.effective_backend = "agy-second".into();
    crate::ledger::append(&cfg, &second_entry).unwrap();
    let third = next_review_candidate(&cfg, &prof, "test", "gah/review-output", None).unwrap();
    assert_eq!(third.backend, "claude");
}

/// TICKET-739 regression: a `deferred_capacity` entry is a routing-decision
/// failure -- no backend ever launched, no verdict was ever produced. Before
/// this fix, both escalation pickers still counted it as a spent attempt, so
/// once the only other configured candidate was a paid, `requires_approval`
/// route, the escalation chain had nothing left to try but that gated route
/// forever, even after the deferred candidate's transient unavailability
/// (e.g. a quota cooldown) had since cleared. This is the sportsball-bets
/// #150 livelock: the escalation chain must still offer the deferred
/// candidate up again instead of skipping straight to the paid gate.
#[test]
fn deferred_capacity_failure_does_not_retire_the_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    let candidates = vec![
        CandidateConfig {
            backend: "claude".into(),
            model: Some("sonnet".into()),
            ..CandidateConfig::default()
        },
        CandidateConfig {
            backend: "opencode".into(),
            model: Some("nous-portal/z-ai/glm-5.2".into()),
            requires_approval: true,
            ..CandidateConfig::default()
        },
    ];
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            escalatory_reviewers: candidates.clone(),
            review_candidates: Some(candidates),
            ..RoutingPolicy::default()
        },
    );
    let mut prof = profile(tmp.path());
    prof.routing = RoutingPolicy::default();

    let mut deferred =
        review_ledger_entry("test", &prof, "gah/livelock", "deferred_capacity", "high");
    deferred.effective_backend = "claude".into();
    deferred.effective_model = Some("sonnet".into());
    crate::ledger::append(&cfg, &deferred).unwrap();

    let next = next_escalatory_reviewer(&cfg, &prof, "test", "gah/livelock", None)
        .expect("a deferred_capacity failure must not exhaust the escalation chain");
    assert_eq!(
        (next.backend.as_str(), next.model.as_deref()),
        ("claude", Some("sonnet")),
        "claude was never actually attempted and must be retried before the paid gate"
    );

    let next_review = next_review_candidate(&cfg, &prof, "test", "gah/livelock", None)
        .expect("a deferred_capacity failure must not exhaust the review candidate pool");
    assert_eq!(
        (next_review.backend.as_str(), next_review.model.as_deref()),
        ("claude", Some("sonnet"))
    );
}
