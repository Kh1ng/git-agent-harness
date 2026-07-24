use crate::config::{GahConfig, Profile};
use crate::dispatch::attempts::classify_git_operation_result;
use crate::dispatch::attempts::clear_wip_checkpoints;
use crate::dispatch::issues::{IssueDetails, TicketMetadata};
use crate::dispatch::metrics::apply_diff_stats;
use crate::dispatch::publish::{
    build_fix_or_improve_mr_body, build_mr_title, emit_human_handoff,
    enforce_generated_artifact_policy, ensure_issue_open_for_publish, publishing_allows_publish,
    MrRenderContext,
};
use crate::dispatch::workflows::already_satisfied_reconcile::AlreadySatisfiedRun;
use crate::dispatch::workflows::improve::conflict_resolution::ConflictSession;
use crate::dispatch::workflows::improve::publish_mr::publish_or_update_mr;
use crate::ledger::LedgerEntry;
use crate::worktree;
use anyhow::Result;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_improve_workflow(
    cfg: &GahConfig,
    profile: &Profile,
    mode: &str,
    ledger: &mut LedgerEntry,
    wt: &Path,
    repo: &Path,
    branch: &str,
    existing_branch: bool,
    ticket_meta: Option<&TicketMetadata>,
    issue_details: Option<&IssueDetails>,
    conflict_session: &ConflictSession,
    already_satisfied: &AlreadySatisfiedRun,
    validation_failed: bool,
    backend_summary: &str,
    wip_checkpoints: &[String],
    route_effective_backend: &str,
    route_effective_model: Option<&str>,
    llm_model: &str,
) -> Result<()> {
    if profile.validation_commands.is_empty() && ledger.validation_result.is_none() {
        ledger.validation_result = Some("not_run".into());
    }

    // Retries cold-start a backend with bounded failure context; validation
    // commands run sequentially and feed bounded output into the next attempt.
    let has_changes = classify_git_operation_result(
        ledger,
        crate::ledger::FailureStage::PostValidation,
        worktree::has_changes(wt, &profile.default_target_branch),
    )?;
    // Reconcile a structured, grounded no-diff completion before the generic
    // no-progress backstop. This is the primary already-satisfied path.
    if !has_changes
        && already_satisfied.reconcile(
            ledger,
            backend_summary,
            wip_checkpoints,
            !validation_failed,
        )?
    {
        return Ok(());
    }
    already_satisfied.enforce_post_validation_changes(ledger, has_changes)?;

    // Reject an explicitly claimed already-satisfied completion that consists
    // only of a coverage-weakening test diff before publishing.
    if already_satisfied.reconcile(ledger, backend_summary, wip_checkpoints, !validation_failed)? {
        return Ok(());
    }

    let commit_title = if validation_failed {
        format!(
            "gah: {} changes for {} [validation-failing draft]",
            mode, profile.repo_id
        )
    } else {
        format!("gah: {} changes for {}", mode, profile.repo_id)
    };
    let mut commit_msg = commit_title;
    if !backend_summary.is_empty() {
        commit_msg.push_str("\n\n");
        commit_msg.push_str(backend_summary);
    }

    enforce_generated_artifact_policy(profile, ledger, wt)?;

    if super::handoff::maybe_perform_handoff(
        cfg,
        profile,
        ledger,
        wt,
        branch,
        &commit_msg,
        ticket_meta,
        backend_summary,
        repo,
        wip_checkpoints,
    )? {
        return Ok(());
    }

    // TICKET-128: honor the per-profile publishing policy. A restricted profile
    // forbids PR/MR creation and/or LLM-generated commit messages, so we stop
    // at a deterministic human handoff after code generation + validation
    // instead of publishing the work. This is independent of reviewer routing
    // and merge policy: review still runs, the worktree is still cleaned up,
    // only the autonomous publish step is suppressed.
    if !publishing_allows_publish(profile) {
        // Commit only if the policy still permits agent-authored commit text.
        if profile.publishing.allow_commit_message_generation {
            if worktree::has_uncommitted_changes(wt)? {
                ledger.commit_attempted = true;
                worktree::stage_all(wt)?;
                worktree::ensure_staged(wt)?;
                worktree::commit_msg(wt, &commit_msg)?;
                ledger.commit_created = true;
            } else {
                ledger.commit_created = true;
            }
        }
        apply_diff_stats(ledger, wt, &profile.default_target_branch);
        emit_human_handoff(
            profile,
            ledger,
            branch,
            "PR/MR creation or commit-message generation disabled by publishing policy",
        );
        clear_wip_checkpoints(repo, wip_checkpoints);
        worktree::preserve_wip(
            wt,
            &profile.default_target_branch,
            &format!("gah: WIP handoff {}", mode),
        )?;
        worktree::cleanup(wt, repo);
        return Ok(());
    }

    if let Some(issue) = issue_details.as_ref() {
        if let Err(error) = ensure_issue_open_for_publish(profile, issue) {
            ledger.set_failure(
                crate::ledger::FailureClass::HumanBlocked,
                crate::ledger::FailureStage::Push,
            );
            worktree::preserve_wip(
                wt,
                &profile.default_target_branch,
                &format!("gah: WIP blocked {}", mode),
            )?;
            worktree::cleanup(wt, repo);
            return Err(error);
        }
    }

    println!("Changes detected. Committing and pushing...");
    let push_url = profile.push_url()?;
    let push_pat = profile.pat();
    if worktree::has_uncommitted_changes(wt)? {
        ledger.commit_attempted = true;
        worktree::stage_all(wt)?;
        worktree::ensure_staged(wt)?;
        worktree::commit_msg(wt, &commit_msg)?;
        ledger.commit_created = true;
    } else {
        // Backend committed its own work already (e.g. vibe) -- nothing left
        // to stage, just push what's already on HEAD.
        ledger.commit_created = true;
    }
    conflict_session.verify_before_publish(ledger)?;
    // Must run after the commit above -- diff_stats/changed_files compare
    // origin/<target> against HEAD, so computing them beforehand (while the
    // real changes are still uncommitted working-tree modifications) always
    // reported "0 file(s) changed, +0, -0" in the MR body.
    apply_diff_stats(ledger, wt, &profile.default_target_branch);
    ledger.push_attempted = true;
    classify_git_operation_result(
        ledger,
        crate::ledger::FailureStage::Push,
        worktree::push_branch(wt, branch, &push_url, &push_pat),
    )?;
    ledger.push_succeeded = true;

    let mr_title = build_mr_title(mode, &profile.repo_id, validation_failed, ticket_meta);
    let mr_ctx = MrRenderContext {
        backend: route_effective_backend,
        model: llm_model,
        branch,
        target_branch: &profile.default_target_branch,
        validation_commands: &profile.validation_commands,
        ledger,
        backend_summary,
    };
    let mr_body = build_fix_or_improve_mr_body(mode, ticket_meta, &mr_ctx, !validation_failed);
    publish_or_update_mr(
        cfg,
        profile,
        ledger,
        branch,
        &mr_title,
        &mr_body,
        existing_branch,
        route_effective_backend,
        route_effective_model,
    )?;

    clear_wip_checkpoints(repo, wip_checkpoints);
    worktree::cleanup(wt, repo);
    Ok(())
}
