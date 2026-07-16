use crate::ledger::{AttemptRecord, FailureClass, FailureStage, LedgerEntry, LedgerUsage};

#[allow(clippy::too_many_arguments)]
pub(super) fn record_cancelled_attempt(
    ledger: &mut LedgerEntry,
    attempt_number: u32,
    backend: &str,
    model: &str,
    exit_code: i32,
    stage: FailureStage,
    duration_seconds: f64,
    usage: LedgerUsage,
    cli_version: Option<String>,
) {
    super::super::super::attempts::mark_shutdown_cancelled(ledger, stage, Some(exit_code));
    ledger.attempts.push(AttemptRecord {
        attempt_number,
        backend: backend.to_string(),
        effective_model: Some(model.to_string()),
        exit_code: Some(exit_code),
        validation_result: Some("cancelled_shutdown".into()),
        failure_class: Some(FailureClass::HarnessError.as_str().into()),
        failure_stage: Some(stage.as_str().into()),
        duration_seconds: Some(duration_seconds),
        diff_path: None,
        usage,
        cli_version,
    });
}
