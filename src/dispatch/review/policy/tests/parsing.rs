use super::*;

fn vibe_route(model: Option<&str>) -> crate::routing::RouteDecision {
    crate::routing::RouteDecision::from_identity(
        crate::execution_identity::ExecutionIdentity::legacy_route(
            "vibe",
            model,
            "vibe",
            model,
            None::<String>,
        ),
        "test".to_string(),
        false,
        None,
        false,
        None,
    )
}

#[test]
fn parse_review_verdict_handles_vibe_json_output() {
    let output = r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["vibe inspected the diff"]}"#;
    let route = vibe_route(Some("mistral-medium-3.5"));
    let usage = crate::ledger::LedgerUsage::default();

    let verdict = parse_review_verdict(output, &route, &usage, ReviewerTier::Standard).unwrap();

    assert_eq!(verdict.verdict, "APPROVE");
    assert_eq!(verdict.confidence, "high");
    assert!(!verdict.human_required);
    assert!(verdict.blocking_findings.is_empty());
    assert!(verdict.non_blocking_findings.is_empty());
    assert!(verdict.risk_notes.is_empty());
    assert_eq!(verdict.reviewer_backend.as_deref(), Some("vibe"));
    assert_eq!(verdict.effective_backend.as_deref(), Some("vibe"));
    assert_eq!(
        verdict.effective_model.as_deref(),
        Some("mistral-medium-3.5")
    );
}

#[test]
fn parse_review_verdict_fails_on_vibe_malformed_json() {
    let route = vibe_route(None);
    let usage = crate::ledger::LedgerUsage::default();
    let result = parse_review_verdict(
        "This is not valid JSON from Vibe",
        &route,
        &usage,
        ReviewerTier::Standard,
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("reviewer did not return verdict JSON"));
}

#[test]
fn parse_review_verdict_fails_on_vibe_empty_output() {
    let route = vibe_route(None);
    let usage = crate::ledger::LedgerUsage::default();
    let result = parse_review_verdict("", &route, &usage, ReviewerTier::Standard);
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("reviewer did not return verdict JSON"));
}

#[test]
fn parse_review_verdict_skips_incidental_empty_braces_in_prose() {
    let review_text = r##"## Review Notes

### Correctness

Found an issue: `find_header_u64` uses `r#"(?i)"?{}\b"?\s*[:=]\s*"?([0-9]+)"?"#`
which lacks a leading boundary check.

## JSON Summary

```json
{
  "verdict": "NEEDS_FIX",
  "confidence": "high",
  "human_required": false,
  "blocking_findings": ["regex lacks leading boundary assertion"],
  "non_blocking_findings": [],
  "risk_notes": []
}
```
"##;
    let route = vibe_route(Some("mistral-medium-3.5"));
    let usage = crate::ledger::LedgerUsage::default();

    let verdict =
        parse_review_verdict(review_text, &route, &usage, ReviewerTier::Standard).unwrap();

    assert_eq!(verdict.verdict, "NEEDS_FIX");
    assert_eq!(verdict.confidence, "high");
    assert_eq!(
        verdict.blocking_findings,
        vec!["regex lacks leading boundary assertion".to_string()]
    );
}
