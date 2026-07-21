use crate::routing::RouteDecision;
use anyhow::Result;

/// Persist an outage against the exact route that stalled, including its
/// model and quota-pool identity, so failover does not immediately select it.
pub(super) fn record_exact_route_unavailability(
    route: &RouteDecision,
    log_text: &str,
    log_path: &str,
) -> Result<Option<crate::quota_parser::ParsedFailure>> {
    super::super::super::attempts::mark_backend_unavailable_from_output_for_identity(
        &route.identity,
        log_text,
        log_path,
    )
}
