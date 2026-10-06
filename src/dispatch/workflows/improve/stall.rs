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

/// Remove the worktree of a run parked for an invalid model, committing any
/// partial work first so the human who fixes the config does not lose it.
pub(super) fn discard_preserving_wip(
    wt: &std::path::Path,
    profile: &crate::config::Profile,
    mode: impl std::fmt::Display,
    attempt_number: u32,
) -> Result<()> {
    crate::worktree::preserve_wip(
        wt,
        &profile.default_target_branch,
        &format!("gah: WIP invalid-model {mode} attempt {attempt_number}"),
    )?;
    crate::worktree::cleanup(wt, std::path::Path::new(&profile.local_path));
    Ok(())
}

/// Classification of a backend that launched but exited nonzero, derived
/// from its terminal log (extracted from `improve` to keep that function
/// focused on orchestration).
pub(super) struct BackendExitFailure {
    pub stalled: bool,
    pub stalled_before_changes: bool,
    pub stalled_during_validation: bool,
    pub cleanup_failed: bool,
    /// Issue #1367: the backend CLI refused the run's writes. Holds the
    /// operator-facing detail naming the setting to fix; retrying is futile.
    pub config_error: Option<String>,
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
    let config_error = (!cleanup_failed)
        .then(|| crate::runner::backends::write_refusal::refusal_detail(log_text))
        .flatten()
        .map(str::to_string);
    let failure_class = if cleanup_failed {
        crate::ledger::FailureClass::HarnessError
    } else if config_error.is_some() {
        crate::ledger::FailureClass::EnvironmentError
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
        config_error,
        failure_class,
    }
}

/// Issue #1367: end a dispatch whose backend CLI refused its writes. The same
/// configuration refuses every retry, so the remaining attempts are not
/// spent and the error tells the operator which setting to fix. `who` is the
/// "<backend> <mode> attempt <n>" label of the refused attempt.
pub(super) fn stop_on_refused_writes(
    ledger: &mut crate::ledger::LedgerEntry,
    worktree: &std::path::Path,
    repo: &std::path::Path,
    profile: &crate::config::Profile,
    who: &str,
    detail: &str,
) -> Result<()> {
    ledger.error_summary = Some(format!("backend writes refused: {detail}"));
    crate::worktree::preserve_wip(
        worktree,
        &profile.default_target_branch,
        &format!("gah: WIP failed {who}"),
    )?;
    crate::worktree::cleanup(worktree, repo);
    anyhow::bail!("{who}: writes were refused (configuration error); not retrying: {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::FailureClass;
    use crate::runner::backends::write_refusal::WRITE_REFUSED_MARKER;

    #[test]
    fn refused_writes_are_a_non_retryable_environment_error_naming_the_setting() {
        let log = format!(
            "backend output\n{WRITE_REFUSED_MARKER}codex could not write. fix profile codex_args.\n"
        );
        let failure = classify_backend_exit_failure(&log, 1);
        assert_eq!(failure.failure_class, FailureClass::EnvironmentError);
        assert_eq!(
            failure.config_error.as_deref(),
            Some("codex could not write. fix profile codex_args.")
        );
    }

    #[test]
    fn backend_output_that_only_mentions_a_configuration_error_stays_retryable() {
        let log = format!(
            "error: This is a configuration error\n{{\"text\":\"{WRITE_REFUSED_MARKER}quoted\"}}\n"
        );
        let failure = classify_backend_exit_failure(&log, 1);
        assert_eq!(failure.failure_class, FailureClass::BackendError);
        assert!(failure.config_error.is_none());
    }

    #[test]
    fn cleanup_failure_outranks_refused_writes() {
        let log = format!(
            "GAH: harness process cleanup failed: pid 1\n{WRITE_REFUSED_MARKER}fix codex_args\n"
        );
        let failure = classify_backend_exit_failure(&log, 1);
        assert!(failure.cleanup_failed);
        assert_eq!(failure.failure_class, FailureClass::HarnessError);
        assert!(failure.config_error.is_none());
    }
}
