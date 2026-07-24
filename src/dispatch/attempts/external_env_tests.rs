use super::*;
use crate::config::ExternalCredentialScope;
use crate::config::RoutingPolicy;
use crate::dispatch::test_util::{gah_config_with_ledger, profile};
use crate::ledger::LedgerEntry;
use std::fs;

#[test]
fn external_env_vars_preserve_non_external_content_and_require_active_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.artifact_root = tmp.path().to_string_lossy().into_owned();
    prof.external_credential_scopes.insert(
        "odds".to_string(),
        ExternalCredentialScope {
            env_vars: vec!["ODDS_API_KEY".to_string()],
        },
    );
    let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
    let env_path = tmp.path().join("task.env");
    fs::write(
        &env_path,
        "PUBLIC_SETTING=keep\nODDS_API_KEY=secret-value\nANOTHER_FLAG=42\n",
    )
    .unwrap();

    let without_grant = external_env_vars_for_work_item(
        &cfg,
        "test",
        &prof,
        Some("ISSUE-42"),
        Some(env_path.to_str().unwrap()),
    );
    assert!(without_grant.contains(&("PUBLIC_SETTING".to_string(), "keep".to_string())));
    assert!(without_grant.contains(&("ANOTHER_FLAG".to_string(), "42".to_string())));
    assert!(!without_grant.iter().any(|(key, _)| key == "ODDS_API_KEY"));

    let grant = LedgerEntry::new_external_approval(
        "test",
        &prof,
        "ISSUE-42",
        "external_approval_grant",
        crate::ledger::ExternalApprovalRecord {
            state: Some("approved".to_string()),
            operation_kind: Some("external_api".to_string()),
            credential_label: Some("odds".to_string()),
            allowed_env_vars: vec!["ODDS_API_KEY".to_string()],
            max_requests: Some(3),
            max_dollars: None,
            expires_at: None,
            purpose: Some("test".to_string()),
            consumed_requests: None,
            consumed_dollars: None,
            denial_reason: None,
        },
    );
    crate::ledger::append(&cfg, &grant).unwrap();
    let with_grant = external_env_vars_for_work_item(
        &cfg,
        "test",
        &prof,
        Some("ISSUE-42"),
        Some(env_path.to_str().unwrap()),
    );
    assert!(with_grant.contains(&("PUBLIC_SETTING".to_string(), "keep".to_string())));
    assert!(with_grant.contains(&("ANOTHER_FLAG".to_string(), "42".to_string())));
    assert!(with_grant.contains(&("ODDS_API_KEY".to_string(), "secret-value".to_string())));
}
