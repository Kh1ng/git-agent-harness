//! Paid-route notices share the ledger's locking and approval history without
//! adding dispatch attempts or human gates for otherwise-valid fallbacks.

use super::{LedgerEntry, RoutingCandidateDiagnostic, RoutingDiagnostics};
use crate::config::{GahConfig, Profile};
use crate::notifications::{notify_event, NotifyEvent};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Serialize, Deserialize)]
struct Notice {
    profile: String,
    repo_id: String,
    work_id: String,
    backend_instance: String,
    model: Option<String>,
    releases: usize,
}

/// Notify once per ticket/candidate occurrence, including skipped candidates
/// in successful fallback decisions. Exact grants/revocations and clear-attempts
/// begin a new occurrence. Routine quota/auth skips never create a notice.
/// Storage/delivery failures are reported without changing routing or approval.
pub(crate) fn notify_paid_route_skips(
    cfg: &GahConfig,
    profile: &Profile,
    entry: &LedgerEntry,
    diagnostics: Option<&RoutingDiagnostics>,
) {
    if profile.notify_command.is_none() {
        return;
    }
    let (Some(work_id), Some(diagnostics)) = (entry.work_id.as_deref(), diagnostics) else {
        return;
    };
    let now = OffsetDateTime::now_utc();
    let alternative = diagnostics
        .candidates
        .iter()
        .filter(|candidate| {
            candidate
                .skip_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("quota_exhausted"))
        })
        .filter_map(|candidate| {
            let reset =
                OffsetDateTime::parse(candidate.unavailable_until.as_deref()?, &Rfc3339).ok()?;
            (reset > now).then_some((candidate, reset))
        })
        .min_by_key(|(_, reset)| *reset)
        .map(|(candidate, _)| candidate);
    for candidate in diagnostics
        .candidates
        .iter()
        .filter(|candidate| candidate.skip_reason.as_deref() == Some("operator_approval_required"))
    {
        match claim_notice(cfg, entry, work_id, candidate) {
            Ok(true) => notify_event(
                cfg,
                profile,
                NotifyEvent::PaidRouteApprovalRequired {
                    profile: &entry.profile,
                    work_id,
                    candidate,
                    alternative,
                },
            ),
            Ok(false) => {}
            Err(error) => eprintln!("[gah] failed to record paid-route notice: {error:#}"),
        }
    }
}

/// Reserve delivery under a cross-process lock before invoking the configured
/// notifier. Like other GAH notices, this is an at-most-once delivery attempt;
/// external delivery cannot be made transactional with local persistence.
fn claim_notice(
    cfg: &GahConfig,
    entry: &LedgerEntry,
    work_id: &str,
    candidate: &RoutingCandidateDiagnostic,
) -> anyhow::Result<bool> {
    let path = cfg
        .defaults
        .ledger_path()
        .with_extension("paid-route-notices.json");
    let _lock = super::locking::exclusive(&path)?;
    let mut notices: Vec<Notice> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("reading paid-route notice journal")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let aliases = super::work_id_aliases(work_id);
    let instance = candidate
        .backend_instance
        .as_deref()
        .unwrap_or(&candidate.backend);
    let releases = super::read_entries(cfg)?
        .iter()
        .filter(|row| {
            row.profile == entry.profile
                && row.repo_id == entry.repo_id
                && row.work_id.as_ref().is_some_and(|id| aliases.contains(id))
                && (row.mode == "clear_attempts"
                    || (matches!(
                        row.mode.as_str(),
                        "paid_route_approval_grant" | "paid_route_approval_revoke"
                    ) && row
                        .usage
                        .backend_instance
                        .as_deref()
                        .unwrap_or(&row.effective_backend)
                        == instance
                        && row.effective_model == candidate.model))
        })
        .count();
    let existing = notices.iter_mut().find(|notice| {
        notice.profile == entry.profile
            && notice.repo_id == entry.repo_id
            && aliases.contains(&notice.work_id)
            && notice.backend_instance == instance
            && notice.model == candidate.model
    });
    if let Some(notice) = existing {
        if notice.releases == releases {
            return Ok(false);
        }
        notice.releases = releases;
    } else {
        notices.push(Notice {
            profile: entry.profile.clone(),
            repo_id: entry.repo_id.clone(),
            work_id: work_id.to_string(),
            backend_instance: instance.to_string(),
            model: candidate.model.clone(),
            releases,
        });
    }
    let mut pending =
        tempfile::NamedTempFile::new_in(path.parent().context("notice journal directory")?)?;
    serde_json::to_writer(&mut pending, &notices)?;
    pending.as_file().sync_all()?;
    pending.persist(&path)?;
    Ok(true)
}

#[cfg(test)]
mod tests;
