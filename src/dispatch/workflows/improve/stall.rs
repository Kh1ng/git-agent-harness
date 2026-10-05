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

/// A runner that rejects the selected model is a configuration fault: park
/// the work item for a human instead of retrying or rerouting.
pub(super) fn invalid_model(
    ledger: &mut crate::ledger::LedgerEntry,
    log_text: &str,
    route: &RouteDecision,
    exit_failure: &mut BackendExitFailure,
) -> Option<String> {
    let label = super::super::super::attempts::route_label(
        &route.effective_backend,
        route.effective_model.as_deref(),
    );
    let message = crate::model_validation::invalid_model_message(log_text, &label)?;
    exit_failure.failure_class = crate::ledger::FailureClass::ConfigError;
    ledger.error_summary = Some(message.clone());
    ledger.human_required = true;
    ledger.human_required_reason_code = Some(
        crate::controller::HumanRequiredReason::ConfigurationInfra
            .as_str()
            .into(),
    );
    Some(message)
}

/// Classification of a backend that launched but exited nonzero, derived
/// from its terminal log (extracted from `improve` to keep that function
/// focused on orchestration).
pub(super) struct BackendExitFailure {
    pub stalled: bool,
    pub stalled_before_changes: bool,
    pub stalled_during_validation: bool,
    pub cleanup_failed: bool,
    pub failure_class: crate::ledger::FailureClass,
}

pub(super) fn classify_backend_exit_failure(log_text: &str, exit_code: i32) -> BackendExitFailure {
    let stalled = log_text.contains("GAH: killed after ")
        && log_text.contains("(stalled")
        && log_text.contains("not just slow).");
    let stalled_before_changes = stalled && log_text.contains("stalled before changes");
    let stalled_during_validation =
        stalled && log_text.contains("stalled during validation with checkpointed changes");
    let cleanup_failed = exit_code == crate::runner::process::PROCESS_CLEANUP_FAILED_EXIT_CODE
        || log_text.contains("GAH: harness process cleanup failed:");
    let failure_class = if cleanup_failed {
        crate::ledger::FailureClass::HarnessError
    } else if stalled_before_changes {
        crate::ledger::FailureClass::AgentNoProgress
    } else if stalled {
        crate::ledger::FailureClass::HarnessError
    } else {
        crate::ledger::FailureClass::BackendError
    };
    BackendExitFailure {
        stalled,
        stalled_before_changes,
        stalled_during_validation,
        cleanup_failed,
        failure_class,
    }
}
