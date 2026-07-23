use anyhow::Result;

use crate::cli::args::ExternalApprovalCommands;
use crate::config;
use crate::ledger::{self, ExternalApprovalRecord, ExternalApprovalSnapshot, LedgerEntry};

#[derive(serde::Serialize)]
struct ScopeStatus {
    scope: ExternalApprovalSnapshot,
    ledger_path: String,
}

pub fn run(command: ExternalApprovalCommands) -> Result<()> {
    match command {
        ExternalApprovalCommands::Request {
            profile,
            work_id,
            credential_label,
            operation_kind,
            max_requests,
            max_dollars,
            expires_at,
            purpose,
            config_path,
            json,
        } => write_transition(
            "external_approval_request",
            &profile,
            &work_id,
            &credential_label,
            &operation_kind,
            max_requests,
            max_dollars,
            expires_at,
            purpose,
            config_path,
            json,
            "requested",
            None,
        ),
        ExternalApprovalCommands::Inspect {
            profile,
            work_id,
            credential_label,
            operation_kind,
            config_path,
            json,
        } => inspect(
            &profile,
            &work_id,
            &credential_label,
            &operation_kind,
            config_path,
            json,
        ),
        ExternalApprovalCommands::Grant {
            profile,
            work_id,
            credential_label,
            operation_kind,
            max_requests,
            max_dollars,
            expires_at,
            purpose,
            config_path,
            json,
        } => write_transition(
            "external_approval_grant",
            &profile,
            &work_id,
            &credential_label,
            &operation_kind,
            max_requests,
            max_dollars,
            expires_at,
            purpose,
            config_path,
            json,
            "approved",
            None,
        ),
        ExternalApprovalCommands::Revoke {
            profile,
            work_id,
            credential_label,
            operation_kind,
            config_path,
            json,
        } => write_transition(
            "external_approval_revoke",
            &profile,
            &work_id,
            &credential_label,
            &operation_kind,
            None,
            None,
            None,
            None,
            config_path,
            json,
            "revoked",
            Some("revoked"),
        ),
        ExternalApprovalCommands::Expire {
            profile,
            work_id,
            credential_label,
            operation_kind,
            config_path,
            json,
        } => write_transition(
            "external_approval_expire",
            &profile,
            &work_id,
            &credential_label,
            &operation_kind,
            None,
            None,
            None,
            None,
            config_path,
            json,
            "expired",
            Some("expired"),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_transition(
    mode: &str,
    profile: &str,
    work_id: &str,
    credential_label: &str,
    operation_kind: &str,
    max_requests: Option<u64>,
    max_dollars: Option<f64>,
    expires_at: Option<String>,
    purpose: Option<String>,
    config_path: Option<String>,
    json: bool,
    state: &str,
    denial_reason: Option<&str>,
) -> Result<()> {
    let cfg = config::load(config_path.as_deref())?;
    let prof = config::get_profile(&cfg, profile)?;
    let allowed_env_vars = prof
        .external_credential_scope(credential_label)
        .map(|scope| scope.env_vars.clone())
        .unwrap_or_default();
    let approval = ExternalApprovalRecord {
        state: Some(state.to_string()),
        operation_kind: Some(operation_kind.to_string()),
        credential_label: Some(credential_label.to_string()),
        allowed_env_vars,
        max_requests,
        max_dollars,
        expires_at,
        purpose,
        consumed_requests: if mode == "external_approval_grant" {
            Some(0)
        } else {
            None
        },
        consumed_dollars: None,
        denial_reason: denial_reason.map(str::to_string),
    };
    let entry = LedgerEntry::new_external_approval(profile, prof, work_id, mode, approval);
    let path = ledger::append(&cfg, &entry)?;
    if json {
        println!("{}", serde_json::to_string(&entry)?);
    } else {
        println!(
            "{} {} for work_id '{}' / '{}' ({})",
            state,
            credential_label,
            work_id,
            operation_kind,
            path.display()
        );
    }
    Ok(())
}

fn inspect(
    profile: &str,
    work_id: &str,
    credential_label: &str,
    operation_kind: &str,
    config_path: Option<String>,
    json: bool,
) -> Result<()> {
    let cfg = config::load(config_path.as_deref())?;
    let prof = config::get_profile(&cfg, profile)?;
    let snapshot = ledger::external_approval_snapshot_from_entries(
        &ledger::read_entries(&cfg)?,
        profile,
        &prof.repo_id,
        work_id,
        credential_label,
        operation_kind,
    )
    .unwrap_or_else(|| ExternalApprovalSnapshot {
        profile: profile.to_string(),
        repo_id: prof.repo_id.clone(),
        work_id: work_id.to_string(),
        credential_label: credential_label.to_string(),
        operation_kind: operation_kind.to_string(),
        state: "denied".to_string(),
        active: false,
        allowed_env_vars: Vec::new(),
        max_requests: None,
        max_dollars: None,
        expires_at: None,
        purpose: None,
        consumed_requests: 0,
        consumed_dollars: None,
        denial_reason: Some("no matching approval".to_string()),
    });
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ScopeStatus {
                scope: snapshot,
                ledger_path: cfg.defaults.ledger_path().display().to_string(),
            })?
        );
    } else {
        println!(
            "{} {} {} {} active={} consumed={} ledger={}",
            snapshot.state,
            snapshot.profile,
            snapshot.repo_id,
            snapshot.work_id,
            snapshot.active,
            snapshot.consumed_requests,
            cfg.defaults.ledger_path().display(),
        );
    }
    Ok(())
}
