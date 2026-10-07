use super::*;

fn assert_claim_after_records(modes: &[&str], expected_active: bool) {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_fake_gh(&bin_dir, "[]");
    let _guard = PathGuard::set(&bin_dir);
    let (cfg, prof, args) = super::repository_scope::claim_fixture(tmp.path());
    let mut entries = vec![LedgerEntry::new_claim("test", &prof, "TICKET-500")];
    for mode in modes {
        let mut entry = LedgerEntry::new("test", &prof, "codex", mode, "x", None, None);
        entry.work_id = Some("TICKET-500".into());
        entries.push(entry);
    }
    let lines: String = entries
        .iter()
        .map(|entry| format!("{}\n", serde_json::to_string(entry).unwrap()))
        .collect();
    fs::write(tmp.path().join("ledger.jsonl"), lines).unwrap();
    let result = check_duplicate_work(&cfg, &prof, &args, false);
    if expected_active {
        assert!(result
            .unwrap_err()
            .downcast_ref::<ActiveClaimError>()
            .is_some());
    } else {
        assert_eq!(result.unwrap().as_deref(), Some("TICKET-500"));
    }
    let index = crate::ledger::index_entries_by_work_id(&entries);
    let history = ledger_lookup_for_ticket(Some("TICKET-500"), &prof, &[], &index).unwrap();
    assert_eq!(history.5, expected_active);
    assert_eq!(
        history.0, 0,
        "control records and resets consume no attempts"
    );
    assert_eq!(history.1, 0);
}

#[test]
fn external_approval_request_keeps_claim_active() {
    assert_claim_after_records(&["external_approval_request"], true);
}

#[test]
fn paid_route_approval_grant_keeps_claim_active() {
    assert_claim_after_records(&["paid_route_approval_grant"], true);
}

#[test]
fn review_hold_and_release_keep_claim_active() {
    assert_claim_after_records(&["review_hold", "review_hold_release"], true);
}

#[test]
fn clear_attempts_resolves_claim() {
    assert_claim_after_records(&["clear_attempts"], false);
}

#[test]
fn control_record_modes_are_complete() {
    for mode in [
        "paid_route_approval_grant",
        "paid_route_approval_revoke",
        "external_approval_grant",
        "external_approval_request",
        "external_approval_consume",
        "external_approval_revoke",
        "external_approval_expire",
        "external_approval_deny",
        "review_hold",
        "review_hold_release",
    ] {
        assert!(is_control_record(mode), "{mode}");
    }
    for mode in ["claim", "fix", "review", "clear_attempts"] {
        assert!(!is_control_record(mode), "{mode}");
    }
}
