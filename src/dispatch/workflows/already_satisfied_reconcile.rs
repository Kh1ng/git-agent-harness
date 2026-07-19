//! Issue #584: reconcile an already-satisfied backend disposition before a
//! completion MR is published. Extracted from the improve workflow so the
//! publishing path stays under the source-size ratchet.

use super::super::already_satisfied::{build_diff_summary, emit_already_satisfied_handoff};
use super::super::publish::{
    emit_human_handoff, reconcile_before_publish, AlreadySatisfiedPublishOutcome,
};
use crate::config::Profile;
use crate::ledger::{FailureClass, FailureStage, LedgerEntry};
use crate::worktree;
use anyhow::{bail, Result};
use std::path::Path;

pub(super) struct AlreadySatisfiedRun<'a> {
    profile: &'a Profile,
    worktree: &'a Path,
    repo: &'a Path,
    branch: &'a str,
    mode: &'a str,
    issue_number: Option<&'a str>,
}

impl<'a> AlreadySatisfiedRun<'a> {
    pub fn new(
        profile: &'a Profile,
        worktree: &'a Path,
        repo: &'a Path,
        branch: &'a str,
        mode: &'a str,
        issue_number: Option<&'a str>,
    ) -> Self {
        Self {
            profile,
            worktree,
            repo,
            branch,
            mode,
            issue_number,
        }
    }

    pub fn reconcile(
        &self,
        ledger: &mut LedgerEntry,
        backend_summary: &str,
        wip_checkpoints: &[String],
        validation_clean: bool,
    ) -> Result<bool> {
        reconcile_already_satisfied_publish(AlreadySatisfiedContext {
            profile: self.profile,
            ledger,
            worktree: self.worktree,
            repo: self.repo,
            branch: self.branch,
            mode: self.mode,
            issue_number: self.issue_number,
            backend_summary,
            wip_checkpoints,
            validation_clean,
        })
    }

    pub fn enforce_post_validation_changes(
        &self,
        ledger: &mut LedgerEntry,
        has_changes: bool,
    ) -> Result<()> {
        if has_changes {
            return Ok(());
        }
        ledger.validation_result = Some("passed_no_changes".into());
        ledger.set_failure(FailureClass::AgentNoProgress, FailureStage::AgentRun);
        if let Some(last_attempt) = ledger.attempts.last_mut() {
            last_attempt.validation_result = Some("passed_no_changes".into());
            last_attempt.failure_class = Some(FailureClass::AgentNoProgress.as_str().into());
            last_attempt.failure_stage = Some(FailureStage::AgentRun.as_str().into());
        }
        worktree::cleanup(self.worktree, self.repo);
        bail!("all worktree changes disappeared before publish")
    }
}

struct AlreadySatisfiedContext<'a> {
    profile: &'a Profile,
    ledger: &'a mut LedgerEntry,
    worktree: &'a Path,
    repo: &'a Path,
    branch: &'a str,
    mode: &'a str,
    issue_number: Option<&'a str>,
    backend_summary: &'a str,
    wip_checkpoints: &'a [String],
    validation_clean: bool,
}

struct BoundedHandoffContext<'a> {
    profile: &'a Profile,
    ledger: &'a mut LedgerEntry,
    worktree: &'a Path,
    repo: &'a Path,
    branch: &'a str,
    mode: &'a str,
    wip_checkpoints: &'a [String],
    has_changes: bool,
}

/// Returns `Ok(true)` when the disposition was reconciled and the improve
/// workflow should return without publishing a completion MR (either an
/// idempotent close for a trusted autonomous provider issue, or a bounded
/// operator handoff). Returns `Ok(false)` to proceed with normal publication.
fn reconcile_already_satisfied_publish(context: AlreadySatisfiedContext<'_>) -> Result<bool> {
    let AlreadySatisfiedContext {
        profile,
        ledger,
        worktree,
        repo,
        branch,
        mode,
        issue_number,
        backend_summary,
        wip_checkpoints,
        validation_clean,
    } = context;
    let diff_summary = build_diff_summary(worktree, &profile.default_target_branch);
    let has_changes = !diff_summary.changed_files.is_empty();
    match reconcile_before_publish(
        profile,
        backend_summary,
        &diff_summary,
        worktree,
        validation_clean,
    ) {
        AlreadySatisfiedPublishOutcome::Proceed => Ok(false),
        AlreadySatisfiedPublishOutcome::CloseIdempotently(evidence) => {
            if let Err(error) = super::super::mutation_policy::enforce_policy(profile, "edit-issue")
            {
                let reason = format!(
                    "already_satisfied was grounded, but source issue mutation policy denied closure: {error:#}"
                );
                return bounded_handoff(
                    BoundedHandoffContext {
                        profile,
                        ledger,
                        worktree,
                        repo,
                        branch,
                        mode,
                        wip_checkpoints,
                        has_changes,
                    },
                    &reason,
                );
            }
            let Some(issue_number) = issue_number else {
                return bounded_handoff(
                    BoundedHandoffContext {
                        profile,
                        ledger,
                        worktree,
                        repo,
                        branch,
                        mode,
                        wip_checkpoints,
                        has_changes,
                    },
                    "already_satisfied was grounded but no provider issue identity was available",
                );
            };
            let body = render_reconciliation_comment(&evidence);
            crate::provider::post_issue_comment(profile, issue_number, &body)?;
            close_source_issue_if_open(profile, issue_number)?;
            record_already_satisfied(ledger);
            emit_already_satisfied_handoff(profile, ledger, branch, &evidence);
            super::super::attempts::clear_wip_checkpoints(repo, wip_checkpoints);
            worktree::cleanup(worktree, repo);
            Ok(true)
        }
        AlreadySatisfiedPublishOutcome::BoundedHandoff(reason) => bounded_handoff(
            BoundedHandoffContext {
                profile,
                ledger,
                worktree,
                repo,
                branch,
                mode,
                wip_checkpoints,
                has_changes,
            },
            &reason,
        ),
    }
}

fn bounded_handoff(context: BoundedHandoffContext<'_>, reason: &str) -> Result<bool> {
    let BoundedHandoffContext {
        profile,
        ledger,
        worktree,
        repo,
        branch,
        mode,
        wip_checkpoints,
        has_changes,
    } = context;
    record_already_satisfied(ledger);
    ledger.human_required = true;
    ledger.human_required_reason_code = Some(
        crate::controller::HumanRequiredReason::PublishingRestriction
            .as_str()
            .into(),
    );
    emit_human_handoff(profile, ledger, branch, reason);
    super::super::attempts::clear_wip_checkpoints(repo, wip_checkpoints);
    if has_changes {
        worktree::preserve_wip(
            worktree,
            &profile.default_target_branch,
            &format!("gah: already-satisfied handoff {mode}"),
        )?;
    }
    worktree::cleanup(worktree, repo);
    Ok(true)
}

fn render_reconciliation_comment(
    evidence: &super::super::already_satisfied::AlreadySatisfiedEvidence,
) -> String {
    let mut body = String::from(
        "GAH verified that the current target branch already satisfies this work item; no completion MR was created.\n\nEvidence:\n",
    );
    for file in &evidence.grounded_files {
        body.push_str(&format!("- file:{file}\n"));
    }
    for test in &evidence.grounded_tests {
        body.push_str(&format!("- test:{test}\n"));
    }
    body
}

fn close_source_issue_if_open(profile: &Profile, issue_number: &str) -> Result<()> {
    let state = match profile.provider.as_str() {
        "github" => crate::provider::github_get_issue_state(profile, issue_number)?,
        "gitlab" => crate::provider::gitlab_get_issue_state(profile, issue_number)?,
        other => bail!("unsupported provider for source issue closure: {other}"),
    };
    if state.as_deref().is_some_and(|state| {
        state.eq_ignore_ascii_case("closed") || state.eq_ignore_ascii_case("merged")
    }) {
        return Ok(());
    }
    if state.is_none() {
        bail!("provider did not return source issue #{issue_number} state");
    }
    match profile.provider.as_str() {
        "github" => crate::provider::github_close_issue(profile, issue_number),
        "gitlab" => crate::provider::gitlab_close_issue(profile, issue_number),
        _ => unreachable!("provider was validated above"),
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
