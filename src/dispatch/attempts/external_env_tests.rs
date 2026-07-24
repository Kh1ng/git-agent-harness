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

#[test]
fn failed_backend_attempt_still_consumes_the_external_request_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.external_credential_scopes.insert(
        "odds".to_string(),
        ExternalCredentialScope {
            env_vars: vec!["ODDS_API_KEY".to_string()],
        },
    );
    let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
    let work_id = "ISSUE-FAILED-ATTEMPT";
    crate::ledger::append(
        &cfg,
        &LedgerEntry::new_external_approval(
            "test",
            &prof,
            work_id,
            "external_approval_grant",
            crate::ledger::ExternalApprovalRecord {
                state: Some("approved".to_string()),
                operation_kind: Some("historical_backfill".to_string()),
                credential_label: Some("odds".to_string()),
                allowed_env_vars: vec!["ODDS_API_KEY".to_string()],
                max_requests: Some(1),
                max_dollars: None,
                expires_at: None,
                purpose: Some("test".to_string()),
                consumed_requests: None,
                consumed_dollars: None,
                denial_reason: None,
            },
        ),
    )
    .unwrap();

    let mut dispatch = LedgerEntry::new(
        "test",
        &prof,
        "opencode",
        "fix",
        "failing attempt",
        None,
        None,
    );
    dispatch.work_id = Some(work_id.to_string());
    dispatch.attempts.push(crate::ledger::AttemptRecord {
        attempt_number: 1,
        backend: "opencode".to_string(),
        exit_code: Some(1),
        failure_class: Some("agent_failure".to_string()),
        ..crate::ledger::AttemptRecord::default()
    });

    record_external_approval_consumption_for_last_attempt(&cfg, "test", &prof, &dispatch);

    let entries = crate::ledger::read_entries(&cfg).unwrap();
    let snapshot = crate::ledger::external_approval_snapshot_from_entries(
        &entries,
        "test",
        &prof.repo_id,
        work_id,
        "odds",
        "historical_backfill",
    )
    .unwrap();
    assert_eq!(snapshot.consumed_requests, 1);
    assert!(!snapshot.active);
    assert_eq!(
        snapshot.denial_reason.as_deref(),
        Some("request cap reached")
    );
}
