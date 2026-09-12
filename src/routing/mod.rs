//! Stable routing facade.

mod decision;
mod diagnostics;
mod policy;
mod reservation;
mod reviewer_history;
#[cfg(test)]
mod test_support;
mod types;

pub use decision::{decide_for_task_with_state, decide_with_state};
pub use reservation::ConcurrencyGuard;
pub(crate) use reviewer_history::reviewer_outcome_metrics;
#[allow(unused_imports)]
pub use types::SkippedBackend;
pub use types::{
    CandidateIdentity, ReviewerOutcomeMetrics, RouteDecision, RouteError, RouteRequest,
    RoutingRuntimeState, TaskRoutingContext,
};

/// Preserve the stable facade path while reservation ownership lives in its
/// permanent module.
pub fn current_concurrent(backend: &str, model: Option<&str>) -> u32 {
    reservation::current_concurrent(backend, model)
}
