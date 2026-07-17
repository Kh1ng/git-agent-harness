use super::super::super::repair_context::{self, RepairContext};
use crate::config::GahConfig;
use crate::ledger::{FailureClass, FailureStage, LedgerEntry};
use crate::worktree;
use anyhow::Result;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(super) fn load_context(
    enabled: bool,
    cfg: &GahConfig,
    profile_name: &str,
    repo_id: &str,
    branch: &str,
    worktree_path: &Path,
    repo: &Path,
    ledger: &mut LedgerEntry,
) -> Result<Option<RepairContext>> {
    if !enabled {
        return Ok(None);
    }
    match repair_context::load(
        cfg,
        profile_name,
        repo_id,
        branch,
        ledger.work_id.as_deref(),
        worktree_path,
    ) {
        Ok(context) => Ok(Some(context)),
        Err(error) => {
            ledger.set_failure(FailureClass::HarnessError, FailureStage::Preflight);
            worktree::cleanup(worktree_path, repo);
            Err(error
                .context("FixMr requires structured findings from the latest applicable review"))
        }
    }
}
