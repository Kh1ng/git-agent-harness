use super::gates::work_id_aliases;
use super::{ExternalApprovalRecord, LedgerEntry};
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

    let parsed_expiry = approval
        .expires_at
        .as_deref()
        .map(|timestamp| OffsetDateTime::parse(timestamp, &Rfc3339));
    let malformed_expiry = parsed_expiry.as_ref().is_some_and(Result::is_err);
    let expired_now = parsed_expiry.and_then(Result::ok);

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
    } else if malformed_expiry {
        state = "denied".to_string();
        active = false;
        denial_reason.get_or_insert_with(|| "invalid expiry timestamp".to_string());
    } else if tally.denied_reason.is_some() || approval.state.as_deref() == Some("denied") {
        state = "denied".to_string();
        active = false;
    } else if state == "requested" {
        active = false;
    } else if state == "consumed" {
        active = tally.grant.is_some();
    }

    if approval
        .max_dollars
        .is_some_and(|cap| !cap.is_finite() || cap < 0.0)
    {
        return ExternalApprovalStateEval {
            state: "denied".to_string(),
            active: false,
            denial_reason: Some("invalid dollar cap".to_string()),
        };
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

// Lifecycle records may change state, never the request/grant's scope.
fn merge_external_approval_record(
    current: &mut ExternalApprovalRecord,
    update: &ExternalApprovalRecord,
) {
    if update.state.is_some() {
        current.state = update.state.clone();
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
) -> HashMap<(String, String), ExternalApprovalScopeState> {
    let mut active_by_scope: HashMap<(String, String), ExternalApprovalScopeState> = HashMap::new();
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
        let scope_key = (
            approval.credential_label.clone().unwrap_or_default(),
            approval.operation_kind.clone().unwrap_or_default(),
        );
        match entry.mode.as_str() {
            "external_approval_request" => {
                active_by_scope.insert(
                    scope_key,
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
                active_by_scope.insert(
                    scope_key,
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
                if let Some(state) = active_by_scope.get_mut(&scope_key) {
                    merge_external_approval_record(&mut state.approval, approval);
                    state.tally.consumed_requests = state
                        .tally
                        .consumed_requests
                        .saturating_add(approval.consumed_requests.unwrap_or(1));
                    match approval.consumed_dollars {
                        Some(dollars) if dollars.is_finite() && dollars >= 0.0 => {
                            let total = state.tally.consumed_dollars.unwrap_or(0.0) + dollars;
                            if total.is_finite() {
                                state.tally.consumed_dollars = Some(total);
                            } else {
                                state.tally.dollars_unknown = true;
                            }
                        }
                        _ => state.tally.dollars_unknown = true,
                    }
                }
            }
            "external_approval_revoke" => {
                if let Some(state) = active_by_scope.get_mut(&scope_key) {
                    merge_external_approval_record(&mut state.approval, approval);
                    state.tally.revoked = true;
                } else {
                    active_by_scope.insert(
                        scope_key,
                        ExternalApprovalScopeState {
                            approval: approval.clone(),
                            tally: ExternalApprovalTally::default(),
                        },
                    );
                }
            }
            "external_approval_expire" => {
                if let Some(state) = active_by_scope.get_mut(&scope_key) {
                    merge_external_approval_record(&mut state.approval, approval);
                    state.tally.expired = true;
                } else {
                    active_by_scope.insert(
                        scope_key,
                        ExternalApprovalScopeState {
                            approval: approval.clone(),
                            tally: ExternalApprovalTally::default(),
                        },
                    );
                }
            }
            "external_approval_deny" => {
                if let Some(state) = active_by_scope.get_mut(&scope_key) {
                    merge_external_approval_record(&mut state.approval, approval);
                    state.tally.denied_reason = approval
                        .denial_reason
                        .clone()
                        .or_else(|| Some("denied".to_string()));
                } else {
                    active_by_scope.insert(
                        scope_key,
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
    active_by_scope
}

/// Project the same scope state used by credential injection and consumption.
/// Sparse lifecycle records retain the original bounds and purpose.
pub fn external_approval_snapshot_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
    credential_label: &str,
    operation_kind: &str,
) -> Option<ExternalApprovalSnapshot> {
    let state = scope_states_from_entries(entries, profile_name, repo_id, work_id)
        .remove(&(credential_label.to_string(), operation_kind.to_string()))?;
    let eval = approval_state(&state.approval, &state.tally);
    Some(ExternalApprovalSnapshot {
        profile: profile_name.to_string(),
        repo_id: repo_id.to_string(),
        work_id: work_id.to_string(),
        credential_label: credential_label.to_string(),
        operation_kind: operation_kind.to_string(),
        state: eval.state,
        active: eval.active,
        allowed_env_vars: state.approval.allowed_env_vars,
        max_requests: state.approval.max_requests,
        max_dollars: state.approval.max_dollars,
        expires_at: state.approval.expires_at,
        purpose: state.approval.purpose,
        consumed_requests: state.tally.consumed_requests,
        consumed_dollars: state.tally.consumed_dollars,
        denial_reason: eval.denial_reason,
    })
}

/// Validate a CLI transition against the ledger read under its write lock.
/// A grant consumes a pending request once and may only narrow its scope.
pub(super) fn prepare_external_approval(
    entries: &[LedgerEntry],
    entry: &mut LedgerEntry,
) -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    let work_id = entry
        .work_id
        .as_deref()
        .context("approval requires a work item")?;
    let approval = entry
        .external_approval
        .as_mut()
        .context("approval scope is missing")?;
    let label = approval
        .credential_label
        .as_deref()
        .context("credential label is missing")?;
    let operation = approval
        .operation_kind
        .as_deref()
        .context("operation kind is missing")?;
    for value in [&entry.profile, &entry.repo_id, work_id, label, operation] {
        ensure!(
            !value.trim().is_empty()
                && value.trim() == value
                && !value.chars().any(char::is_control),
            "approval identifiers must be nonempty and contain no control characters"
        );
    }
    if let Some(purpose) = &approval.purpose {
        ensure!(
            !purpose.trim().is_empty() && !purpose.chars().any(char::is_control),
            "approval purpose must be nonempty and contain no control characters"
        );
    }
    ensure!(
        approval.max_requests != Some(0),
        "--max-requests must be positive"
    );
    ensure!(
        approval
            .max_dollars
            .is_none_or(|cap| cap.is_finite() && cap > 0.0),
        "--max-dollars must be finite and positive"
    );
    let expiry = approval
        .expires_at
        .as_deref()
        .map(|value| OffsetDateTime::parse(value, &Rfc3339))
        .transpose()
        .context("--expires-at must be a valid RFC3339 timestamp")?;
    ensure!(
        expiry.is_none_or(|expiry| expiry > OffsetDateTime::now_utc()),
        "--expires-at must be in the future"
    );
    match entry.mode.as_str() {
        "external_approval_request" => {
            ensure!(
                !approval.allowed_env_vars.is_empty(),
                "choose a configured credential scope"
            );
        }
        "external_approval_grant" => {
            let requested = external_approval_snapshot_from_entries(
                entries,
                &entry.profile,
                &entry.repo_id,
                work_id,
                label,
                operation,
            )
            .context("no matching pending request; record an external-approval request first")?;
            ensure!(
                requested.state == "requested",
                "approval is not pending; record a new request before granting again"
            );
            ensure!(
                requested.max_requests != Some(0)
                    && requested
                        .max_dollars
                        .is_none_or(|cap| cap.is_finite() && cap > 0.0),
                "pending request has invalid bounds; record a new request"
            );
            ensure!(
                !approval.allowed_env_vars.is_empty()
                    && approval
                        .allowed_env_vars
                        .iter()
                        .all(|name| requested.allowed_env_vars.contains(name)),
                "configured credentials exceed the requested scope; record a new request"
            );
            if let (Some(grant), Some(request)) = (approval.max_requests, requested.max_requests) {
                ensure!(grant <= request, "request cap exceeds the pending request");
            }
            if let (Some(grant), Some(request)) = (approval.max_dollars, requested.max_dollars) {
                ensure!(grant <= request, "dollar cap exceeds the pending request");
            }
            if let Some(request) = requested.expires_at.as_deref() {
                let requested_expiry = OffsetDateTime::parse(request, &Rfc3339)
                    .context("pending request has invalid expiry")?;
                ensure!(
                    expiry.is_none_or(|grant| grant <= requested_expiry),
                    "expiry exceeds the pending request"
                );
            }
            ensure!(
                approval.purpose.is_none() || approval.purpose == requested.purpose,
                "purpose differs from the pending request; record a new request"
            );
            approval.max_requests = approval.max_requests.or(requested.max_requests);
            approval.max_dollars = approval.max_dollars.or(requested.max_dollars);
            approval.expires_at = approval.expires_at.take().or(requested.expires_at);
            approval.purpose = approval.purpose.take().or(requested.purpose);
        }
        "external_approval_revoke" | "external_approval_expire" => {}
        _ => anyhow::bail!("unsupported external approval transition"),
    }
    Ok(())
}

pub fn active_external_approval_env_vars_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
) -> HashSet<String> {
    let mut by_label: HashMap<String, Vec<ExternalApprovalScopeState>> = HashMap::new();
    for ((label, _operation_kind), state) in
        scope_states_from_entries(entries, profile_name, repo_id, work_id)
    {
        by_label.entry(label).or_default().push(state);
    }
    by_label
        .into_values()
        .filter_map(|mut states| {
            // The backend invocation does not identify which external
            // operation it will perform. Multiple operation grants for the
            // same credential are therefore ambiguous and must not be merged
            // into a broader grant.
            if states.len() != 1 {
                return None;
            }
            let state = states.pop().expect("one state");
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
    _usage: &crate::ledger::LedgerUsage,
) -> anyhow::Result<usize> {
    let Some(work_id) = work_id else {
        return Ok(0);
    };
    let entries = match super::jsonl::read_entries(cfg) {
        Ok(entries) => entries,
        Err(_) => return Ok(0),
    };
    let states = scope_states_from_entries(&entries, profile_name, &profile.repo_id, work_id);
    let mut recorded = 0usize;
    let mut by_label: HashMap<String, Vec<ExternalApprovalScopeState>> = HashMap::new();
    for ((label, _operation_kind), state) in states {
        by_label.entry(label).or_default().push(state);
    }

    for (label, mut scoped_states) in by_label {
        if scoped_states.len() != 1 {
            continue;
        }
        let state = scoped_states.pop().expect("one state");
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
            allowed_env_vars: state.approval.allowed_env_vars.clone(),
            max_requests: state.approval.max_requests,
            max_dollars: state.approval.max_dollars,
            expires_at: state.approval.expires_at.clone(),
            purpose: state.approval.purpose.clone(),
            consumed_requests: Some(1),
            // Backend token cost is not external-service spend. Until a
            // provider reports the latter explicitly, record it as unknown;
            // approvals with a dollar cap then fail closed before a retry.
            consumed_dollars: None,
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
    fn malformed_expiry_fails_closed() {
        let (tmp, cfg) = test_config();
        let profile = approval_profile(tmp.path());
        let work_id = "ISSUE-43-BAD-EXPIRY";
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
                Some("tomorrow-ish".to_string()),
            ),
        );
        crate::ledger::append(&cfg, &grant).unwrap();

        let entries = crate::ledger::read_entries(&cfg).unwrap();
        assert!(active_external_approval_env_vars_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
        )
        .is_empty());
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
        assert_eq!(
            snapshot.denial_reason.as_deref(),
            Some("invalid expiry timestamp")
        );
    }

    #[test]
    fn same_credential_across_operations_is_not_cross_clobbered_or_injected() {
        let (tmp, cfg) = test_config();
        let profile = approval_profile(tmp.path());
        let work_id = "ISSUE-43-CROSS-OP";
        for operation in ["historical_backfill", "current_poll"] {
            let grant = LedgerEntry::new_external_approval(
                "test",
                &profile,
                work_id,
                "external_approval_grant",
                approval_record("odds", operation, "approved", Some(3), None),
            );
            crate::ledger::append(&cfg, &grant).unwrap();
        }

        let entries = crate::ledger::read_entries(&cfg).unwrap();
        let historical = external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
            "odds",
            "historical_backfill",
        )
        .unwrap();
        let current = external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
            "odds",
            "current_poll",
        )
        .unwrap();

        assert_eq!(historical.operation_kind, "historical_backfill");
        assert_eq!(current.operation_kind, "current_poll");
        assert!(historical.active);
        assert!(current.active);
        assert!(active_external_approval_env_vars_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
        )
        .is_empty());
    }

    #[test]
    fn unknown_external_spend_exhausts_a_dollar_capped_scope() {
        let (tmp, cfg) = test_config();
        let profile = approval_profile(tmp.path());
        let work_id = "ISSUE-43-DOLLARS";
        let mut approval = approval_record("odds", "historical_backfill", "approved", None, None);
        approval.max_dollars = Some(5.0);
        crate::ledger::append(
            &cfg,
            &LedgerEntry::new_external_approval(
                "test",
                &profile,
                work_id,
                "external_approval_grant",
                approval,
            ),
        )
        .unwrap();

        record_external_approval_consumption_for_work_item(
            &cfg,
            "test",
            &profile,
            Some(work_id),
            &crate::ledger::LedgerUsage::default(),
        )
        .unwrap();

        let entries = crate::ledger::read_entries(&cfg).unwrap();
        let snapshot = external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            work_id,
            "odds",
            "historical_backfill",
        )
        .unwrap();
        assert!(!snapshot.active);
        assert_eq!(snapshot.denial_reason.as_deref(), Some("usage unknown"));
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

#[cfg(test)]
#[path = "approval_transition_tests.rs"]
mod transition_tests;
