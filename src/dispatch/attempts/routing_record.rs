//! Durable evidence for the route selected for each dispatch attempt.
use super::{CandidateIdentity, LedgerEntry, Result, RouteDecision};

pub(in crate::dispatch) fn record_route_attempt(
    ledger: &mut LedgerEntry,
    route: &RouteDecision,
) -> Result<()> {
    route.identity.validate_for_persistence()?;
    ledger
        .routing_runtime
        .dispatch_attempted
        .insert(CandidateIdentity::from_execution_identity(&route.identity));
    let mut diagnostics = route.routing_diagnostics.clone().unwrap_or_default();
    diagnostics.selected_subscription_capacity = Some(crate::routing::subscription::capacity(
        &route.identity,
        &crate::quota_store::load_account_observations(),
        time::OffsetDateTime::now_utc(),
    ));
    ledger
        .attempt_routing
        .push(crate::ledger::AttemptRoutingRecord {
            attempt_number: ledger.attempt_routing.len() as u32 + 1,
            backend_instance: route.identity.backend_instance.clone(),
            effective_model: route.effective_model.clone(),
            identity: Some(route.identity.clone()),
            routing_diagnostics: Some(diagnostics),
        });
    Ok(())
}
