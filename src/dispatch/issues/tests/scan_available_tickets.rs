use super::*;

#[test]
fn scan_available_tickets_excludes_issue_already_archived_locally() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\n","labels":[],"author":{"login":"owner"}}]"#;
    let gh_path = bin_dir.join("gh");
    fs::write(
        &gh_path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"api\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
            issue_json.replace('\'', "'\\''")
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_path, perms).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);

    let closed_dir = tmp.path().join("docs/tickets/closed");
    fs::create_dir_all(&closed_dir).unwrap();
    fs::write(
        closed_dir.join("TICKET-101-fail-closed-version-drift.md"),
        "# TICKET-101: Fail closed\n\nGoal: test\n",
    )
    .unwrap();

    let cfg = ticket_cfg(tmp.path());
    let mut prof = profile(tmp.path());
    prof.local_path = tmp.path().display().to_string();
    prof.provider = "github".to_string();
    prof.repo = "owner/repo".to_string();

    let candidates = scan_available_tickets(
        &prof,
        &[],
        &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
    );
    assert!(
        candidates.is_empty(),
        "expected locally-archived TICKET-101 issue to be excluded, got {candidates:?}"
    );
}
