// Shared tail of the three validation-failure branches in `improve`'s retry
// loop: they build an otherwise-identical `AttemptRecord`, differing only in
// validation_result/failure_class/failure_stage, then record external-approval
// consumption and checkpoint. Extracted so the file stays under the
// source_structure line ceiling and the shape stays enforced in one place.

use super::super::super::attempts::{
    attempt_usage, record_external_approval_consumption_for_last_attempt,
};
use super::shutdown::ShutdownContext;
use crate::config::{GahConfig, Profile};
use crate::ledger::{AttemptRecord, LedgerEntry};
use crate::routing::RouteDecision;
use crate::runner::{self, RunResult};
use crate::usage_attribution::UsageAttribution;
use anyhow::Result;

#[allow(clippy::too_many_arguments)]
pub(super) fn record_failed_validation_attempt(
    ledger: &mut LedgerEntry,
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    shutdown_ctx: &ShutdownContext,
    shutdown_after_result: bool,
    attempt: u32,
    route: &RouteDecision,
    llm: &runner::LlmConfig,
    result: &RunResult,
    claude_path: &str,
    attempt_start: std::time::Instant,
    validation_result: &str,
    failure_class: Option<String>,
    failure_stage: Option<String>,
) -> Result<()> {
    // Issue #915: capture the outcome before `failure_class`/`failure_stage`
    // move into the AttemptRecord below. Best-effort -- capture failures are
    // swallowed inside memory_gateway and never affect dispatch.
    let capture_summary = format!(
        "Attempt {} failed validation: {} (class={}, stage={})",
        attempt + 1,
        validation_result,
        failure_class.as_deref().unwrap_or("unknown"),
        failure_stage.as_deref().unwrap_or("unknown"),
    );
    ledger.attempts.push(AttemptRecord {
        attempt_number: attempt + 1,
        backend: route.effective_backend.clone(),
        effective_model: Some(llm.model.clone()),
        exit_code: Some(0),
        validation_result: Some(validation_result.into()),
        failure_class,
        failure_stage,
        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
        diff_path: None,
        checkpoint_branch: None,
        checkpoint_sha: None,
        usage: attempt_usage(
            &result.log_path,
            result.agy_cli_log_delta.as_deref(),
            UsageAttribution::from_route(route).with_fallback_model(&llm.model),
            result.transcript_path.as_deref(),
            Some(claude_path),
        ),
        cli_version: result.agy_version.clone(),
    });
    if let Some(work_id) = ledger.work_id.clone() {
        crate::memory_gateway::capture_attempt_and_update_ledger(
            profile_name,
            &profile.local_path,
            &work_id,
            &format!("Dispatch attempt {} for {}", attempt + 1, work_id),
            &capture_summary,
            ledger,
        );
    }
    record_external_approval_consumption_for_last_attempt(cfg, profile_name, profile, ledger);
    shutdown_ctx.checkpoint_after_result(ledger, shutdown_after_result)
}
