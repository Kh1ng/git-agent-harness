//! A trusted human reviewer's outstanding "changes requested" on a managed
//! pull request, turned into the structured `NEEDS_FIX` review the repair
//! path already consumes. Without it GAH only knows its own reviews: it
//! approves, tries a merge the provider refuses, and escalates work an agent
//! can do.
use super::{review_state::current_review_generation, SyncMr};
use crate::config::Profile;
use crate::ledger::LedgerEntry;
use anyhow::{Context, Result};
use serde::Deserialize;

/// Backend recorded on an imported review: no agent ran it.
const HUMAN_REVIEWER: &str = "human";
const PAGE: usize = 100;

#[derive(Debug, Deserialize)]
struct RestReview {
    id: u64,
    #[serde(default)]
    user: Option<RestUser>,
    state: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    commit_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RestUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RestReviewComment {
    #[serde(default)]
    pull_request_review_id: Option<u64>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<u64>,
    body: String,
}

/// One reviewer's standing request for changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestedChanges {
    review_id: u64,
    reviewer: String,
    body: String,
    commit_sha: Option<String>,
}

/// Each reviewer's latest decisive review, kept when it still requests
/// changes. A later approval or dismissal withdraws an earlier request;
/// plain comments decide nothing. Reviews arrive oldest first.
fn outstanding(reviews: Vec<RestReview>) -> Vec<RequestedChanges> {
    let mut latest = Vec::<(String, RestReview)>::new();
    for review in reviews {
        let Some(login) = review.user.as_ref().map(|user| user.login.clone()) else {
            continue;
        };
        if !matches!(
            review.state.as_str(),
            "CHANGES_REQUESTED" | "APPROVED" | "DISMISSED"
        ) {
            continue;
        }
        latest.retain(|(seen, _)| *seen != login);
        latest.push((login, review));
    }
    latest
        .into_iter()
        .filter(|(_, review)| review.state == "CHANGES_REQUESTED")
        .map(|(reviewer, review)| RequestedChanges {
            review_id: review.id,
            reviewer,
            body: review.body.unwrap_or_default(),
            commit_sha: review.commit_id,
        })
        .collect()
}

fn github_get<T: for<'de> Deserialize<'de>>(endpoint: &str, what: &str) -> Result<T> {
    let out = crate::provider::provider_command("gh")
        .args(["api", "--method", "GET", endpoint])
        .output()
        .with_context(|| format!("GitHub REST {what}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "GitHub REST {what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("parsing GitHub REST {what}"))
}

/// The marker that ties an imported review to the provider review it came
/// from, so each request is imported once.
fn import_marker(review_id: u64) -> String {
    format!("requested_changes:{review_id}")
}

/// The structured review for one request, bound to the PR's current source
/// and metadata exactly as a GAH review of it would be.
fn review_entry(
    profile_name: &str,
    profile: &Profile,
    mr: &SyncMr,
    request: &RequestedChanges,
    inline_comments: Vec<String>,
) -> LedgerEntry {
    let mut entry = LedgerEntry::new(
        profile_name,
        profile,
        HUMAN_REVIEWER,
        "review",
        &mr.branch,
        None,
        None,
    );
    entry.work_id = mr.work_id.clone();
    entry.branch = Some(mr.branch.clone());
    entry.mr_url = mr.url.clone();
    entry.dispatch_reason = Some(import_marker(request.review_id));
    entry.reviewer_backend = Some(format!("{HUMAN_REVIEWER}:{}", request.reviewer));
    entry.review_verdict = Some("NEEDS_FIX".into());
    entry.review_source_sha = mr.source_sha.clone();
    entry.review_metadata_fingerprint = Some(mr.review_metadata_fingerprint());
    entry.review_contract_version = Some(crate::ledger::REVIEW_CONTRACT_VERSION);
    entry.review_generation = current_review_generation(mr);
    entry.review_blocking_findings = std::iter::once(format!(
        "{} requested changes on the pull request: {}",
        request.reviewer,
        request.body.trim()
    ))
    .chain(inline_comments)
    .collect();
    entry
}

/// Review entries for every trusted reviewer's request that applies to a
/// managed PR's current commit and has not been imported yet. A request made
/// against an older commit is left alone: the branch has moved since, and
/// whether the new commit satisfies it is the reviewer's call.
pub(crate) fn pending_review_entries(
    profile_name: &str,
    profile: &Profile,
    entries: &[LedgerEntry],
) -> Result<Vec<LedgerEntry>> {
    if profile.provider != "github" {
        return Ok(Vec::new());
    }
    let trusted = profile
        .publishing
        .trusted_issue_human_authors
        .clone()
        .unwrap_or_default();
    if trusted.is_empty() {
        return Ok(Vec::new());
    }
    let mut pending = Vec::new();
    for mr in super::repository::active_github_mrs_without_ci(profile, true)? {
        let (Some(number), Some(source_sha), Some(_)) = (&mr.id, &mr.source_sha, &mr.work_id)
        else {
            continue;
        };
        let reviews: Vec<RestReview> = github_get(
            &format!(
                "repos/{}/pulls/{number}/reviews?per_page={PAGE}",
                profile.repo
            ),
            "pull request reviews",
        )?;
        for request in outstanding(reviews) {
            let marker = import_marker(request.review_id);
            if !trusted.contains(&request.reviewer)
                || request.commit_sha.as_deref() != Some(source_sha.as_str())
                || entries
                    .iter()
                    .any(|entry| entry.dispatch_reason.as_deref() == Some(marker.as_str()))
            {
                continue;
            }
            let comments: Vec<RestReviewComment> = github_get(
                &format!(
                    "repos/{}/pulls/{number}/comments?per_page={PAGE}",
                    profile.repo
                ),
                "pull request review comments",
            )?;
            let inline = comments
                .into_iter()
                .filter(|comment| comment.pull_request_review_id == Some(request.review_id))
                .map(|comment| {
                    format!(
                        "{}{}: {}",
                        comment.path.unwrap_or_default(),
                        comment
                            .line
                            .map(|line| format!(":{line}"))
                            .unwrap_or_default(),
                        comment.body.trim()
                    )
                })
                .collect();
            pending.push(review_entry(profile_name, profile, &mr, &request, inline));
        }
    }
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(id: u64, login: &str, state: &str) -> RestReview {
        RestReview {
            id,
            user: Some(RestUser {
                login: login.into(),
            }),
            state: state.into(),
            body: Some(format!("review {id}")),
            commit_id: Some("abc".into()),
        }
    }

    #[test]
    fn only_a_reviewers_latest_decisive_review_counts() {
        let requests = outstanding(vec![
            review(1, "owner", "CHANGES_REQUESTED"),
            review(2, "owner", "COMMENTED"),
            review(3, "other", "CHANGES_REQUESTED"),
            review(4, "other", "APPROVED"),
            review(5, "third", "CHANGES_REQUESTED"),
            review(6, "third", "DISMISSED"),
            review(7, "owner", "CHANGES_REQUESTED"),
        ]);
        assert_eq!(
            requests
                .iter()
                .map(|request| (request.review_id, request.reviewer.as_str()))
                .collect::<Vec<_>>(),
            [(7, "owner")]
        );
    }

    #[test]
    fn an_imported_request_is_a_current_needs_fix_review_of_the_pr() {
        let profile = crate::config::tests::test_profile_for_notifications();
        let mr = SyncMr {
            title: "[GAH] Fix: #12 example".into(),
            body: Some("body".into()),
            branch: "gah/example".into(),
            labels: vec![],
            url: Some("https://github.com/owner/repo/pull/3".into()),
            id: Some("3".into()),
            state: Some("open".into()),
            draft: false,
            source_sha: Some("abc".into()),
            merge_commit_sha: None,
            merge_status: None,
            merged: false,
            updated_at: None,
            merged_at: None,
            ci_failed: false,
            ci_passed: true,
            ci_pending: false,
            work_id: Some("#12".into()),
        };
        let request = RequestedChanges {
            review_id: 7,
            reviewer: "owner".into(),
            body: " Validate the value before saving. ".into(),
            commit_sha: Some("abc".into()),
        };

        let entry = review_entry(
            "p",
            &profile,
            &mr,
            &request,
            vec!["src/a.rs:4: reject zero".into()],
        );

        assert_eq!(entry.review_verdict.as_deref(), Some("NEEDS_FIX"));
        assert_eq!(entry.reviewer_backend.as_deref(), Some("human:owner"));
        assert_eq!(entry.review_generation, current_review_generation(&mr));
        assert!(super::super::review_state::review_metadata_matches(
            &entry, &mr
        ));
        assert_eq!(
            entry.review_blocking_findings,
            [
                "owner requested changes on the pull request: Validate the value before saving.",
                "src/a.rs:4: reject zero"
            ]
        );
        assert_eq!(
            entry.dispatch_reason.as_deref(),
            Some("requested_changes:7")
        );
    }
}
