use super::*;
use crate::config::Defaults;
use crate::ledger::{RoutingCandidateDiagnostic, RoutingDiagnostics};

#[test]
fn projects_exact_requests_and_revocable_grants_without_crossing_repositories() {
    let profile = crate::ledger::test_util::profile();
    let defaults = Defaults::default();
    let mut gate = LedgerEntry::new("test", &profile, "auto", "fix", "#822", None, None);
    gate.work_id = Some("#822".into());
    gate.human_required = true;
    gate.human_required_reason_code = Some("policy_approval".into());
    gate.routing_diagnostics = Some(RoutingDiagnostics {
        candidates: ["paid-a", "paid-b"]
            .into_iter()
            .map(|instance| RoutingCandidateDiagnostic {
                backend: "opencode".into(),
                backend_instance: Some(instance.into()),
                model: Some("provider/model".into()),
                skip_reason: Some("operator_approval_required".into()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    let mut entries = vec![gate];
    let requested = paid_route_approvals_from_entries(&entries, "test", &profile, &defaults);
    assert_eq!(requested.len(), 2);
    assert!(requested
        .iter()
        .all(|route| route.requested && !route.approved));
    let mut grant = LedgerEntry::new_paid_route_approval_for_instance(
        "test",
        &profile,
        "TICKET-822",
        "opencode",
        Some("paid-a"),
        Some("provider/model"),
        true,
    );
    grant.repo_id = "other-repo".into();
    entries.push(grant.clone());
    assert!(
        paid_route_approvals_from_entries(&entries, "test", &profile, &defaults)
            .iter()
            .all(|route| !route.approved)
    );
    grant.repo_id = profile.repo_id.clone();
    entries.push(grant.clone());
    let routes = paid_route_approvals_from_entries(&entries, "test", &profile, &defaults);
    assert!(routes[0].approved && !routes[0].requested);
    assert!(!routes[1].approved && routes[1].requested);
    grant.mode = "paid_route_approval_revoke".into();
    entries.push(grant.clone());
    assert!(
        paid_route_approvals_from_entries(&entries, "test", &profile, &defaults)
            .iter()
            .all(|route| route.requested && !route.approved)
    );
    grant.mode = "paid_route_approval_grant".into();
    entries.push(grant.clone());
    grant.mode = "clear_attempts".into();
    entries.push(grant);
    let remaining = paid_route_approvals_from_entries(&entries, "test", &profile, &defaults);
    assert_eq!(remaining.len(), 1);
    assert!(
        remaining[0].approved,
        "clearing a gate must not hide a revocable grant"
    );
}
