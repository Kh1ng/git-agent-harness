//! Issue #653: the external-approval pause gate — request raising, dedup,
//! and notification for work items paused because a declared external
//! credential scope lacks an active grant.

use crate::config::{GahConfig, Profile};
use crate::dispatch::attempts::ExternalApprovalRequiredError;
use crate::ledger::{self, ExternalApprovalRecord, LedgerEntry};
use crate::notifications::{notify_event, NotifyEvent};

/// Raise (or refresh) the external-approval request for one gap scope.
///
/// Returns `true` when a NEW or materially-changed request was raised (the
/// caller should notify). Returns `false` when an identical pending request
/// already exists (dedup — no alert storm), or when the request reached a
/// terminal state (denied/expired): the hold stays and the operator
/// intervenes; GAH never re-raises past a denial.
pub(crate) fn raise_external_approval_request(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    ledger: &LedgerEntry,
    gap: &crate::dispatch::attempts::ExternalCredentialGap,
) -> bool {
    let Some(work_id) = ledger.work_id.as_deref() else {
        return false;
    };
    let Ok(entries) = ledger::read_entries(cfg) else {
        return false;
    };
    if let Some(snapshot) = ledger::external_approval_snapshot_from_entries(
        &entries,
        profile_name,
        &profile.repo_id,
        work_id,
        &gap.label,
        "env_credential",
    ) {
        match snapshot.state.as_str() {
            // Identical pending request: dedup both the ledger append and
            // the notification.
            "requested"
                if snapshot.max_requests == gap.max_requests
                    && snapshot.max_dollars == gap.max_dollars
                    && snapshot.allowed_env_vars == gap.env_vars =>
            {
                return false;
            }
            // Persist derived expiry/cap outcomes once so their resolution
            // notification survives restarts without becoming an alert storm.
            state @ ("denied" | "expired") => {
                let mode = if state == "denied" {
                    "external_approval_deny"
                } else {
                    "external_approval_expire"
                };
                let already_recorded = entries
                    .iter()
                    .rev()
                    .find(|entry| {
                        entry.profile == profile_name
                            && entry.repo_id == profile.repo_id
                            && entry.work_id.as_deref() == Some(work_id)
                            && entry.external_approval.as_ref().is_some_and(|approval| {
                                approval.credential_label.as_deref() == Some(&gap.label)
                                    && approval.operation_kind.as_deref() == Some("env_credential")
                            })
                    })
                    .is_some_and(|entry| entry.mode == mode);
                if !already_recorded {
                    let terminal = ExternalApprovalRecord {
                        state: Some(state.to_string()),
                        operation_kind: Some("env_credential".to_string()),
                        credential_label: Some(gap.label.clone()),
                        allowed_env_vars: Vec::new(),
                        max_requests: None,
                        max_dollars: None,
                        expires_at: None,
                        purpose: None,
                        consumed_requests: None,
                        consumed_dollars: None,
                        denial_reason: snapshot.denial_reason,
                    };
                    match ledger::append_external_approval(
                        cfg,
                        LedgerEntry::new_external_approval(
                            profile_name,
                            profile,
                            work_id,
                            mode,
                            terminal,
                        ),
                    ) {
                        Ok(_) => notify_event(
                            cfg,
                            profile,
                            NotifyEvent::ExternalApprovalResolved {
                                profile: profile_name,
                                work_id,
                                credential_label: &gap.label,
                                state,
                            },
                        ),
                        Err(err) => eprintln!(
                            "warning: failed to persist external approval {state}: {err:#}"
                        ),
                    }
                }
                return false;
            }
            // Granted cannot co-occur with a gap because the credential would
            // be injected. Do not turn that inconsistency into a new request.
            "granted" => return false,
            _ => {}
        }
    }

    let approval = ExternalApprovalRecord {
        state: Some("requested".to_string()),
        operation_kind: Some("env_credential".to_string()),
        credential_label: Some(gap.label.clone()),
        allowed_env_vars: gap.env_vars.clone(),
        max_requests: gap.max_requests,
        max_dollars: gap.max_dollars,
        expires_at: None,
        purpose: gap.purpose.clone(),
        consumed_requests: None,
        consumed_dollars: None,
        denial_reason: None,
    };
    let entry = LedgerEntry::new_external_approval(
        profile_name,
        profile,
        work_id,
        "external_approval_request",
        approval,
    );
    // append_external_approval validates the scope and appends under the
    // ledger write lock; a failure here is swallowed (the hold is already
    // latched on the dispatch ledger, so the work item stays paused even if
    // the request record could not be written — the next cycle re-raises).
    let _ = ledger::append_external_approval(cfg, entry);
    true
}

/// Fire the operator notification for a newly-raised request through every
/// configured surface (notify_command + notification channels, #1179).
pub(crate) fn notify_external_approval_request(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    ledger: &LedgerEntry,
    gap: &crate::dispatch::attempts::ExternalCredentialGap,
) {
    let work_id = ledger.work_id.as_deref().unwrap_or("<work-id>");
    let work_url = if work_id.starts_with("https://") || work_id.starts_with("http://") {
        Some(work_id.to_string())
    } else {
        let number = work_id.trim_start_matches('#');
        profile
            .web_url()
            .filter(|_| number.chars().all(|c| c.is_ascii_digit()))
            .map(|repo| {
                if profile.provider.eq_ignore_ascii_case("gitlab") {
                    format!("{repo}/-/issues/{number}")
                } else {
                    format!("{repo}/issues/{number}")
                }
            })
    };
    let cli_arg = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
    let base_command = format!(
        "--profile {} --work-id {} --credential-label {} --operation-kind env_credential",
        cli_arg(profile_name),
        cli_arg(work_id),
        cli_arg(&gap.label),
    );
    let grant_command = format!("gah external-approval grant {base_command}");
    let deny_command = format!("gah external-approval deny {base_command}");
    let bounds = format!(
        "max_requests={} max_dollars={}",
        gap.max_requests
            .map_or_else(|| "unbounded".to_string(), |value| value.to_string()),
        gap.max_dollars
            .map_or_else(|| "unbounded".to_string(), |value| value.to_string()),
    );
    notify_event(
        cfg,
        profile,
        NotifyEvent::ExternalApprovalRequested {
            profile: profile_name,
            project: &profile.repo_id,
            work_id,
            work_url: work_url.as_deref(),
            credential_label: &gap.label,
            env_vars: &gap.env_vars.join(", "),
            bounds: &bounds,
            expires_at: "none",
            purpose: gap.purpose.as_deref(),
            grant_command: &grant_command,
            deny_command: &deny_command,
        },
    );
}

/// Issue #653: match a pause error and latch the durable work-item hold.
/// Returns `true` when the error was an external-approval pause and the
/// ledger entry now carries the hold — the workflow then skips the backend
/// launch (no attempt consumed) and ends the dispatch cleanly.
pub(crate) fn latch_external_approval_hold(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    ledger: &mut LedgerEntry,
    error: &anyhow::Error,
) -> bool {
    let Some(gap_error) = error.downcast_ref::<ExternalApprovalRequiredError>() else {
        return false;
    };
    ledger.validation_result = Some("external_api_approval_required".into());
    ledger.human_required = true;
    ledger.human_required_reason_code = Some(
        crate::controller::HumanRequiredReason::ExternalApiApprovalRequired
            .as_str()
            .to_string(),
    );
    ledger.failure_class = Some(crate::ledger::FailureClass::HumanBlocked.as_str().into());
    ledger.failure_stage = None;
    ledger.error_summary = Some(format!("{gap_error}"));
    for gap in &gap_error.gaps {
        if raise_external_approval_request(cfg, profile_name, profile, ledger, gap) {
            notify_external_approval_request(cfg, profile_name, profile, ledger, gap);
        }
    }
    true
}
