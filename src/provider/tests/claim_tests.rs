use super::*;
use crate::provider::claims::{claim_lost, GithubClaim};
use std::time::{Duration, Instant};

#[test]
fn pr_assignment_settles_monitors_loss_and_releases_only_own_bot() {
    let tmp = TempDir::new().unwrap();
    let state = tmp.path().join("state.json");
    let calls = tmp.path().join("calls");
    let now = chrono::Utc::now().to_rfc3339();
    fs::write(
        &state,
        format!(r#"{{"state":"open","updated_at":"{now}","assignees":[]}}"#),
    )
    .unwrap();
    make_fake_bin(
        tmp.path(),
        "gh",
        &format!(
            r#"#!/bin/sh
/bin/echo "$*" >> '{calls}'
case "$3 $4" in
"GET repos/owner/repo/pulls") /bin/echo '[{{"number":42}}]' ;;
"PATCH repos/owner/repo/issues/42") /bin/echo '{{"state":"open","updated_at":"{now}","assignees":[{{"login":"bot-a"}}]}}' > '{state}'; /bin/cat '{state}' ;;
"GET repos/owner/repo/issues/42") /bin/cat '{state}' ;;
"GET repos/owner/repo/pulls/42") /bin/echo '{{"updated_at":"{now}","head":{{"sha":"abc"}}}}' ;;
"GET repos/owner/repo/commits/abc") /bin/echo '{{"commit":{{"committer":{{"date":"{now}"}}}}}}' ;;
"GET repos/owner/repo/commits/abc/status") /bin/echo '{{"statuses":[]}}' ;;
"GET repos/owner/repo/commits/abc/check-runs") /bin/echo '{{"check_runs":[]}}' ;;
"DELETE repos/owner/repo/issues/42/assignees") /bin/echo '{{}}' ;;
*) /bin/echo "unexpected gh invocation: $*" >&2; exit 1 ;;
esac
"#,
            calls = calls.display(),
            state = state.display(),
        ),
    );
    let _path = PathOverride::set(tmp.path().to_string_lossy().into_owned());
    let mut profile = github_profile();
    profile.publishing.github_claim_identity = Some("bot-a".into());
    profile.publishing.github_claim_settle_seconds = 1;
    profile.publishing.github_claim_poll_seconds = 1;
    let start = Instant::now();
    let claim = GithubClaim::acquire(&profile, "gah/42").unwrap().unwrap();
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert!(claim.ensure_owned().is_ok());
    // Recent activity also protects claims made by this same identity.
    assert!(GithubClaim::acquire(&profile, "gah/42").unwrap().is_none());
    fs::write(
        &state,
        format!(r#"{{"state":"open","updated_at":"{now}","assignees":[{{"login":"bot-b"}}]}}"#),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !claim_lost() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(claim_lost());
    assert!(claim.ensure_owned().is_err());
    drop(claim);
    assert!(!claim_lost());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(calls.contains("PATCH repos/owner/repo/issues/42 -f assignees[]=bot-a"));
    assert!(calls.contains("DELETE repos/owner/repo/issues/42/assignees -f assignees[]=bot-a"));
    assert!(!calls.contains("assignees[]=bot-b"));
}

#[test]
fn shared_pr_intake_keeps_trust_and_unattended_policy_without_fleet_routing() {
    let mut profile = github_profile();
    profile.publishing.issue_intake_mode = crate::config::IssueIntakeMode::CanonicalAutonomousOnly;
    let trusted = serde_json::json!({"user": {"login": "owner", "type": "User"}});
    let untrusted = serde_json::json!({"user": {"login": "stranger", "type": "User"}});
    let mut labels = vec![profile.publishing.canonical_autonomous_label.clone()];
    assert!(crate::dispatch::github_work_item_intake_allowed(
        &profile, &trusted, &labels
    ));
    labels.push("fleet:other".into());
    assert!(crate::dispatch::github_work_item_intake_allowed(
        &profile, &trusted, &labels
    ));
    assert!(!crate::dispatch::github_work_item_intake_allowed(
        &profile, &untrusted, &labels
    ));
    assert!(!crate::dispatch::github_work_item_intake_allowed(
        &profile,
        &trusted,
        &[]
    ));
    for blocker in ["exec:owner-decision", "blocked", "gah:blocked", "planning"] {
        let mut blocked = labels.clone();
        blocked.push(blocker.into());
        assert!(!crate::dispatch::github_work_item_intake_allowed(
            &profile, &trusted, &blocked
        ));
    }
}
