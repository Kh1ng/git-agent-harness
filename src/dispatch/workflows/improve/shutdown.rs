use crate::ledger::{AttemptRecord, FailureClass, FailureStage, LedgerEntry, LedgerUsage};
use crate::usage_attribution::UsageAttribution;
use anyhow::Result;
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

fn sanitize_ref_component(value: &str) -> String {
    let mut sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    while sanitized.contains("--") {
        sanitized = sanitized.replace("--", "-");
    }
    let trimmed = sanitized.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed
    }
}

pub(super) fn shutdown_recovery_branch(
    dispatch_branch: &str,
    run_id: &str,
    source_work_id: &str,
    attempt_number: u32,
) -> String {
    format!(
        "gah-recovery/{}/run-{}/work-{}/attempt-{attempt_number}",
        sanitize_ref_component(dispatch_branch),
        sanitize_ref_component(run_id),
        sanitize_ref_component(source_work_id),
    )
}

pub(super) fn pause_after_backend_result_for_shutdown_race() {
    let Some(delay_ms) = std::env::var("GAH_INTERNAL_PAUSE_AFTER_BACKEND_RESULT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|delay_ms| *delay_ms > 0)
    else {
        return;
    };
    thread::sleep(Duration::from_millis(delay_ms));
}

#[derive(Serialize)]
struct ShutdownRecoveryArtifact<'a> {
    run_id: &'a str,
    attempt_number: u32,
    source_work_id: &'a str,
    recovery_branch: &'a str,
    checkpointed: bool,
    dirty_worktree: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn checkpoint_shutdown_wip(
    worktree: &Path,
    repo: &Path,
    target_branch: &str,
    attempt_session: &Path,
    recovery_branch: &str,
    message: &str,
    run_id: &str,
    source_work_id: &str,
    attempt_number: u32,
) -> Result<()> {
    let dirty_worktree = crate::worktree::has_changes(worktree, target_branch)?;
    let checkpointed = if dirty_worktree {
        crate::worktree::checkpoint_wip(worktree, target_branch, recovery_branch, message)?
    } else {
        false
    };
    let artifact = ShutdownRecoveryArtifact {
        run_id,
        attempt_number,
        source_work_id,
        recovery_branch,
        checkpointed,
        dirty_worktree,
    };
    fs::write(
        attempt_session.join("shutdown-recovery.json"),
        serde_json::to_string_pretty(&artifact)?,
    )?;
    crate::worktree::cleanup(worktree, repo);
    if dirty_worktree && !checkpointed {
        eprintln!(
            "warning: shutdown recovery branch {recovery_branch} could not be checkpointed; cleaned up worktree after preserving the best available artifacts"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn checkpoint_shutdown_wip_if_requested(
    shutdown_requested_after_result: bool,
    worktree: &Path,
    repo: &Path,
    target_branch: &str,
    attempt_session: &Path,
    dispatch_branch: &str,
    run_id: &str,
    source_work_id: &str,
    attempt_number: u32,
    mode: &str,
    bail_message: &str,
) -> Result<()> {
    if shutdown_requested_after_result || crate::runner::shutdown_requested() {
        let recovery_branch =
            shutdown_recovery_branch(dispatch_branch, run_id, source_work_id, attempt_number);
        let message = format!(
            "gah: WIP shutdown {mode} attempt {attempt_number} run {run_id} work {source_work_id}"
        );
        checkpoint_shutdown_wip(
            worktree,
            repo,
            target_branch,
            attempt_session,
            &recovery_branch,
            &message,
            run_id,
            source_work_id,
            attempt_number,
        )?;
        anyhow::bail!("{bail_message}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn maybe_finalize_after_backend_result(
    shutdown_requested_after_result: bool,
    ledger: &mut LedgerEntry,
    backend: &str,
    model: &str,
    usage_attribution: UsageAttribution<'_>,
    result_log_path: &str,
    result_agy_cli_log_delta: Option<&str>,
    result_transcript_path: Option<&str>,
    result_exit_code: i32,
    result_agy_version: Option<String>,
    attempt_start: &std::time::Instant,
    worktree: &Path,
    repo: &Path,
    target_branch: &str,
    attempt_session: &Path,
    dispatch_branch: &str,
    run_id: &str,
    source_work_id: &str,
    attempt_number: u32,
    mode: &str,
    claude_path: &str,
    failure_stage: FailureStage,
    record_cancelled: bool,
    bail_message: &str,
) -> Result<()> {
    if shutdown_requested_after_result || crate::runner::shutdown_requested() {
        if record_cancelled {
            record_cancelled_attempt(
                ledger,
                attempt_number,
                backend,
                model,
                result_exit_code,
                failure_stage,
                attempt_start.elapsed().as_secs_f64(),
                crate::dispatch::attempts::attempt_usage(
                    result_log_path,
                    result_agy_cli_log_delta,
                    usage_attribution,
                    result_transcript_path,
                    Some(claude_path),
                ),
                result_agy_version,
            );
        }
        checkpoint_shutdown_wip_if_requested(
            shutdown_requested_after_result,
            worktree,
            repo,
            target_branch,
            attempt_session,
            dispatch_branch,
            run_id,
            source_work_id,
            attempt_number,
            mode,
            bail_message,
        )?;
        anyhow::bail!("{bail_message}");
    }
    Ok(())
}

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
