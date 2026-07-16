//! Issue #584: reconcile an already-satisfied backend disposition before a
//! completion MR is published. Extracted from the improve workflow so the
//! publishing path stays under the source-size ratchet.

use super::super::already_satisfied::{build_diff_summary, emit_already_satisfied_handoff};
use super::super::publish::{
    emit_human_handoff, reconcile_before_publish, AlreadySatisfiedPublishOutcome,
};
use super::super::DispatchArgs;
use crate::config::Profile;
use crate::ledger::{FailureClass, FailureStage, LedgerEntry};
use crate::worktree;
use anyhow::Result;
use std::path::Path;

/// Returns `Ok(true)` when the disposition was reconciled and the improve
/// workflow should return without publishing a completion MR (either an
/// idempotent close for a trusted autonomous provider issue, or a bounded
/// operator handoff). Returns `Ok(false)` to proceed with normal publication.
#[allow(clippy::too_many_arguments)]
pub(super) fn reconcile_already_satisfied_publish(
    profile: &Profile,
    ledger: &mut LedgerEntry,
    wt: &Path,
    repo: &Path,
    branch: &str,
    args: &DispatchArgs,
    backend_summary: &str,
    wip_checkpoints: &[String],
) -> Result<bool> {
    let diff_summary = build_diff_summary(wt, &profile.default_target_branch);
    match reconcile_before_publish(profile, backend_summary, &diff_summary) {
        AlreadySatisfiedPublishOutcome::Proceed => Ok(false),
        AlreadySatisfiedPublishOutcome::CloseIdempotently(evidence) => {
            // Trusted autonomous provider issue: the requirement is already met
            // with grounded evidence and no real change. Close idempotently
            // instead of publishing a regressive completion MR.
            record_already_satisfied(ledger);
            emit_already_satisfied_handoff(profile, ledger, branch, &evidence);
            super::super::attempts::clear_wip_checkpoints(repo, wip_checkpoints);
            worktree::preserve_wip(
                wt,
                &profile.default_target_branch,
                &format!("gah: already-satisfied {}", args.mode),
            )?;
            worktree::cleanup(wt, repo);
            Ok(true)
        }
        AlreadySatisfiedPublishOutcome::BoundedHandoff(reason) => {
            // Not safe to autonomously close (untrusted provider, or only a
            // test-only regression diff exists). Emit a bounded operator
            // handoff rather than publishing a regressive completion MR.
            record_already_satisfied(ledger);
            emit_human_handoff(profile, ledger, branch, &reason);
            super::super::attempts::clear_wip_checkpoints(repo, wip_checkpoints);
            worktree::preserve_wip(
                wt,
                &profile.default_target_branch,
                &format!("gah: already-satisfied handoff {}", args.mode),
            )?;
            worktree::cleanup(wt, repo);
            Ok(true)
        }
    }
}

fn record_already_satisfied(ledger: &mut LedgerEntry) {
    ledger.set_failure(FailureClass::AlreadySatisfied, FailureStage::PostValidation);
    ledger.validation_result = Some("already_satisfied".into());
    if let Some(last_attempt) = ledger.attempts.last_mut() {
        last_attempt.validation_result = Some("already_satisfied".into());
        last_attempt.failure_class = Some(FailureClass::AlreadySatisfied.as_str().into());
        last_attempt.failure_stage = Some(FailureStage::PostValidation.as_str().into());
    }
}
