use super::{work_id_aliases, ExternalApprovalRecord, LedgerEntry};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Instance-aware approval destinations. Historical approvals without an
/// explicit instance retain their legacy backend/model key; new grants target
/// the exact backend-instance/model destination.
pub fn active_paid_route_approval_destinations_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    work_id: &str,
) -> HashSet<(String, Option<String>)> {
    let mut active = HashSet::new();
    let aliases = work_id_aliases(work_id);
    for entry in entries {
        if entry.profile != profile_name
            || !entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
        {
            continue;
        }
        let identity = (
            entry
                .usage
                .backend_instance
                .clone()
                .unwrap_or_else(|| entry.effective_backend.clone()),
            entry.effective_model.clone(),
        );
        match entry.mode.as_str() {
            "paid_route_approval_grant" => {
                active.insert(identity);
            }
            "paid_route_approval_revoke" => {
                active.remove(&identity);
            }
            _ => {}
        }
    }
    active
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalApprovalSnapshot {
    pub profile: String,
    pub repo_id: String,
    pub work_id: String,
    pub credential_label: String,
    pub operation_kind: String,
    pub state: String,
    pub active: bool,
    pub allowed_env_vars: Vec<String>,
    pub max_requests: Option<u64>,
    pub max_dollars: Option<f64>,
    pub expires_at: Option<String>,
    pub purpose: Option<String>,
    pub consumed_requests: u64,
    pub consumed_dollars: Option<f64>,
    pub denial_reason: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct ExternalApprovalTally {
    request: Option<ExternalApprovalRecord>,
    grant: Option<ExternalApprovalRecord>,
    consumed_requests: u64,
    consumed_dollars: Option<f64>,
    dollars_unknown: bool,
    revoked: bool,
    expired: bool,
    denied_reason: Option<String>,
}

fn matches_external_scope(
    entry: &LedgerEntry,
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
    credential_label: &str,
    operation_kind: &str,
) -> bool {
    entry.profile == profile_name
        && entry.repo_id == repo_id
        && entry.work_id.as_deref() == Some(work_id)
        && entry.external_approval.as_ref().is_some_and(|approval| {
            approval.credential_label.as_deref() == Some(credential_label)
                && approval.operation_kind.as_deref() == Some(operation_kind)
        })
}

fn snapshot_from_state(
    entry: &LedgerEntry,
    approval: &ExternalApprovalRecord,
    tally: &ExternalApprovalTally,
) -> ExternalApprovalSnapshot {
    let mut state = approval
        .state
        .clone()
        .unwrap_or_else(|| "approved".to_string());
    let mut active = true;
    let mut denial_reason = tally
        .denied_reason
        .clone()
        .or_else(|| approval.denial_reason.clone());

    let expired_now = approval.expires_at.as_deref().and_then(|timestamp| {
        time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339).ok()
    });

    if tally.revoked {
        state = "revoked".to_string();
        active = false;
    } else if tally.expired
        || expired_now.is_some_and(|expires_at| expires_at <= time::OffsetDateTime::now_utc())
    {
        state = "expired".to_string();
        active = false;
        denial_reason.get_or_insert_with(|| "expired".to_string());
    } else if tally.denied_reason.is_some() {
        state = "denied".to_string();
        active = false;
    } else if tally.consumed_requests > 0 || tally.consumed_dollars.is_some() {
        state = "consumed".to_string();
    }

    if active {
        if let Some(max_requests) = approval.max_requests {
            if tally.consumed_requests >= max_requests {
                state = "denied".to_string();
                active = false;
                denial_reason.get_or_insert_with(|| "request cap reached".to_string());
            }
        }
        if active && approval.max_dollars.is_some() {
            if tally.dollars_unknown {
                state = "denied".to_string();
                active = false;
                denial_reason.get_or_insert_with(|| "usage unknown".to_string());
            } else if let Some(max_dollars) = approval.max_dollars {
                let consumed = tally.consumed_dollars.unwrap_or(0.0);
                if consumed >= max_dollars {
                    state = "denied".to_string();
                    active = false;
                    denial_reason.get_or_insert_with(|| "dollar cap reached".to_string());
                }
            }
        }
    }

    ExternalApprovalSnapshot {
        profile: entry.profile.clone(),
        repo_id: entry.repo_id.clone(),
        work_id: entry.work_id.clone().unwrap_or_default(),
        credential_label: approval.credential_label.clone().unwrap_or_default(),
        operation_kind: approval.operation_kind.clone().unwrap_or_default(),
        state,
        active,
        allowed_env_vars: approval.allowed_env_vars.clone(),
        max_requests: approval.max_requests,
        max_dollars: approval.max_dollars,
        expires_at: approval.expires_at.clone(),
        purpose: approval.purpose.clone(),
        consumed_requests: tally.consumed_requests,
        consumed_dollars: tally.consumed_dollars,
        denial_reason,
    }
}

pub fn external_approval_snapshot_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
    credential_label: &str,
    operation_kind: &str,
) -> Option<ExternalApprovalSnapshot> {
    let mut tally = ExternalApprovalTally::default();
    let mut current: Option<ExternalApprovalRecord> = None;
    let mut current_entry: Option<&LedgerEntry> = None;

    for entry in entries {
        if !matches_external_scope(
            entry,
            profile_name,
            repo_id,
            work_id,
            credential_label,
            operation_kind,
        ) {
            continue;
        }
        let Some(approval) = entry.external_approval.as_ref() else {
            continue;
        };
        current = Some(approval.clone());
        current_entry = Some(entry);
        match entry.mode.as_str() {
            "external_approval_request" => {
                tally = ExternalApprovalTally::default();
                tally.request = Some(approval.clone());
            }
            "external_approval_grant" => {
                tally = ExternalApprovalTally::default();
                tally.grant = Some(approval.clone());
            }
            "external_approval_consume" => {
                tally.consumed_requests += approval.consumed_requests.unwrap_or(1);
                match approval.consumed_dollars {
                    Some(dollars) => {
                        tally.consumed_dollars =
                            Some(tally.consumed_dollars.unwrap_or(0.0) + dollars);
                    }
                    None => tally.dollars_unknown = true,
                }
            }
            "external_approval_revoke" => {
                tally.revoked = true;
            }
            "external_approval_expire" => {
                tally.expired = true;
            }
            "external_approval_deny" => {
                tally.denied_reason = approval
                    .denial_reason
                    .clone()
                    .or_else(|| Some("denied".to_string()));
            }
            _ => {}
        }
    }

    current.and_then(|approval| {
        current_entry.map(|entry| snapshot_from_state(entry, &approval, &tally))
    })
}

pub fn active_external_approval_env_vars_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
) -> HashSet<String> {
    let mut active_by_label: std::collections::HashMap<String, (ExternalApprovalRecord, bool)> =
        std::collections::HashMap::new();
    for entry in entries {
        if entry.profile != profile_name
            || entry.repo_id != repo_id
            || entry.work_id.as_deref() != Some(work_id)
        {
            continue;
        }
        let Some(approval) = entry.external_approval.as_ref() else {
            continue;
        };
        let label = approval.credential_label.clone().unwrap_or_default();
        match entry.mode.as_str() {
            "external_approval_request" => {
                active_by_label.insert(label, (approval.clone(), false));
            }
            "external_approval_grant" => {
                active_by_label.insert(label, (approval.clone(), true));
            }
            "external_approval_consume" => {
                if let Some((record, active)) = active_by_label.get_mut(&label) {
                    if *active {
                        *record = approval.clone();
                    }
                }
            }
            "external_approval_revoke" | "external_approval_expire" | "external_approval_deny" => {
                if let Some((record, active)) = active_by_label.get_mut(&label) {
                    *record = approval.clone();
                    *active = false;
                } else {
                    active_by_label.insert(label, (approval.clone(), false));
                }
            }
            _ => {}
        }
    }
    active_by_label
        .into_iter()
        .filter_map(|(_, (approval, active))| {
            if !active
                || approval.denial_reason.is_some()
                || approval.state.as_deref() == Some("revoked")
                || approval.state.as_deref() == Some("expired")
            {
                return None;
            }
            Some(approval.allowed_env_vars)
        })
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_targets_one_backend_instance() {
        let profile = crate::ledger::test_util::profile();
        let grant = LedgerEntry::new_paid_route_approval_for_instance(
            "test",
            &profile,
            "ISSUE-42",
            "opencode",
            Some("opencode-api"),
            Some("openai/gpt-5"),
            true,
        );
        let active =
            active_paid_route_approval_destinations_from_entries(&[grant], "test", "ISSUE-42");

        assert!(active.contains(&("opencode-api".into(), Some("openai/gpt-5".into()))));
        assert!(!active.contains(&("opencode-subscription".into(), Some("openai/gpt-5".into()))));
    }
}
