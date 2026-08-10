use super::super::super::repair_context::{self, RepairContext};
use crate::config::GahConfig;
use crate::dispatch::DispatchArgs;
use crate::ledger::{FailureClass, FailureStage, LedgerEntry};
use crate::worktree;
use anyhow::Result;
use std::path::Path;

pub(super) struct LoadContext<'a> {
    pub enabled: bool,
    pub cfg: &'a GahConfig,
    pub profile: &'a crate::config::Profile,
    pub profile_name: &'a str,
    pub repo_id: &'a str,
    pub branch: &'a str,
    pub expected_review_generation: Option<&'a str>,
    pub worktree_path: &'a Path,
    pub repo: &'a Path,
}

impl<'a> LoadContext<'a> {
    pub fn new(
        enabled: bool,
        args: &'a DispatchArgs,
        cfg: &'a GahConfig,
        profile: &'a crate::config::Profile,
        branch: &'a str,
        worktree_path: &'a Path,
        repo: &'a Path,
    ) -> Self {
        Self {
            enabled,
            cfg,
            profile,
            profile_name: &args.profile,
            repo_id: &profile.repo_id,
            branch,
            expected_review_generation: args.expected_review_generation.as_deref(),
            worktree_path,
            repo,
        }
    }
}

pub(super) fn load_context(
    request: LoadContext<'_>,
    ledger: &mut LedgerEntry,
) -> Result<Option<RepairContext>> {
    if !request.enabled {
        return Ok(None);
    }
    match repair_context::load(
        request.cfg,
        request.profile,
        repair_context::RepairIdentity {
            profile_name: request.profile_name,
            repo_id: request.repo_id,
            branch: request.branch,
            work_id: ledger.work_id.as_deref(),
            expected_review_generation: request.expected_review_generation,
        },
        request.worktree_path,
    ) {
        Ok(context) => {
            ledger.review_source_sha = Some(context.source_sha.clone());
            ledger.review_metadata_fingerprint = Some(context.metadata_fingerprint.clone());
            ledger.review_contract_version = Some(context.review_contract_version);
            ledger.review_generation = Some(context.review_generation.clone());
            Ok(Some(context))
        }
        Err(error) => {
            // Issue #551: a stale remote source (advanced, merged, or
            // closed) is an expected terminal outcome, not a harness bug --
            // classify it distinctly so the controller stops cleanly instead
            // of treating this branch as broken.
            if repair_context::stale_source_error(&error).is_some() {
                ledger.set_failure(FailureClass::StaleSource, FailureStage::Preflight);
                worktree::cleanup(request.worktree_path, request.repo);
                return Err(error.context(
                    "FixMr requires structured findings from the latest applicable review",
                ));
            }
            // Issue #931: a CI_FAILED-classified repair (decision.rs rule 6)
            // goes straight to FixMr without ever routing through ReviewMr,
            // so "no review has ever run at this generation" is an expected
            // outcome for that path, not a harness bug -- treating it as a
            // hard failure wedged the controller into retrying `fix_mr`
            // against a generation that could never be satisfied, forever
            // (confirmed: #818 retried identically for 2+ hours before the
            // stuck-loop gate finally caught it). Proceed without repair
            // context instead; the worker still has the branch's actual
            // state and CI/validation output to work from.
            if repair_context::no_review_found_error(&error).is_some() {
                println!(
                    "No structured review found for this repair generation (expected for a CI_FAILED-triggered fix that never went through review) -- proceeding without review findings."
                );
                return Ok(None);
            }
            ledger.set_failure(FailureClass::HarnessError, FailureStage::Preflight);
            worktree::cleanup(request.worktree_path, request.repo);
            Err(error
                .context("FixMr requires structured findings from the latest applicable review"))
        }
    }
}
