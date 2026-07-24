use super::{work_id_aliases, ExternalApprovalRecord, LedgerEntry};
use crate::config::{GahConfig, Profile};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

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

#[derive(Debug, Clone)]
struct ExternalApprovalScopeState {
    approval: ExternalApprovalRecord,
    tally: ExternalApprovalTally,
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
    let state_eval = approval_state(approval, tally);

    ExternalApprovalSnapshot {
        profile: entry.profile.clone(),
        repo_id: entry.repo_id.clone(),
        work_id: entry.work_id.clone().unwrap_or_default(),
        credential_label: approval.credential_label.clone().unwrap_or_default(),
        operation_kind: approval.operation_kind.clone().unwrap_or_default(),
        state: state_eval.state,
        active: state_eval.active,
        allowed_env_vars: approval.allowed_env_vars.clone(),
        max_requests: approval.max_requests,
        max_dollars: approval.max_dollars,
        expires_at: approval.expires_at.clone(),
        purpose: approval.purpose.clone(),
        consumed_requests: tally.consumed_requests,
        consumed_dollars: tally.consumed_dollars,
        denial_reason: state_eval.denial_reason,
    }
}

#[derive(Debug)]
struct ExternalApprovalStateEval {
    state: String,
    active: bool,
    denial_reason: Option<String>,
}

fn approval_state(
    approval: &ExternalApprovalRecord,
    tally: &ExternalApprovalTally,
) -> ExternalApprovalStateEval {
    let mut state = approval.state.clone().unwrap_or_else(|| {
        if tally.request.is_some() && tally.grant.is_none() {
            "requested".to_string()
        } else if tally.consumed_requests > 0 || tally.consumed_dollars.is_some() {
            "consumed".to_string()
        } else {
            "approved".to_string()
        }
    });
    let mut active = tally.grant.is_some() && state != "requested";
    let mut denial_reason = tally
        .denied_reason
        .clone()
        .or_else(|| approval.denial_reason.clone());

    let expired_now = approval
        .expires_at
        .as_deref()
        .and_then(|timestamp| OffsetDateTime::parse(timestamp, &Rfc3339).ok());

    if tally.revoked || approval.state.as_deref() == Some("revoked") {
        state = "revoked".to_string();
        active = false;
    } else if tally.expired
        || approval.state.as_deref() == Some("expired")
        || expired_now.is_some_and(|expires_at| expires_at <= OffsetDateTime::now_utc())
    {
        state = "expired".to_string();
        active = false;
        denial_reason.get_or_insert_with(|| "expired".to_string());
    } else if tally.denied_reason.is_some() || approval.state.as_deref() == Some("denied") {
        state = "denied".to_string();
        active = false;
    } else if state == "requested" {
        active = false;
    } else if state == "consumed" {
        active = tally.grant.is_some();
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

    ExternalApprovalStateEval {
        state,
        active,
        denial_reason,
    }
}

fn merge_external_approval_record(
    current: &mut ExternalApprovalRecord,
    update: &ExternalApprovalRecord,
) {
    if !update.allowed_env_vars.is_empty() || current.allowed_env_vars.is_empty() {
        current.allowed_env_vars = update.allowed_env_vars.clone();
    }
    if update.state.is_some() {
        current.state = update.state.clone();
    }
    if update.operation_kind.is_some() {
        current.operation_kind = update.operation_kind.clone();
    }
    if update.credential_label.is_some() {
        current.credential_label = update.credential_label.clone();
    }
    if update.max_requests.is_some() {
        current.max_requests = update.max_requests;
    }
    if update.max_dollars.is_some() {
        current.max_dollars = update.max_dollars;
    }
    if update.expires_at.is_some() {
        current.expires_at = update.expires_at.clone();
    }
    if update.purpose.is_some() {
        current.purpose = update.purpose.clone();
    }
    if update.consumed_requests.is_some() {
        current.consumed_requests = update.consumed_requests;
    }
    if update.consumed_dollars.is_some() {
        current.consumed_dollars = update.consumed_dollars;
    }
    if update.denial_reason.is_some() {
        current.denial_reason = update.denial_reason.clone();
    }
}

fn scope_states_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
) -> HashMap<String, ExternalApprovalScopeState> {
    let mut active_by_label: HashMap<String, ExternalApprovalScopeState> = HashMap::new();
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
                active_by_label.insert(
                    label,
                    ExternalApprovalScopeState {
                        approval: approval.clone(),
                        tally: ExternalApprovalTally {
                            request: Some(approval.clone()),
                            ..ExternalApprovalTally::default()
                        },
                    },
                );
            }
            "external_approval_grant" => {
                active_by_label.insert(
                    label,
                    ExternalApprovalScopeState {
                        approval: approval.clone(),
                        tally: ExternalApprovalTally {
                            grant: Some(approval.clone()),
                            ..ExternalApprovalTally::default()
                        },
                    },
                );
            }
            "external_approval_consume" => {
                if let Some(state) = active_by_label.get_mut(&label) {
                    merge_external_approval_record(&mut state.approval, approval);
                    state.tally.consumed_requests += approval.consumed_requests.unwrap_or(1);
                    match approval.consumed_dollars {
                        Some(dollars) => {
                            state.tally.consumed_dollars =
                                Some(state.tally.consumed_dollars.unwrap_or(0.0) + dollars);
                        }
                        None => state.tally.dollars_unknown = true,
                    }
                }
            }
            "external_approval_revoke" => {
                if let Some(state) = active_by_label.get_mut(&label) {
                    merge_external_approval_record(&mut state.approval, approval);
                    state.tally.revoked = true;
                } else {
                    active_by_label.insert(
                        label,
                        ExternalApprovalScopeState {
                            approval: approval.clone(),
                            tally: ExternalApprovalTally::default(),
                        },
                    );
                }
            }
            "external_approval_expire" => {
                if let Some(state) = active_by_label.get_mut(&label) {
                    merge_external_approval_record(&mut state.approval, approval);
                    state.tally.expired = true;
                } else {
                    active_by_label.insert(
                        label,
                        ExternalApprovalScopeState {
                            approval: approval.clone(),
                            tally: ExternalApprovalTally::default(),
                        },
                    );
                }
            }
            "external_approval_deny" => {
                if let Some(state) = active_by_label.get_mut(&label) {
                    merge_external_approval_record(&mut state.approval, approval);
                    state.tally.denied_reason = approval
                        .denial_reason
                        .clone()
                        .or_else(|| Some("denied".to_string()));
                } else {
                    active_by_label.insert(
                        label,
                        ExternalApprovalScopeState {
                            approval: approval.clone(),
                            tally: ExternalApprovalTally {
                                denied_reason: approval
                                    .denial_reason
                                    .clone()
                                    .or_else(|| Some("denied".to_string())),
                                ..ExternalApprovalTally::default()
                            },
                        },
                    );
                }
            }
            _ => {}
        }
    }
    active_by_label
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
    scope_states_from_entries(entries, profile_name, repo_id, work_id)
        .into_iter()
        .filter_map(|(_, state)| {
            let eval = approval_state(&state.approval, &state.tally);
            if !eval.active {
                return None;
            }
            Some(state.approval.allowed_env_vars)
        })
        .flatten()
        .collect()
}

pub fn record_external_approval_consumption_for_work_item(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    work_id: Option<&str>,
    usage: &crate::ledger::LedgerUsage,
) -> anyhow::Result<usize> {
    let Some(work_id) = work_id else {
        return Ok(0);
    };
    let entries = match super::jsonl::read_entries(cfg) {
        Ok(entries) => entries,
        Err(_) => return Ok(0),
    };
    let states = scope_states_from_entries(&entries, profile_name, &profile.repo_id, work_id);
    let consumed_dollars = usage.actual_cost_usd.or(usage.estimated_cost_usd);
    let mut recorded = 0usize;

    for (label, state) in states {
        let Some(scope) = profile.external_credential_scope(&label) else {
            continue;
        };
        if scope.env_vars.is_empty() {
            continue;
        }
        let eval = approval_state(&state.approval, &state.tally);
        if !eval.active {
            continue;
        }
        let approval = ExternalApprovalRecord {
            state: Some("consumed".to_string()),
            operation_kind: state.approval.operation_kind.clone(),
            credential_label: state.approval.credential_label.clone(),
            allowed_env_vars: scope.env_vars.clone(),
            max_requests: state.approval.max_requests,
            max_dollars: state.approval.max_dollars,
            expires_at: state.approval.expires_at.clone(),
            purpose: state.approval.purpose.clone(),
            consumed_requests: Some(1),
            consumed_dollars,
            denial_reason: None,
        };
        let entry = LedgerEntry::new_external_approval(
            profile_name,
            profile,
            work_id,
            "external_approval_consume",
            approval,
        );
        super::jsonl::append(cfg, &entry)?;
        recorded += 1;
    }

    Ok(recorded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ExternalCredentialScope;
    use crate::ledger::test_util::{profile as test_profile, test_config};
    use std::collections::HashMap;

    fn approval_profile(tmp: &std::path::Path) -> crate::config::Profile {
        let mut profile = test_profile();
        profile.artifact_root = tmp.display().to_string();
        profile.local_path = tmp.display().to_string();
        profile.external_credential_scopes = HashMap::from([(
            "odds".to_string(),
            ExternalCredentialScope {
                env_vars: vec!["ODDS_API_KEY".to_string()],
            },
        )]);
        profile
    }

    fn approval_record(
        label: &str,
        operation_kind: &str,
        state: &str,
        max_requests: Option<u64>,
        expires_at: Option<String>,
    ) -> ExternalApprovalRecord {
        ExternalApprovalRecord {
            state: Some(state.to_string()),
            operation_kind: Some(operation_kind.to_string()),
            credential_label: Some(label.to_string()),
            allowed_env_vars: vec!["ODDS_API_KEY".to_string()],
            max_requests,
            max_dollars: None,
            expires_at,
            purpose: Some("test approval".to_string()),
            consumed_requests: None,
            consumed_dollars: None,
            denial_reason: None,
        }
    }

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

    #[test]
    fn expired_or_capped_external_approvals_do_not_inject_credentials() {
        let (tmp, cfg) = test_config();
        let profile = approval_profile(tmp.path());
        let work_id = "ISSUE-42";
        let grant = LedgerEntry::new_external_approval(
            "test",
            &profile,
            work_id,
            "external_approval_grant",
            approval_record("odds", "external_api", "approved", Some(1), None),
        );
        crate::ledger::append(&cfg, &grant).unwrap();
        record_external_approval_consumption_for_work_item(
            &cfg,
            "test",
            &profile,
            Some(work_id),
            &crate::ledger::LedgerUsage::default(),
        )
        .unwrap();

        let entries = crate::ledger::read_entries(&cfg).unwrap();
        let active = active_external_approval_env_vars_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
        );
        assert!(active.is_empty());

        let snapshot = external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
            "odds",
            "external_api",
        )
        .unwrap();
        assert_eq!(snapshot.state, "denied");
        assert!(!snapshot.active);
        assert_eq!(
            snapshot.denial_reason.as_deref(),
            Some("request cap reached")
        );
    }

    #[test]
    fn expired_grants_are_rejected_before_the_next_attempt() {
        let (tmp, cfg) = test_config();
        let profile = approval_profile(tmp.path());
        let work_id = "ISSUE-43";
        let expired_at = (time::OffsetDateTime::now_utc() - time::Duration::hours(1))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let grant = LedgerEntry::new_external_approval(
            "test",
            &profile,
            work_id,
            "external_approval_grant",
            approval_record(
                "odds",
                "external_api",
                "approved",
                Some(3),
                Some(expired_at),
            ),
        );
        crate::ledger::append(&cfg, &grant).unwrap();

        let entries = crate::ledger::read_entries(&cfg).unwrap();
        let active = active_external_approval_env_vars_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
        );
        assert!(active.is_empty());

        let snapshot = external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
            "odds",
            "external_api",
        )
        .unwrap();
        assert_eq!(snapshot.state, "expired");
        assert!(!snapshot.active);
    }

    #[test]
    fn cross_scope_reuse_is_rejected() {
        let (tmp, cfg) = test_config();
        let mut profile = approval_profile(tmp.path());
        profile.repo_id = "repo-a".to_string();
        let other_profile = {
            let mut p = profile.clone();
            p.repo_id = "repo-b".to_string();
            p
        };
        let work_id = "ISSUE-44";
        let grant = LedgerEntry::new_external_approval(
            "test",
            &other_profile,
            work_id,
            "external_approval_grant",
            approval_record("odds", "external_api", "approved", Some(2), None),
        );
        crate::ledger::append(&cfg, &grant).unwrap();

        let entries = crate::ledger::read_entries(&cfg).unwrap();
        assert!(external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
            "odds",
            "external_api",
        )
        .is_none());
        assert!(active_external_approval_env_vars_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
        )
        .is_empty());
    }
}
