use super::*;

fn early_failure_fixture(tmp: &std::path::Path) -> (GahConfig, Profile, DispatchArgs) {
    let bin_dir = tmp.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_fake_gh(&bin_dir, "[]");
    super::repository_scope::claim_fixture(tmp)
}

/// The terminal entry a direct dispatch writes when it fails before the
/// workflow has resolved a work identity: mode `fix`, no work id.
fn early_failure_entry(prof: &Profile) -> LedgerEntry {
    let mut entry = LedgerEntry::new("test", prof, "codex", "fix", "TICKET-500.md", None, None);
    entry.failure_class = Some("route_unavailable".into());
    assert!(entry.work_id.is_none());
    entry
}

#[test]
fn early_failure_with_claimed_work_id_releases_the_claim() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, prof, args) = early_failure_fixture(tmp.path());
    let _guard = PathGuard::set(tmp.path().join("bin"));

    let (_, claimed) = acquire_claim(&cfg, &prof, &args).unwrap();
    assert_eq!(claimed.as_deref(), Some("TICKET-500"));

    let mut entry = early_failure_entry(&prof);
    stamp_claimed_work_id(&mut entry, claimed);
    assert_eq!(entry.work_id.as_deref(), Some("TICKET-500"));
    crate::ledger::append(&cfg, &entry).unwrap();

    let again = check_duplicate_work(&cfg, &prof, &args, false);
    assert_eq!(again.unwrap().as_deref(), Some("TICKET-500"));
}

#[test]
fn early_failure_without_work_id_leaves_the_claim_active() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, prof, args) = early_failure_fixture(tmp.path());
    let _guard = PathGuard::set(tmp.path().join("bin"));

    acquire_claim(&cfg, &prof, &args).unwrap();
    crate::ledger::append(&cfg, &early_failure_entry(&prof)).unwrap();

    let again = check_duplicate_work(&cfg, &prof, &args, false);
    assert!(again
        .unwrap_err()
        .downcast_ref::<ActiveClaimError>()
        .is_some());
}

#[test]
fn stamp_keeps_a_work_id_the_workflow_already_set() {
    let tmp = tempfile::tempdir().unwrap();
    let (_cfg, prof, _args) = super::repository_scope::claim_fixture(tmp.path());
    let mut entry = early_failure_entry(&prof);
    entry.work_id = Some("TICKET-777".into());
    stamp_claimed_work_id(&mut entry, Some("TICKET-500".into()));
    assert_eq!(entry.work_id.as_deref(), Some("TICKET-777"));
}
