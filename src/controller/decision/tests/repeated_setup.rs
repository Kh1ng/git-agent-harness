use super::*;

#[test]
fn repeated_setup_gate_prevents_dispatch_and_retry() {
    let mut snapshot = empty_snapshot();
    let mut candidate = ticket(
        "setup",
        Some("#1462"),
        3,
        Some("harness_error"),
        false,
        true,
    );
    candidate.human_required = true;
    candidate.human_required_reason_code = Some("repeated_setup_failure".into());
    snapshot.available_tickets.push(candidate);
    snapshot.availability.push(ScopeStatusJson {
        backend_instance: None,
        backend: "codex".into(),
        model: None,
        quota_pool: None,
        eligible_now: true,
        reason: None,
        unavailable_until: None,
        source: None,
        last_error_summary: None,
        observed_at: None,
        scope: None,
    });
    assert_eq!(decide_next_action(&snapshot).kind(), "no_op");
}
