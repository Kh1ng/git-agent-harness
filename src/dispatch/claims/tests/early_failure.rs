use super::*;

fn early_failure_fixture(tmp: &std::path::Path) -> (GahConfig, Profile, DispatchArgs) {
    let bin_dir = tmp.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_fake_gh(&bin_dir, "[]");
    super::repository_scope::claim_fixture(tmp)
}

/// The terminal entry a direct dispatch writes when it fails before the
/// workflow has resolved a work identity: mode `fix`, no work id, the
/// session directory `run()` gave the dispatch.
fn early_failure_entry(prof: &Profile, session_dir: &Path) -> LedgerEntry {
    let mut entry = LedgerEntry::new(
        "test",
        prof,
        "codex",
        "fix",
        "TICKET-500.md",
        None,
        Some(session_dir),
    );
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

    let session_dir = tmp.path().join("sessions/run");
    let (_, claimed) = acquire_claim(&cfg, &prof, &args, &session_dir).unwrap();
    assert_eq!(claimed.as_deref(), Some("TICKET-500"));

    let mut entry = early_failure_entry(&prof, &session_dir);
    stamp_claimed_work_id(&mut entry, claimed);
    assert_eq!(entry.work_id.as_deref(), Some("TICKET-500"));
    crate::ledger::append(&cfg, &entry).unwrap();

    let again = check_duplicate_work(&cfg, &prof, &args, false);
    assert_eq!(again.unwrap().as_deref(), Some("TICKET-500"));
}

/// Issue #1466: even without the stamped work id, the terminal entry shares
/// the claim's session directory, which is enough to end the claim. The
/// legacy shape (a claim with no session directory) is covered by
/// `claim_work_id::claim_without_a_session_directory_stays_active`.
#[test]
fn early_failure_without_work_id_releases_the_claim_through_its_session() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, prof, args) = early_failure_fixture(tmp.path());
    let _guard = PathGuard::set(tmp.path().join("bin"));

    let session_dir = tmp.path().join("sessions/run");
    acquire_claim(&cfg, &prof, &args, &session_dir).unwrap();
    crate::ledger::append(&cfg, &early_failure_entry(&prof, &session_dir)).unwrap();

    let again = check_duplicate_work(&cfg, &prof, &args, false);
    assert_eq!(again.unwrap().as_deref(), Some("TICKET-500"));
}

#[test]
fn stamp_keeps_a_work_id_the_workflow_already_set() {
    let tmp = tempfile::tempdir().unwrap();
    let (_cfg, prof, _args) = super::repository_scope::claim_fixture(tmp.path());
    let mut entry = early_failure_entry(&prof, &tmp.path().join("sessions/run"));
    entry.work_id = Some("TICKET-777".into());
    stamp_claimed_work_id(&mut entry, Some("TICKET-500".into()));
    assert_eq!(entry.work_id.as_deref(), Some("TICKET-777"));
}
