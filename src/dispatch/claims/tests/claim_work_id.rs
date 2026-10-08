use super::*;

/// Issue #1466: the claim is taken under the id the target file names, the
/// run records its terminal entry under the branch it resolved. Both carry
/// the dispatch's session directory, which is what ends the claim.
fn fixture(tmp: &Path) -> (GahConfig, Profile, DispatchArgs) {
    let bin_dir = tmp.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_fake_gh(&bin_dir, "[]");
    super::repository_scope::claim_fixture(tmp)
}

const NON_AUTHORITATIVE_TICKET: &str = "# Fix the thing\n\nGoal: test claim guard\n";

/// The terminal entry of a run that resolved its identity to the dispatch
/// branch: work id = branch, failed after the workflow ran.
fn run_entry(prof: &Profile, args: &DispatchArgs, session_dir: &Path) -> LedgerEntry {
    let mut entry = LedgerEntry::new(
        "test",
        prof,
        "codex",
        "improve",
        &args.target,
        Some("run-1".into()),
        Some(session_dir),
    );
    entry.work_id = Some("gah/repo-1700000000-abc123".into());
    entry.branch = entry.work_id.clone();
    entry.set_failure(
        crate::ledger::FailureClass::AgentNoProgress,
        crate::ledger::FailureStage::PostValidation,
    );
    entry
}

fn write_candidate_file(tmp: &Path) -> std::path::PathBuf {
    let dir = tmp.join("candidates");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("latest.json");
    let artifact = serde_json::json!({
        "counts": {"seen": 1, "converted": 1, "skipped_warning": 0},
        "candidates": [{
            "candidate_id": "cand-500",
            "source_gate_status": "pass",
            "suggested_blueprint_phase": "now",
            "provider_mutation_allowed": false,
            "suggested_labels": [],
            "affected_files": [],
            "evidence": [],
            "acceptance_criteria": [],
            "verification": [],
            "hydration_used": false,
            "hydration_source": "",
            "hydration_match_method": "",
            "hydrated_fields": [],
            "debug_gate_keys": [],
            "debug_scout_keys": [],
            "debug_hydrated_keys": [],
            "debug_hydrated_finding_excerpt": ""
        }]
    });
    fs::write(&path, serde_json::to_string(&artifact).unwrap()).unwrap();
    path
}

/// Claim `expected_id` for a run in `sessions/run-1`, record that run's
/// terminal entry under its branch in `run_session_dir` (when given), and
/// return whether a second dispatch of the same target is refused.
fn second_dispatch_refused(
    cfg: &GahConfig,
    prof: &Profile,
    args: &DispatchArgs,
    tmp: &Path,
    expected_id: &str,
    run_session_dir: Option<&Path>,
) -> bool {
    let session_dir = tmp.join("sessions").join("run-1");
    let (_, claimed) = acquire_claim(cfg, prof, args, &session_dir).unwrap();
    assert_eq!(claimed.as_deref(), Some(expected_id));

    if let Some(run_session_dir) = run_session_dir {
        let mut entry = run_entry(prof, args, run_session_dir);
        // #1461 leaves a work id the workflow already set alone.
        stamp_claimed_work_id(&mut entry, claimed);
        assert_ne!(entry.work_id.as_deref(), Some(expected_id));
        crate::ledger::append(cfg, &entry).unwrap();
    }

    match check_duplicate_work(cfg, prof, args, false) {
        Ok(work_id) => {
            assert_eq!(work_id.as_deref(), Some(expected_id));
            false
        }
        Err(error) => {
            assert!(error.downcast_ref::<ActiveClaimError>().is_some());
            true
        }
    }
}

#[test]
fn non_authoritative_ticket_claim_ends_when_its_run_records_under_the_branch() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, prof, args) = fixture(tmp.path());
    let _guard = PathGuard::set(tmp.path().join("bin"));
    // The filename names TICKET-500; the heading does not.
    fs::write(&args.target, NON_AUTHORITATIVE_TICKET).unwrap();

    let session_dir = tmp.path().join("sessions").join("run-1");
    assert!(!second_dispatch_refused(
        &cfg,
        &prof,
        &args,
        tmp.path(),
        "TICKET-500",
        Some(&session_dir),
    ));
}

#[test]
fn candidate_file_claim_ends_when_its_run_records_under_the_branch() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, prof, mut args) = fixture(tmp.path());
    let _guard = PathGuard::set(tmp.path().join("bin"));
    args.target = write_candidate_file(tmp.path()).display().to_string();

    let session_dir = tmp.path().join("sessions").join("run-1");
    assert!(!second_dispatch_refused(
        &cfg,
        &prof,
        &args,
        tmp.path(),
        "cand-500",
        Some(&session_dir),
    ));
}

#[test]
fn claim_without_an_ending_entry_is_still_refused() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, prof, args) = fixture(tmp.path());
    let _guard = PathGuard::set(tmp.path().join("bin"));
    fs::write(&args.target, NON_AUTHORITATIVE_TICKET).unwrap();

    assert!(second_dispatch_refused(
        &cfg,
        &prof,
        &args,
        tmp.path(),
        "TICKET-500",
        None,
    ));
}

#[test]
fn another_runs_entry_does_not_end_the_claim() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, prof, args) = fixture(tmp.path());
    let _guard = PathGuard::set(tmp.path().join("bin"));
    fs::write(&args.target, NON_AUTHORITATIVE_TICKET).unwrap();

    // A different dispatch's terminal entry, recorded under a branch in
    // another session directory, says nothing about this claim's run.
    let other_session_dir = tmp.path().join("sessions").join("run-0");
    assert!(second_dispatch_refused(
        &cfg,
        &prof,
        &args,
        tmp.path(),
        "TICKET-500",
        Some(&other_session_dir),
    ));
}

#[test]
fn claim_without_a_session_directory_stays_active() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, prof, args) = fixture(tmp.path());
    let _guard = PathGuard::set(tmp.path().join("bin"));
    fs::write(&args.target, NON_AUTHORITATIVE_TICKET).unwrap();

    // A claim written before claims recorded their session directory.
    let claim = LedgerEntry::new_claim("test", &prof, "TICKET-500");
    assert!(claim.session_dir.is_none());
    crate::ledger::append(&cfg, &claim).unwrap();
    let session_dir = tmp.path().join("sessions").join("run-1");
    crate::ledger::append(&cfg, &run_entry(&prof, &args, &session_dir)).unwrap();

    let again = check_duplicate_work(&cfg, &prof, &args, false);
    assert!(again
        .unwrap_err()
        .downcast_ref::<ActiveClaimError>()
        .is_some());
}

/// Discovery (`gah loop`, `gah status`) applies the same rule: a claim
/// whose run recorded its terminal entry under the branch no longer marks
/// the ticket claimed, and a claim with no ending entry still does.
#[test]
fn discovery_does_not_report_a_claim_whose_run_ended_under_the_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let ticket_dir = tmp.path().join("docs/tickets");
    fs::create_dir_all(&ticket_dir).unwrap();
    fs::write(
        ticket_dir.join("TICKET-500-test.md"),
        "# TICKET-500: Test\n\nGoal: test claim guard\n",
    )
    .unwrap();
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = String::new();

    let session_dir = tmp.path().join("sessions").join("run-1");
    let mut claim = LedgerEntry::new_claim("test", &prof, "TICKET-500");
    claim.session_dir = Some(session_dir.display().to_string());
    let claimed = |entries: &[LedgerEntry]| {
        let index = crate::ledger::index_entries_by_work_id(entries);
        let scan = scan_available_tickets_with_dependencies(&prof, &[], &index, entries);
        assert_eq!(scan.available_tickets.len(), 1);
        scan.available_tickets[0].has_active_claim
    };

    assert!(claimed(std::slice::from_ref(&claim)));

    let mut ended = LedgerEntry::new(
        "test",
        &prof,
        "codex",
        "improve",
        "TICKET-500-test.md",
        Some("run-1".into()),
        Some(&session_dir),
    );
    ended.work_id = Some("gah/repo-1700000000-abc123".into());
    ended.branch = ended.work_id.clone();
    ended.set_failure(
        crate::ledger::FailureClass::AgentNoProgress,
        crate::ledger::FailureStage::PostValidation,
    );
    assert!(!claimed(&[claim.clone(), ended.clone()]));

    // Another run's entry says nothing about this claim.
    ended.session_dir = Some(tmp.path().join("sessions/run-0").display().to_string());
    assert!(claimed(&[claim, ended]));
}
