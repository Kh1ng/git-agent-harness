use super::*;
use crate::dispatch::test_util::profile;
use crate::test_support::{ExecGuard, PathGuard};
use std::cell::RefCell;
use std::rc::Rc;

const MICHAEL: &str = "Ram-Rod6198";
const COLTON: &str = "Kh1ng";

fn policy(priority_logins: &[&str]) -> IssueClaimPolicy {
    IssueClaimPolicy {
        mode: IssueClaimMode::GithubAssignee,
        priority_logins: priority_logins.iter().map(|l| l.to_string()).collect(),
        ..Default::default()
    }
}

fn at(minutes: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::minutes(minutes)
}

fn claim(id: u64, login: &str, minutes: i64) -> ClaimComment {
    ClaimComment {
        id,
        login: login.to_string(),
        created_at: at(minutes),
        renewed_at: at(minutes),
    }
}

fn view(assignees: &[&str], claims: Vec<ClaimComment>) -> IssueClaimView {
    IssueClaimView {
        assignees: assignees.iter().map(|l| l.to_string()).collect(),
        claims,
    }
}

/// One issue shared by every loop in a test.
#[derive(Default)]
struct Issue {
    view: IssueClaimView,
    next_comment_id: u64,
    open_pull_request: bool,
    comments_fail: bool,
}

/// One loop's handle on the shared issue, acting under its own login.
struct LoopBoard {
    login: &'static str,
    issue: Rc<RefCell<Issue>>,
    now: OffsetDateTime,
}

impl IssueBoard for LoopBoard {
    fn view(&self, _issue: &str) -> Result<IssueClaimView> {
        Ok(self.issue.borrow().view.clone())
    }

    fn assign(&self, _issue: &str, login: &str) -> Result<()> {
        let assignees = &mut self.issue.borrow_mut().view.assignees;
        if !assignees.iter().any(|assigned| assigned == login) {
            assignees.push(login.to_string());
        }
        Ok(())
    }

    fn unassign(&self, _issue: &str, logins: &[String]) -> Result<()> {
        self.issue
            .borrow_mut()
            .view
            .assignees
            .retain(|assigned| !logins.contains(assigned));
        Ok(())
    }

    fn post_claim(&self, _issue: &str, _body: &str) -> Result<()> {
        let mut issue = self.issue.borrow_mut();
        if issue.comments_fail {
            anyhow::bail!("comment refused");
        }
        issue.next_comment_id += 1;
        let id = issue.next_comment_id;
        issue.view.claims.push(ClaimComment {
            id,
            login: self.login.to_string(),
            created_at: self.now,
            renewed_at: self.now,
        });
        Ok(())
    }

    fn renew_claim(&self, comment_id: u64, _body: &str) -> Result<()> {
        let mut issue = self.issue.borrow_mut();
        let comment = issue
            .view
            .claims
            .iter_mut()
            .find(|claim| claim.id == comment_id)
            .context("no such comment")?;
        comment.renewed_at = self.now;
        Ok(())
    }

    fn has_open_pull_request(&self, _issue: &str) -> Result<bool> {
        Ok(self.issue.borrow().open_pull_request)
    }
}

fn two_loops(issue: &Rc<RefCell<Issue>>, minutes: i64) -> (LoopBoard, LoopBoard) {
    let board = |login| LoopBoard {
        login,
        issue: Rc::clone(issue),
        now: at(minutes),
    };
    (board(MICHAEL), board(COLTON))
}

/// Colton stakes first, but Michael reads the issue before Colton's
/// assignment is visible, so both post a claim.
fn stake_both_at_once(
    issue: &Rc<RefCell<Issue>>,
    michael: &LoopBoard,
    colton: &LoopBoard,
    policy: &IssueClaimPolicy,
) {
    assert_eq!(stake(colton, policy, COLTON, "7", at(0)).unwrap(), None);
    issue.borrow_mut().view.assignees.clear();
    assert_eq!(stake(michael, policy, MICHAEL, "7", at(0)).unwrap(), None);
    issue.borrow_mut().view.assignees.push(COLTON.into());
}

fn assignees(issue: &Rc<RefCell<Issue>>) -> Vec<String> {
    issue.borrow().view.assignees.clone()
}

#[test]
fn the_earliest_claim_comment_wins_a_contested_claim() {
    let contested = view(
        &[MICHAEL, COLTON],
        vec![claim(11, COLTON, 0), claim(12, MICHAEL, 0)],
    );
    let policy = policy(&[]);

    assert_eq!(
        standing(&contested, MICHAEL, &policy, at(1)),
        Standing::Theirs(COLTON.into())
    );
    assert_eq!(standing(&contested, COLTON, &policy, at(1)), Standing::Mine);
}

#[test]
fn a_priority_login_wins_over_an_earlier_claim_comment() {
    let contested = view(
        &[COLTON, MICHAEL],
        vec![claim(11, COLTON, 0), claim(12, MICHAEL, 0)],
    );
    let policy = policy(&["ram-rod6198"]);

    assert_eq!(
        standing(&contested, MICHAEL, &policy, at(1)),
        Standing::Mine
    );
    assert_eq!(
        standing(&contested, COLTON, &policy, at(1)),
        Standing::Theirs(MICHAEL.into())
    );
}

#[test]
fn priority_does_not_take_an_issue_from_a_login_already_holding_it() {
    let held = view(&[COLTON], vec![claim(11, COLTON, 0)]);

    assert_eq!(
        standing(&held, MICHAEL, &policy(&[MICHAEL]), at(30)),
        Standing::Theirs(COLTON.into())
    );
}

#[test]
fn a_claim_holds_until_its_ttl_and_not_a_minute_longer() {
    let held = view(&[COLTON], vec![claim(11, COLTON, 0)]);
    let policy = policy(&[]);

    assert_eq!(
        standing(&held, MICHAEL, &policy, at(59)),
        Standing::Theirs(COLTON.into())
    );
    assert_eq!(
        standing(&held, MICHAEL, &policy, at(60)),
        Standing::Stale(vec![COLTON.into()])
    );
    // An unrenewed claim lapses in the holder's own view too.
    assert_eq!(
        standing(&held, COLTON, &policy, at(60)),
        Standing::Unclaimed
    );
}

#[test]
fn only_the_latest_claim_of_a_login_counts() {
    let reclaimed = view(&[COLTON], vec![claim(11, COLTON, 0), claim(40, COLTON, 90)]);

    assert_eq!(
        standing(&reclaimed, MICHAEL, &policy(&[]), at(100)),
        Standing::Theirs(COLTON.into())
    );
}

#[test]
fn a_hand_assigned_issue_is_held_without_any_claim_comment() {
    let by_hand = view(&[COLTON], vec![]);

    assert_eq!(
        standing(&by_hand, MICHAEL, &policy(&[MICHAEL]), at(100_000)),
        Standing::Theirs(COLTON.into())
    );
    // Assigned to this loop's own login, it is this loop's to claim.
    assert_eq!(
        standing(&by_hand, COLTON, &policy(&[]), at(0)),
        Standing::Unclaimed
    );
}

#[test]
fn a_claim_comment_from_a_login_that_is_not_assigned_holds_nothing() {
    let withdrawn = view(&[], vec![claim(11, COLTON, 0)]);

    assert_eq!(
        standing(&withdrawn, MICHAEL, &policy(&[]), at(1)),
        Standing::Unclaimed
    );
}

#[test]
fn two_loops_racing_for_one_issue_leave_exactly_one_holder() {
    let issue = Rc::new(RefCell::new(Issue::default()));
    let (michael, colton) = two_loops(&issue, 0);
    let policy = policy(&[]);

    stake_both_at_once(&issue, &michael, &colton, &policy);

    assert_eq!(
        settle(&michael, &policy, MICHAEL, "7", at(1)).unwrap(),
        ClaimOutcome::Lost(format!("yielded to {COLTON}"))
    );
    assert_eq!(
        settle(&colton, &policy, COLTON, "7", at(1)).unwrap(),
        ClaimOutcome::Won
    );
    assert_eq!(assignees(&issue), [COLTON]);
}

#[test]
fn the_priority_login_wins_the_race_whoever_commented_first() {
    let issue = Rc::new(RefCell::new(Issue::default()));
    let (michael, colton) = two_loops(&issue, 0);
    let policy = policy(&[MICHAEL]);

    stake_both_at_once(&issue, &michael, &colton, &policy);

    assert_eq!(
        settle(&colton, &policy, COLTON, "7", at(1)).unwrap(),
        ClaimOutcome::Lost(format!("yielded to {MICHAEL}"))
    );
    assert_eq!(
        settle(&michael, &policy, MICHAEL, "7", at(1)).unwrap(),
        ClaimOutcome::Won
    );
    assert_eq!(assignees(&issue), [MICHAEL]);
}

#[test]
fn a_held_issue_is_left_untouched() {
    let issue = Rc::new(RefCell::new(Issue::default()));
    let (michael, colton) = two_loops(&issue, 0);
    let policy = policy(&[MICHAEL]);
    assert_eq!(stake(&colton, &policy, COLTON, "7", at(0)).unwrap(), None);

    assert_eq!(
        stake(&michael, &policy, MICHAEL, "7", at(5)).unwrap(),
        Some(ClaimOutcome::Lost(format!("held by {COLTON}")))
    );
    assert_eq!(assignees(&issue), [COLTON]);
    assert_eq!(issue.borrow().view.claims.len(), 1);
}

#[test]
fn an_expired_claim_is_taken_over_by_removing_the_stale_assignee() {
    let issue = Rc::new(RefCell::new(Issue::default()));
    let (michael, colton) = two_loops(&issue, 61);
    let policy = policy(&[]);
    issue.borrow_mut().view = view(&[COLTON], vec![claim(1, COLTON, 0)]);
    issue.borrow_mut().next_comment_id = 1;

    assert_eq!(
        stake(&michael, &policy, MICHAEL, "7", at(61)).unwrap(),
        None
    );
    assert_eq!(assignees(&issue), [MICHAEL]);
    assert_eq!(
        settle(&michael, &policy, MICHAEL, "7", at(62)).unwrap(),
        ClaimOutcome::Won
    );
    // The previous holder now sees a live claim that is not its own.
    assert_eq!(
        stake(&colton, &policy, COLTON, "7", at(62)).unwrap(),
        Some(ClaimOutcome::Lost(format!("held by {MICHAEL}")))
    );
}

#[test]
fn an_open_pull_request_keeps_an_expired_claim() {
    let issue = Rc::new(RefCell::new(Issue {
        view: view(&[COLTON], vec![claim(1, COLTON, 0)]),
        open_pull_request: true,
        ..Default::default()
    }));
    let (michael, _) = two_loops(&issue, 600);

    let outcome = stake(&michael, &policy(&[MICHAEL]), MICHAEL, "7", at(600)).unwrap();

    assert!(
        matches!(outcome, Some(ClaimOutcome::Lost(reason)) if reason.contains("open pull request"))
    );
    assert_eq!(assignees(&issue), [COLTON]);
}

#[test]
fn a_live_claim_of_this_loop_is_reused_without_a_new_comment() {
    let issue = Rc::new(RefCell::new(Issue {
        view: view(&[MICHAEL], vec![claim(1, MICHAEL, 0)]),
        ..Default::default()
    }));
    let (michael, _) = two_loops(&issue, 30);

    assert_eq!(
        stake(&michael, &policy(&[]), MICHAEL, "7", at(30)).unwrap(),
        Some(ClaimOutcome::Won)
    );
    assert_eq!(issue.borrow().view.claims.len(), 1);
}

#[test]
fn a_renewed_claim_outlives_its_ttl_and_lapses_a_ttl_after_its_last_renewal() {
    let issue = Rc::new(RefCell::new(Issue {
        view: view(&[COLTON], vec![claim(1, COLTON, 0)]),
        ..Default::default()
    }));
    let policy = policy(&[]);
    let (_, colton) = two_loops(&issue, 40);

    assert_eq!(renew(&colton, &policy, COLTON, "7", at(40)).unwrap(), None);

    let seen = issue.borrow().view.clone();
    assert_eq!(
        standing(&seen, MICHAEL, &policy, at(99)),
        Standing::Theirs(COLTON.into())
    );
    assert_eq!(
        standing(&seen, MICHAEL, &policy, at(100)),
        Standing::Stale(vec![COLTON.into()])
    );
    // Renewal keeps the claim's place in a contest: its comment id is unchanged.
    assert_eq!(seen.claims[0].id, 1);
}

#[test]
fn a_lease_taken_over_after_it_lapsed_is_reported_lost_and_not_renewed() {
    let issue = Rc::new(RefCell::new(Issue::default()));
    let policy = policy(&[]);
    let (michael, colton) = two_loops(&issue, 0);
    assert_eq!(stake(&colton, &policy, COLTON, "7", at(0)).unwrap(), None);

    // Colton's renewals stopped; Michael takes the lapsed claim over.
    let (michael_later, colton_later) = two_loops(&issue, 61);
    drop((michael, colton));
    assert_eq!(
        stake(&michael_later, &policy, MICHAEL, "7", at(61)).unwrap(),
        None
    );

    let lost = renew(&colton_later, &policy, COLTON, "7", at(62)).unwrap();

    assert_eq!(lost, Some(format!("lease taken over by {MICHAEL}")));
    assert_eq!(assignees(&issue), [MICHAEL]);
    let colton_claim = issue.borrow().view.claims[0].clone();
    assert_eq!(colton_claim.renewed_at, at(0));
}

#[test]
fn a_lost_lease_stops_the_dispatch_with_a_claim_lost_error() {
    let lease = IssueLease {
        issue: "7".into(),
        lost: Arc::new(Mutex::new(Some(format!("lease taken over by {MICHAEL}")))),
        stop: None,
        renewer: None,
    };

    let error = lease.ensure_held().unwrap_err();

    assert!(issue_claim_lost(&error).is_some());
    assert!(!crate::dispatch::terminal::should_notify_dispatch_failure(
        &error
    ));
}

#[test]
fn a_claim_whose_comment_cannot_be_posted_leaves_no_assignee_behind() {
    let issue = Rc::new(RefCell::new(Issue {
        comments_fail: true,
        ..Default::default()
    }));
    let (michael, _) = two_loops(&issue, 0);

    assert!(stake(&michael, &policy(&[]), MICHAEL, "7", at(0)).is_err());
    assert!(assignees(&issue).is_empty());
}

#[test]
fn a_local_profile_and_a_ticket_file_are_never_claimed_on_the_provider() {
    // No `gh` is reachable here: any provider call would fail the claim.
    let tmp = tempfile::tempdir().unwrap();
    crate::provider::set_test_provider_path(&tmp.path().display().to_string());
    let mut prof = profile(tmp.path());

    let local = claim_issue(&prof, "7");
    prof.publishing.issue_claim.mode = IssueClaimMode::GithubAssignee;
    let ticket_file = claim_issue(&prof, "docs/tickets/TICKET-007-example.md");
    crate::provider::clear_test_provider_path();

    assert_eq!(local.unwrap(), ClaimOutcome::Won);
    assert_eq!(ticket_file.unwrap(), ClaimOutcome::Won);
}

/// A `gh` that answers the reads intake makes: the signed-in login, the
/// open-issue list, each issue's claim comments, and its (absent) sub-issues.
fn install_fake_gh(
    bin_dir: &std::path::Path,
    issues: &serde_json::Value,
    claims: &[(u64, String)],
) {
    let mut script = format!(
        "#!/bin/sh\ncase \"$*\" in\n  'api user '*) echo {MICHAEL} ;;\n  *'issues?state=open'*) cat <<'JSON'\n{issues}\nJSON\n  ;;\n"
    );
    for (number, lines) in claims {
        script.push_str(&format!(
            "  *'issues/{number}/comments'*) cat <<'JSON'\n{lines}\nJSON\n  ;;\n"
        ));
    }
    script.push_str(
        "  *'/comments'*) ;;\n  *'/sub_issues'*) echo '[]' ;;\n  *) echo \"unexpected gh call: $*\" >&2; exit 2 ;;\nesac\n",
    );
    let gh_path = bin_dir.join("gh");
    std::fs::write(&gh_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn intake_skips_issues_another_login_holds() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let issue = |number: u64, assignees: &[&str]| {
        serde_json::json!({
            "number": number,
            "title": format!("Issue {number}"),
            "body": "",
            "labels": [],
            "user": {"login": "owner", "type": "User"},
            "state": "open",
            "assignees": assignees.iter().map(|login| serde_json::json!({"login": login})).collect::<Vec<_>>(),
        })
    };
    let claim_line = |id: u64, age: time::Duration| {
        serde_json::json!({
            "id": id,
            "login": COLTON,
            "created_at": (OffsetDateTime::now_utc() - age).format(&Rfc3339).unwrap(),
        })
        .to_string()
    };
    install_fake_gh(
        &bin_dir,
        &serde_json::json!([
            issue(1, &[]),
            issue(2, &[COLTON]),
            issue(3, &[COLTON]),
            issue(4, &[COLTON]),
            issue(5, &[MICHAEL]),
        ]),
        &[
            (2, claim_line(21, time::Duration::minutes(5))),
            (4, claim_line(41, time::Duration::minutes(61))),
        ],
    );
    let _guard = PathGuard::set(&bin_dir);
    let mut prof = profile(tmp.path());
    // No label gate: an unlabelled issue is eligible in this mode.
    prof.publishing.issue_claim = policy(&[MICHAEL]);

    let scan = crate::dispatch::claims::scan_available_tickets_with_dependencies(
        &prof,
        &[],
        &Default::default(),
    );

    assert_eq!(scan.provider_error, None);
    let held: Vec<(&str, bool)> = scan
        .available_tickets
        .iter()
        .map(|ticket| (ticket.ticket_path.as_str(), ticket.has_active_claim))
        .collect();
    assert_eq!(
        held,
        [
            ("1", false), // unassigned
            ("2", true),  // live claim by another login
            ("3", true),  // assigned by hand to another login
            ("4", false), // the other login's claim has expired
            ("5", false), // assigned to this loop's own login
        ]
    );
}

#[test]
fn intake_fails_closed_when_the_signed_in_login_is_unknown() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let gh_path = bin_dir.join("gh");
    std::fs::write(
        &gh_path,
        "#!/bin/sh\ncase \"$*\" in\n  'api user '*) echo 'not signed in' >&2; exit 1 ;;\n  *) echo '[]' ;;\nesac\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);
    let mut prof = profile(tmp.path());
    prof.publishing.issue_claim = policy(&[]);

    let scan = crate::dispatch::claims::scan_available_tickets_with_dependencies(
        &prof,
        &[],
        &Default::default(),
    );

    assert!(scan.available_tickets.is_empty());
    assert!(scan.provider_error.unwrap().contains("read own login"));
}

fn ledger_entry(prof: &Profile) -> LedgerEntry {
    LedgerEntry::new("test", prof, "auto", "fix", "7", None, None)
}

#[test]
fn a_dispatch_that_loses_its_claim_ends_as_a_skip_not_a_failure() {
    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // Issue 7 is assigned to someone else by hand: no claim comments exist.
    let gh_path = bin_dir.join("gh");
    std::fs::write(
        &gh_path,
        "#!/bin/sh\ncase \"$*\" in\n  'api user '*) echo me ;;\n  *'/comments'*) ;;\n  *'issues/7 '*) echo someone-else ;;\n  *) exit 2 ;;\nesac\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _guard = PathGuard::set(&bin_dir);
    let mut prof = profile(tmp.path());
    prof.publishing.issue_claim = policy(&[]);
    let mut ledger = ledger_entry(&prof);

    let error = claim_or_lose(&prof, "7", &mut ledger).unwrap_err();

    assert_eq!(
        issue_claim_lost(&error).map(ToString::to_string).as_deref(),
        Some("issue #7: held by someone-else")
    );
    assert!(crate::ledger::gates::launched_no_backend(&ledger));
    assert!(!crate::dispatch::terminal::should_notify_dispatch_failure(
        &error
    ));
}

#[test]
fn a_lost_claim_does_not_count_as_an_attempt_on_the_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let ticket_dir = tmp.path().join("docs/tickets");
    std::fs::create_dir_all(&ticket_dir).unwrap();
    std::fs::write(
        ticket_dir.join("TICKET-302-test.md"),
        "# TICKET-302: Test lost claim\nGoal: remain dispatchable\n",
    )
    .unwrap();
    let mut prof = profile(tmp.path());
    prof.provider = String::new();
    let mut lost = ledger_entry(&prof);
    lost.work_id = Some("TICKET-302".into());
    lost.validation_result = Some(crate::ledger::gates::CLAIM_LOST.into());
    lost.failure_class = Some("harness_error".into());
    let index = crate::ledger::index_entries_by_work_id(&[lost]);

    let scan =
        crate::dispatch::claims::scan_available_tickets_with_dependencies(&prof, &[], &index);

    assert_eq!(scan.available_tickets.len(), 1);
    assert_eq!(scan.available_tickets[0].prior_attempt_count, 0);
    assert_eq!(scan.available_tickets[0].last_failure_class, None);
}
