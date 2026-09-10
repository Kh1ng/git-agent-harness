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
            // Denied/expired/granted are terminal for auto-raising: the hold
            // stands and re-raising would be an alert storm. Granted cannot
            // co-occur with a gap (the credential would be injected).
            "denied" | "expired" | "granted" => return false,
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
    profile: &Profile,
    ledger: &LedgerEntry,
    gap: &crate::dispatch::attempts::ExternalCredentialGap,
) {
    notify_event(
        cfg,
        profile,
        NotifyEvent::ExternalApprovalRequested {
            profile: profile.display_name.as_str(),
            work_id: ledger.work_id.as_deref().unwrap_or("unknown"),
            credential_label: &gap.label,
            env_vars: &gap.env_vars.join(", "),
            bounds: &format!(
                "max_requests={:?} max_dollars={:?}",
                gap.max_requests, gap.max_dollars
            ),
            purpose: gap.purpose.as_deref(),
            grant_command: &format!(
                "gah external-approval grant --profile {} {} --credential-label {} --operation-kind env_credential",
                profile.display_name,
                ledger.work_id.as_deref().unwrap_or("<work-id>"),
                gap.label
            ),
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
            notify_external_approval_request(cfg, profile, ledger, gap);
        }
    }
    true
}
