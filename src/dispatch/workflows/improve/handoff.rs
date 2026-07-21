use crate::config::{GahConfig, Profile};
use crate::dispatch::attempts::clear_wip_checkpoints;
use crate::dispatch::issues::TicketMetadata;
use crate::dispatch::publish::handle_handoff_delivery;
use crate::ledger::LedgerEntry;
use crate::worktree;
use anyhow::Result;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(super) fn maybe_perform_handoff(
    cfg: &GahConfig,
    profile: &Profile,
    ledger: &mut LedgerEntry,
    wt: &Path,
    branch: &str,
    commit_msg: &str,
    ticket_meta: Option<&TicketMetadata>,
    backend_summary: &str,
    repo: &Path,
    wip_checkpoints: &[String],
) -> Result<bool> {
    if profile.delivery_mode != crate::config::DeliveryMode::Handoff {
        return Ok(false);
    }
    let ticket_id = ledger
        .work_id
        .clone()
        .or_else(|| {
            ticket_meta.and_then(|m| m.ticket_id.clone().or_else(|| m.issue_number.clone()))
        })
        .unwrap_or_else(|| "unknown".to_string());
    handle_handoff_delivery(
        cfg,
        profile,
        ledger,
        wt,
        branch,
        commit_msg,
        &ticket_id,
        backend_summary,
    )?;
    clear_wip_checkpoints(repo, wip_checkpoints);
    worktree::cleanup(wt, repo);
    Ok(true)
}
