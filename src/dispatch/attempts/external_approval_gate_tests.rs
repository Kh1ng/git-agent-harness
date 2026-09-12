use super::*;
use crate::config::ExternalCredentialScope;
use crate::config::RoutingPolicy;
use crate::dispatch::attempts::external_approval_gap::external_approval_gaps_for_work_item;
use crate::dispatch::external_approval_pause::{
    notify_external_approval_request, raise_external_approval_request,
};
use crate::dispatch::test_util::{gah_config_with_ledger, profile};
use std::fs;
use std::process::Command;

/// Issue #653 hermetic scenario: the profile declares an external credential
/// scope, the operator's env file carries the credential, and no grant is
/// active. The launch must pause with the typed error — before the backend
/// process is spawned — and raise exactly one request. After a grant, the
/// same launch proceeds and injects the credential.
#[test]
fn unapproved_external_credential_pauses_launch_and_grant_resumes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    let notifications = tmp.path().join("notifications.txt");
    prof.notify_command = Some(format!("cat >> {}", notifications.display()));
    prof.external_credential_scopes.insert(
        "odds".to_string(),
        ExternalCredentialScope {
            env_vars: vec!["ODDS_API_KEY".to_string()],
            max_requests: Some(1),
            max_dollars: None,
            purpose: Some("odds feed".to_string()),
        },
    );
    let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
    let env_path = tmp.path().join("task.env");
    fs::write(
        &env_path,
        "PUBLIC_SETTING=keep\nODDS_API_KEY=secret-value\n",
    )
    .unwrap();
    let work_id = "#653";

    // 1. Pause: the gap is detected, bounded by the declared scope.
    let gaps = external_approval_gaps_for_work_item(
        &cfg,
        "test",
        &prof,
        Some(work_id),
        Some(env_path.to_str().unwrap()),
    );
    assert_eq!(gaps.len(), 1, "one unapproved scope must pause");
    assert_eq!(gaps[0].label, "odds");
    assert_eq!(gaps[0].env_vars, vec!["ODDS_API_KEY".to_string()]);

    // 2. The request is raised once; a second raise is deduped.
    let mut ledger =
        crate::ledger::LedgerEntry::new("session", &prof, "codex", "improve", "target", None, None);
    ledger.work_id.clone_from(&Some(work_id.to_string()));
    let mut cfg = cfg;
    cfg.profiles.insert("test".to_string(), prof.clone());
    if raise_external_approval_request(&cfg, "test", &prof, &ledger, &gaps[0]) {
        notify_external_approval_request(&cfg, "test", &prof, &ledger, &gaps[0]);
    }
    if raise_external_approval_request(&cfg, "test", &prof, &ledger, &gaps[0]) {
        notify_external_approval_request(&cfg, "test", &prof, &ledger, &gaps[0]);
    }

    // Exactly one pending request is visible in the ledger.
    let entries = crate::ledger::read_entries(&cfg).unwrap();
    let requests = entries
        .iter()
        .filter(|row| row.mode == "external_approval_request")
        .count();
    assert_eq!(requests, 1, "duplicate requests must be deduped");
    assert_eq!(
        fs::read_to_string(&notifications).unwrap().lines().count(),
        1,
        "duplicate requests must emit one notification"
    );

    // 3. Granting releases the hold: the effective gate resolves.
    let grant_entry = crate::ledger::LedgerEntry::new_external_approval(
        "test",
        &prof,
        work_id,
        "external_approval_grant",
        crate::ledger::ExternalApprovalRecord {
            state: Some("approved".to_string()),
            operation_kind: Some("env_credential".to_string()),
            credential_label: Some("odds".to_string()),
            allowed_env_vars: vec!["ODDS_API_KEY".to_string()],
            max_requests: Some(1),
            max_dollars: None,
            expires_at: None,
            purpose: Some("odds feed".to_string()),
            consumed_requests: Some(0),
            consumed_dollars: None,
            denial_reason: None,
        },
    );
    crate::ledger::append_external_approval(&cfg, grant_entry).unwrap();

    let entries = crate::ledger::read_entries(&cfg).unwrap();
    let snapshot = crate::ledger::external_approval_snapshot_from_entries(
        &entries,
        "test",
        &prof.repo_id,
        work_id,
        "odds",
        "env_credential",
    )
    .expect("grant must be visible");
    assert_eq!(snapshot.state, "approved");
    assert!(snapshot.active);

    // 4. The gap is gone: launch proceeds and the credential is injected.
    let gaps_after = external_approval_gaps_for_work_item(
        &cfg,
        "test",
        &prof,
        Some(work_id),
        Some(env_path.to_str().unwrap()),
    );
    assert!(gaps_after.is_empty(), "grant must clear the gap");
    let injected = external_env_vars_for_work_item(
        &cfg,
        "test",
        &prof,
        Some(work_id),
        Some(env_path.to_str().unwrap()),
    );
    assert!(
        injected.iter().any(|(key, _)| key == "ODDS_API_KEY"),
        "granted credential must be injected: {injected:?}"
    );

    // 5. One successful use exhausts the request cap and re-holds without
    // raising a duplicate request after a restart-style ledger reload.
    crate::ledger::record_external_approval_consumption_for_work_item(
        &cfg,
        "test",
        &prof,
        Some(work_id),
        &crate::ledger::LedgerUsage::default(),
    )
    .unwrap();
    let entries = crate::ledger::read_entries(&cfg).unwrap();
    let exhausted = crate::ledger::external_approval_snapshot_from_entries(
        &entries,
        "test",
        &prof.repo_id,
        work_id,
        "odds",
        "env_credential",
    )
    .unwrap();
    assert_eq!(
        exhausted.denial_reason.as_deref(),
        Some("request cap reached")
    );
    assert!(!exhausted.active);
    assert_eq!(
        external_approval_gaps_for_work_item(
            &cfg,
            "test",
            &prof,
            Some(work_id),
            Some(env_path.to_str().unwrap()),
        )
        .len(),
        1,
        "cap exhaustion must re-hold"
    );
    assert!(!raise_external_approval_request(
        &cfg, "test", &prof, &ledger, &gaps[0]
    ));
    assert_eq!(
        fs::read_to_string(&notifications).unwrap().lines().count(),
        2,
        "cap exhaustion must emit one terminal resolution"
    );

    let _ = Command::new("true");
}
