use crate::ledger::{AttemptRecord, FailureClass, FailureStage, LedgerEntry, LedgerUsage};
use crate::worktree;
use anyhow::Result;
use std::fs;
use std::path::Path;

#[cfg(debug_assertions)]
pub(super) fn pause_after_backend_result_if_requested() -> Result<()> {
    if let Some(path) = std::env::var_os("GAH_TEST_PAUSE_AFTER_BACKEND_RESULT_FILE") {
        let gate = std::path::PathBuf::from(path);
        let ready = gate.with_extension("ready");
        fs::write(&ready, "ready\n")?;
        loop {
            if fs::read_to_string(&gate)
                .map(|contents| contents.trim() == "resume")
                .unwrap_or(false)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = fs::remove_file(ready);
    }
    Ok(())
}

#[cfg(not(debug_assertions))]
pub(super) fn pause_after_backend_result_if_requested() -> Result<()> {
    Ok(())
}

pub(super) struct ShutdownContext<'a> {
    worktree_path: &'a Path,
    repo: &'a Path,
    target_branch: &'a str,
    dispatch_branch: &'a str,
    attempt_session: &'a Path,
    run_id: Option<String>,
    attempt_number: u32,
    work_id: Option<String>,
}

impl<'a> ShutdownContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        worktree_path: &'a Path,
        repo: &'a Path,
        target_branch: &'a str,
        dispatch_branch: &'a str,
        attempt_session: &'a Path,
        run_id: Option<String>,
        attempt_number: u32,
        work_id: Option<String>,
    ) -> Self {
        Self {
            worktree_path,
            repo,
            target_branch,
            dispatch_branch,
            attempt_session,
            run_id,
            attempt_number,
            work_id,
        }
    }

    pub(super) fn checkpoint_after_result(
        &self,
        ledger: &mut LedgerEntry,
        shutdown_after_result: bool,
    ) -> Result<()> {
        maybe_checkpoint_after_backend_result(
            ledger,
            shutdown_after_result,
            self.worktree_path,
            self.repo,
            self.target_branch,
            self.dispatch_branch,
            self.attempt_session,
            self.run_id.as_deref(),
            self.attempt_number,
            self.work_id.as_deref(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_cancelled_attempt_and_cleanup(
        &self,
        ledger: &mut LedgerEntry,
        backend: &str,
        model: &str,
        exit_code: i32,
        stage: FailureStage,
        duration_seconds: f64,
        usage: LedgerUsage,
        cli_version: Option<String>,
    ) -> Result<()> {
        record_cancelled_attempt_and_cleanup(
            ledger,
            self.attempt_number,
            backend,
            model,
            exit_code,
            stage,
            duration_seconds,
            usage,
            cli_version,
            self.worktree_path,
            self.repo,
            self.target_branch,
            self.dispatch_branch,
            self.attempt_session,
            self.run_id.as_deref(),
            self.work_id.as_deref(),
        )
    }

    pub(super) fn record_cancelled_backend_result_and_cleanup(
        &self,
        ledger: &mut LedgerEntry,
        route: &crate::routing::RouteDecision,
        model: &str,
        result: &crate::runner::RunResult,
        claude_path: Option<&str>,
        duration_seconds: f64,
    ) -> Result<()> {
        let usage = super::super::super::attempts::attempt_usage(
            &result.log_path,
            result.agy_cli_log_delta.as_deref(),
            crate::usage_attribution::UsageAttribution::from_route(route)
                .with_fallback_model(model),
            result.transcript_path.as_deref(),
            claude_path,
        );
        self.record_cancelled_attempt_and_cleanup(
            ledger,
            &route.effective_backend,
            model,
            result.exit_code,
            FailureStage::AgentRun,
            duration_seconds,
            usage,
            result.agy_version.clone(),
        )
    }
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
        checkpoint_branch: None,
        checkpoint_sha: None,
        usage,
        cli_version,
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn checkpoint_and_cleanup_after_shutdown(
    ledger: &mut LedgerEntry,
    worktree_path: &Path,
    repo: &Path,
    target_branch: &str,
    dispatch_branch: &str,
    attempt_session: &Path,
    run_id: Option<&str>,
    attempt_number: u32,
    work_id: Option<&str>,
    reason: &str,
) -> Result<()> {
    let recovery_branch =
        super::super::super::attempts::wip_checkpoint_branch(dispatch_branch, attempt_number);
    let checkpointed = worktree::checkpoint_wip(
        worktree_path,
        target_branch,
        &recovery_branch,
        &format!(
            "gah: WIP shutdown {} attempt {}",
            dispatch_branch, attempt_number
        ),
    )?;
    let checkpoint_sha = if checkpointed {
        worktree::git(&["rev-parse", "HEAD"], worktree_path).ok()
    } else {
        None
    };

    // Issue #362: typed identity on the ledger entry (AC2), in addition to
    // the on-disk recovery artifact below that find_latest_resumable_checkpoint
    // actually reads for the resume decision -- this is for ledger
    // queryability, not the resume mechanism itself.
    super::super::super::checkpoints::record_checkpoint_in_ledger(
        ledger,
        &recovery_branch,
        checkpoint_sha.clone(),
        checkpointed,
    );
    if checkpointed {
        super::super::super::checkpoints::mark_attempt_as_resumable(
            ledger,
            attempt_number,
            &recovery_branch,
            checkpoint_sha.clone(),
        );
    }

    // Issue #362: Enhanced recovery artifact with checkpoint SHA for resumption
    let recovery_artifact = serde_json::json!({
        "run_id": run_id.unwrap_or("unknown"),
        "attempt_number": attempt_number,
        "source_work_id": work_id.unwrap_or("unknown"),
        "dispatch_branch": dispatch_branch,
        "recovery_branch": recovery_branch.clone(),
        "checkpointed": checkpointed,
        "checkpoint_sha": checkpoint_sha,
        "is_resumable": checkpointed,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    });
    fs::write(
        attempt_session.join("shutdown-recovery.json"),
        serde_json::to_vec_pretty(&recovery_artifact)?,
    )?;
    worktree::cleanup(worktree_path, repo);
    anyhow::bail!("{reason}");
}

#[allow(clippy::too_many_arguments)]
pub(super) fn maybe_checkpoint_after_backend_result(
    ledger: &mut LedgerEntry,
    shutdown_after_result: bool,
    worktree_path: &Path,
    repo: &Path,
    target_branch: &str,
    dispatch_branch: &str,
    attempt_session: &Path,
    run_id: Option<&str>,
    attempt_number: u32,
    work_id: Option<&str>,
) -> Result<()> {
    if shutdown_after_result {
        checkpoint_and_cleanup_after_shutdown(
            ledger,
            worktree_path,
            repo,
            target_branch,
            dispatch_branch,
            attempt_session,
            run_id,
            attempt_number,
            work_id,
            "shutdown requested after backend completed; not retrying",
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_cancelled_attempt_and_cleanup(
    ledger: &mut LedgerEntry,
    attempt_number: u32,
    backend: &str,
    model: &str,
    exit_code: i32,
    stage: FailureStage,
    duration_seconds: f64,
    usage: LedgerUsage,
    cli_version: Option<String>,
    worktree_path: &Path,
    repo: &Path,
    target_branch: &str,
    dispatch_branch: &str,
    attempt_session: &Path,
    run_id: Option<&str>,
    work_id: Option<&str>,
) -> Result<()> {
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
        checkpoint_branch: None,
        checkpoint_sha: None,
        usage,
        cli_version,
    });
    checkpoint_and_cleanup_after_shutdown(
        ledger,
        worktree_path,
        repo,
        target_branch,
        dispatch_branch,
        attempt_session,
        run_id,
        attempt_number,
        work_id,
        &format!("shutdown requested while {backend} was running"),
    )
}
