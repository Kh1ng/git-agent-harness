//! Managed status: an issue that carries the managed label, or that a login
//! other than this loop's is assigned to, is a manager's. Autonomous intake
//! skips it and reports it as `managed`; an operator's explicit target does not
//! care, because that is how the manager runs the issue it holds.

use super::*;
use crate::config::IssueClaimMode;
use std::path::Path;

/// A fake `gh` that answers `api user` with `own_login` (or fails when it is
/// `None`) and every other `api` call with `issues_json`. Each `api user` call
/// also appends a line to `login_calls`, so a test can count lookups.
fn install_fake_gh(bin_dir: &Path, own_login: Option<&str>, issues_json: &str, login_calls: &Path) {
    fs::create_dir_all(bin_dir).unwrap();
    let login_branch = match own_login {
        Some(login) => format!("printf '%s\\n' '{login}'; exit 0"),
        None => "echo 'gh: not logged in' >&2; exit 1".to_string(),
    };
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"user\" ]; then\n  echo lookup >> '{}'\n  {login_branch}\nfi\nif [ \"$1\" = \"api\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
        login_calls.display(),
        issues_json.replace('\'', "'\\''")
    );
    let gh_path = bin_dir.join("gh");
    fs::write(&gh_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&gh_path, perms).unwrap();
    }
}

fn issue_json(number: u64, labels: &[&str], assignees: &[&str]) -> String {
    let labels = labels
        .iter()
        .map(|label| format!(r#"{{"name":"{label}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let assignees = assignees
        .iter()
        .map(|login| format!(r#"{{"login":"{login}","type":"User"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"number":{number},"title":"Issue {number}","body":"Goal: do it\n","labels":[{labels}],"assignees":[{assignees}],"author":{{"login":"owner","type":"User","is_bot":false}},"state":"OPEN"}}"#
    )
}

fn login_call_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|content| content.lines().count())
        .unwrap_or(0)
}

#[test]
fn managed_label_keeps_an_issue_out_of_autonomous_intake() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let calls = tmp.path().join("login-calls");
    let issues = format!(
        "[{},{}]",
        issue_json(7, &["managed"], &[]),
        issue_json(8, &["gah:Managed", "bug"], &[])
    );
    install_fake_gh(&tmp.path().join("bin"), Some("loop-bot"), &issues, &calls);
    let _guard = PathGuard::set(tmp.path().join("bin"));
    let prof = profile(tmp.path());

    let discovery = discover_open_issues(&prof);
    assert!(discovery.allowed.is_empty());
    assert_eq!(discovery.rejected.len(), 2);
    for rejection in &discovery.rejected {
        assert_eq!(rejection.reason_code, MANAGED_REASON_CODE);
        assert!(
            rejection.reason.contains("managed label"),
            "{}",
            rejection.reason
        );
    }
    assert_eq!(discovery.rejected[0].work_id.as_deref(), Some("#7"));

    let cfg = ticket_cfg(tmp.path());
    let candidates = scan_available_tickets(
        &prof,
        &[],
        &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
    );
    assert!(candidates.is_empty());
    assert_eq!(
        login_call_count(&calls),
        0,
        "no assignee, so no login lookup"
    );
}

#[test]
fn an_assignee_other_than_this_loop_marks_the_issue_managed() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let calls = tmp.path().join("login-calls");
    let issues = format!(
        "[{},{},{}]",
        issue_json(1, &[], &["colton"]),
        issue_json(2, &[], &["Loop-Bot"]),
        issue_json(3, &[], &[])
    );
    install_fake_gh(&tmp.path().join("bin"), Some("loop-bot"), &issues, &calls);
    let _guard = PathGuard::set(tmp.path().join("bin"));
    let prof = profile(tmp.path());

    let discovery = discover_open_issues(&prof);
    let allowed = discovery
        .allowed
        .iter()
        .map(|issue| issue.number.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        allowed,
        vec!["2", "3"],
        "own login (any case) and unassigned stay in"
    );
    assert_eq!(discovery.rejected.len(), 1);
    let rejection = &discovery.rejected[0];
    assert_eq!(rejection.work_id.as_deref(), Some("#1"));
    assert_eq!(rejection.reason_code, MANAGED_REASON_CODE);
    assert!(rejection.reason.contains("colton"), "{}", rejection.reason);
    assert_eq!(login_call_count(&calls), 1, "one lookup per discovery pass");
}

#[test]
fn an_unreadable_own_login_treats_every_assignee_as_someone_else() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let calls = tmp.path().join("login-calls");
    let issues = format!("[{}]", issue_json(4, &[], &["loop-bot"]));
    install_fake_gh(&tmp.path().join("bin"), None, &issues, &calls);
    let _guard = PathGuard::set(tmp.path().join("bin"));
    let prof = profile(tmp.path());

    let discovery = discover_open_issues(&prof);
    assert!(discovery.allowed.is_empty());
    assert_eq!(discovery.rejected.len(), 1);
    assert_eq!(discovery.rejected[0].reason_code, MANAGED_REASON_CODE);
    assert!(discovery.rejected[0].reason.contains("loop-bot"));
}

#[test]
fn managed_wins_over_the_other_label_dispositions() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let calls = tmp.path().join("login-calls");
    let issues = format!("[{}]", issue_json(5, &["blocked", "managed"], &[]));
    install_fake_gh(&tmp.path().join("bin"), Some("loop-bot"), &issues, &calls);
    let _guard = PathGuard::set(tmp.path().join("bin"));
    let prof = profile(tmp.path());

    let discovery = discover_open_issues(&prof);
    assert_eq!(discovery.rejected.len(), 1);
    assert_eq!(discovery.rejected[0].reason_code, MANAGED_REASON_CODE);
}

#[test]
fn an_explicit_target_fetch_does_not_apply_the_managed_check() {
    let tmp = tempfile::tempdir().unwrap();
    let prof = profile(tmp.path());
    let resp: serde_json::Value =
        serde_json::from_str(&issue_json(9, &["managed"], &["colton"])).unwrap();

    let details = issue_details_from_github_response(&prof, "9", &resp, false).unwrap();
    assert_eq!(details.number, "9");
    assert_eq!(details.labels, vec!["managed".to_string()]);
}

#[test]
fn the_managed_label_also_blocks_the_ticket_scan_filter() {
    assert!(issue_is_auto_dispatch_blocked(&["gah:managed".to_string()]));
    assert!(issue_is_auto_dispatch_blocked(&[" Managed ".to_string()]));
    assert!(!issue_is_auto_dispatch_blocked(&["unmanaged".to_string()]));
    assert!(issue_label_is_managed("managed"));
    assert!(!issue_label_is_managed("managed-by-bot"));
}

#[test]
fn with_github_assignee_claims_on_only_the_label_marks_an_issue_managed() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let calls = tmp.path().join("login-calls");
    let issues = format!(
        "[{},{}]",
        issue_json(10, &["managed"], &[]),
        issue_json(11, &[], &["colton"])
    );
    install_fake_gh(&tmp.path().join("bin"), Some("loop-bot"), &issues, &calls);
    let _guard = PathGuard::set(tmp.path().join("bin"));
    let mut prof = profile(tmp.path());
    prof.publishing.issue_claim.mode = IssueClaimMode::GithubAssignee;

    // The assignee of #11 is a claim for `issue_claim` to arbitrate, not a
    // managed hold; that module reads the claim comments itself (here the
    // fake answers the comment lookup with the issue list, which parses as
    // no claims and so leaves #11 held by colton as `claimed_elsewhere`).
    let discovery = try_discover_open_issues(&prof).unwrap();
    assert_eq!(discovery.rejected.len(), 1);
    assert_eq!(discovery.rejected[0].work_id.as_deref(), Some("#10"));
    assert_eq!(discovery.rejected[0].reason_code, MANAGED_REASON_CODE);
    assert_eq!(
        discovery
            .allowed
            .iter()
            .map(|issue| issue.number.as_str())
            .collect::<Vec<_>>(),
        vec!["11"]
    );
}
