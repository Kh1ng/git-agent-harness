use anyhow::{bail, Result};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

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
    if let Some(timestamp) = expires_at.as_deref() {
        if OffsetDateTime::parse(timestamp, &Rfc3339).is_err() {
            bail!("--expires-at must be a valid RFC3339 timestamp");
        }
    }
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

    #[test]
    fn cli_approval_lifecycle_transitions_and_redacts_scope_metadata() {
        let (tmp, mut cfg) = test_config();
        let profile = approval_profile(tmp.path());
        cfg.profiles.insert("test".to_string(), profile.clone());
        let config_path = tmp.path().join("gah.toml");
        config::save(&cfg, Some(config_path.to_str().unwrap())).unwrap();

        run(ExternalApprovalCommands::Request {
            profile: "test".to_string(),
            work_id: "ISSUE-42".to_string(),
            credential_label: "odds".to_string(),
            operation_kind: "external_api".to_string(),
            max_requests: Some(2),
            max_dollars: None,
            expires_at: None,
            purpose: Some("test".to_string()),
            config_path: Some(config_path.to_string_lossy().into_owned()),
            json: true,
        })
        .unwrap();

        run(ExternalApprovalCommands::Grant {
            profile: "test".to_string(),
            work_id: "ISSUE-42".to_string(),
            credential_label: "odds".to_string(),
            operation_kind: "external_api".to_string(),
            max_requests: Some(2),
            max_dollars: None,
            expires_at: None,
            purpose: Some("test".to_string()),
            config_path: Some(config_path.to_string_lossy().into_owned()),
            json: true,
        })
        .unwrap();

        let loaded = config::load(Some(config_path.to_str().unwrap())).unwrap();
        let entries = ledger::read_entries(&loaded).unwrap();
        let snapshot = ledger::external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            "ISSUE-42",
            "odds",
            "external_api",
        )
        .unwrap();
        assert_eq!(snapshot.state, "approved");
        assert!(snapshot.active);
        assert_eq!(snapshot.allowed_env_vars, vec!["ODDS_API_KEY".to_string()]);

        run(ExternalApprovalCommands::Revoke {
            profile: "test".to_string(),
            work_id: "ISSUE-42".to_string(),
            credential_label: "odds".to_string(),
            operation_kind: "external_api".to_string(),
            config_path: Some(config_path.to_string_lossy().into_owned()),
            json: true,
        })
        .unwrap();

        let loaded = config::load(Some(config_path.to_str().unwrap())).unwrap();
        let entries = ledger::read_entries(&loaded).unwrap();
        let snapshot = ledger::external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            "ISSUE-42",
            "odds",
            "external_api",
        )
        .unwrap();
        assert_eq!(snapshot.state, "revoked");
        assert!(!snapshot.active);

        run(ExternalApprovalCommands::Grant {
            profile: "test".to_string(),
            work_id: "ISSUE-43".to_string(),
            credential_label: "odds".to_string(),
            operation_kind: "external_api".to_string(),
            max_requests: Some(2),
            max_dollars: None,
            expires_at: None,
            purpose: Some("test".to_string()),
            config_path: Some(config_path.to_string_lossy().into_owned()),
            json: true,
        })
        .unwrap();

        run(ExternalApprovalCommands::Expire {
            profile: "test".to_string(),
            work_id: "ISSUE-43".to_string(),
            credential_label: "odds".to_string(),
            operation_kind: "external_api".to_string(),
            config_path: Some(config_path.to_string_lossy().into_owned()),
            json: true,
        })
        .unwrap();

        let loaded = config::load(Some(config_path.to_str().unwrap())).unwrap();
        let entries = ledger::read_entries(&loaded).unwrap();
        let snapshot = ledger::external_approval_snapshot_from_entries(
            &entries,
            "test",
            &profile.repo_id,
            "ISSUE-43",
            "odds",
            "external_api",
        )
        .unwrap();
        assert_eq!(snapshot.state, "expired");
        assert!(!snapshot.active);
    }

    #[test]
    fn cli_rejects_malformed_expiry_before_writing_ledger() {
        let (tmp, mut cfg) = test_config();
        let profile = approval_profile(tmp.path());
        cfg.profiles.insert("test".to_string(), profile);
        let config_path = tmp.path().join("gah.toml");
        config::save(&cfg, Some(config_path.to_str().unwrap())).unwrap();

        let error = run(ExternalApprovalCommands::Grant {
            profile: "test".to_string(),
            work_id: "ISSUE-BAD-EXPIRY".to_string(),
            credential_label: "odds".to_string(),
            operation_kind: "external_api".to_string(),
            max_requests: Some(2),
            max_dollars: None,
            expires_at: Some("next tuesday".to_string()),
            purpose: Some("test".to_string()),
            config_path: Some(config_path.to_string_lossy().into_owned()),
            json: true,
        })
        .unwrap_err();

        assert!(error.to_string().contains("valid RFC3339"));
        let loaded = config::load(Some(config_path.to_str().unwrap())).unwrap();
        assert!(ledger::read_entries(&loaded).unwrap().is_empty());
    }
}
