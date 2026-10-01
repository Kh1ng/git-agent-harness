//! Reads issues and their native relations from GitHub or GitLab. One
//! listing gives titles, states, labels, and bodies; native relations cost a
//! call per issue, so only issues in the requested epic are asked, and on
//! GitHub only those whose listing says they have any.

use super::map::{self, IssueFacts};
use crate::config::Profile;
use crate::dispatch::dependencies::{fetch_github_sub_issues, fetch_gitlab_blocks_links};
use crate::provider::{list_provider_issue_values, provider_command};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};

pub struct Listing {
    pub issues: BTreeMap<u64, IssueFacts>,
    /// GitHub sub-issue counts from the listing: (total, open).
    pub native_child_counts: BTreeMap<u64, (usize, usize)>,
    /// GitHub issues whose listing reports blockers.
    pub with_blockers: BTreeSet<u64>,
    /// False when the provider sent no relation counts at all (GitLab, or an
    /// older GitHub Enterprise), so every issue in the tree must be asked.
    pub counted: bool,
}

pub fn listing(profile: &Profile) -> Result<Listing> {
    let github = match profile.provider.as_str() {
        "github" => true,
        "gitlab" => false,
        other => bail!("unsupported provider: {other}"),
    };
    let mut result = Listing {
        issues: BTreeMap::new(),
        native_child_counts: BTreeMap::new(),
        with_blockers: BTreeSet::new(),
        counted: false,
    };
    for value in list_provider_issue_values(profile)? {
        let Some(facts) = facts(&value, github) else {
            continue;
        };
        result.counted |= value.get("sub_issues_summary").is_some();
        let count = |pointer: &str| value.pointer(pointer).and_then(|v| v.as_u64()).unwrap_or(0);
        let total = count("/sub_issues_summary/total");
        if total > 0 {
            let done = count("/sub_issues_summary/completed").min(total);
            result
                .native_child_counts
                .insert(facts.number, (total as usize, (total - done) as usize));
        }
        if count("/issue_dependencies_summary/total_blocked_by") > 0 {
            result.with_blockers.insert(facts.number);
        }
        result.issues.insert(facts.number, facts);
    }
    Ok(result)
}

fn facts(value: &serde_json::Value, github: bool) -> Option<IssueFacts> {
    let field = |github_name: &str, gitlab_name: &str| {
        value[if github { github_name } else { gitlab_name }]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    Some(IssueFacts {
        number: value[if github { "number" } else { "iid" }].as_u64()?,
        title: field("title", "title"),
        url: field("html_url", "web_url"),
        open: matches!(value["state"].as_str(), Some("open" | "opened")),
        labels: value["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|label| label.as_str().or_else(|| label["name"].as_str()))
            .map(ToOwned::to_owned)
            .collect(),
        body: field("body", "description"),
        native_children: Vec::new(),
        native_blockers: Vec::new(),
    })
}

/// Fills in native relations for the epic's tree. Children are found level
/// by level, since a sub-issue can have its own. GitLab has no native
/// children; its `Parent:` lines are the relation.
pub fn relate(profile: &Profile, epic: u64, listing: &mut Listing) -> Result<()> {
    let github = profile.provider == "github";
    let mut asked = BTreeSet::new();
    // Off GitHub there are no native children: one pass asks nothing.
    loop {
        let pending: Vec<u64> = map::descendants(epic, &listing.issues)
            .into_keys()
            .filter(|number| github && !asked.contains(number))
            .filter(|number| !listing.counted || listing.native_child_counts.contains_key(number))
            .collect();
        if pending.is_empty() {
            break;
        }
        asked.extend(pending.iter().copied());
        for (number, children) in each(&pending, |number| {
            Ok(fetch_github_sub_issues(profile, &number.to_string())
                .map_err(|error| anyhow::anyhow!("{error}"))?
                .iter()
                .filter_map(|child| child.parse().ok())
                .collect())
        })? {
            if let Some(issue) = listing.issues.get_mut(&number) {
                issue.native_children = children;
            }
        }
    }
    let tree: Vec<u64> = map::descendants(epic, &listing.issues)
        .into_keys()
        .filter(|number| !listing.counted || listing.with_blockers.contains(number))
        .collect();
    for (number, blockers) in each(&tree, |number| {
        if github {
            github_blocked_by(profile, number)
        } else {
            Ok(fetch_gitlab_blocks_links(profile, &number.to_string())
                .map_err(|error| anyhow::anyhow!("{error}"))?
                .iter()
                .filter_map(|blocker| blocker.parse().ok())
                .collect())
        }
    })? {
        if let Some(issue) = listing.issues.get_mut(&number) {
            issue.native_blockers = blockers;
        }
    }
    Ok(())
}

/// Runs `ask` for every number, a few at a time: each is a provider round
/// trip, and an epic can have dozens.
fn each(
    numbers: &[u64],
    ask: impl Fn(u64) -> Result<Vec<u64>> + Sync,
) -> Result<Vec<(u64, Vec<u64>)>> {
    const AT_ONCE: usize = 6;
    let mut answers = Vec::new();
    for batch in numbers.chunks(AT_ONCE) {
        let results: Vec<Result<Vec<u64>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|number| {
                    let ask = &ask;
                    scope.spawn(move || ask(*number))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("relation lookup panicked")))
                })
                .collect()
        });
        for (number, result) in batch.iter().zip(results) {
            answers.push((*number, result?));
        }
    }
    Ok(answers)
}

/// GitHub's native "blocked by" dependencies for one issue, limited to this
/// repository. A 404 means the repository has no dependency support.
fn github_blocked_by(profile: &Profile, number: u64) -> Result<Vec<u64>> {
    let endpoint = format!(
        "repos/{}/issues/{number}/dependencies/blocked_by",
        profile.repo
    );
    let output = provider_command("gh")
        .args(["api", "--method", "GET", &endpoint])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("404") || stderr.to_ascii_lowercase().contains("not found") {
            return Ok(Vec::new());
        }
        bail!(
            "gh api {endpoint} failed: {}",
            crate::redact::redact(stderr.trim())
        );
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(same_repository_numbers(&value, &profile.repo))
}

fn same_repository_numbers(value: &serde_json::Value, repo: &str) -> Vec<u64> {
    let suffix = format!("/repos/{}", repo.to_ascii_lowercase());
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|issue| {
            issue["repository_url"]
                .as_str()
                .is_none_or(|url| url.to_ascii_lowercase().ends_with(&suffix))
        })
        .filter_map(|issue| issue["number"].as_u64())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn github_and_gitlab_listings_read_the_same_facts() {
        let github = json!({"number": 7, "title": "T", "html_url": "https://g/7", "state": "open",
            "labels": [{"name": "epic"}], "body": "Parent: #1"});
        let gitlab = json!({"iid": 7, "title": "T", "web_url": "https://g/7", "state": "opened",
            "labels": ["epic"], "description": "Parent: #1"});
        for (value, is_github) in [(github, true), (gitlab, false)] {
            let facts = facts(&value, is_github).unwrap();
            assert_eq!(
                (facts.number, facts.open, facts.labels.clone()),
                (7, true, vec!["epic".to_string()])
            );
            assert_eq!(
                (facts.url.as_str(), facts.body.as_str()),
                ("https://g/7", "Parent: #1")
            );
        }
        assert!(
            !facts(&json!({"number": 8, "state": "closed"}), true)
                .unwrap()
                .open
        );
    }

    #[test]
    fn blockers_from_other_repositories_are_left_out() {
        let value = json!([
            {"number": 3, "repository_url": "https://api.github.com/repos/Owner/Repo"},
            {"number": 4, "repository_url": "https://api.github.com/repos/other/repo"},
            {"number": 5}
        ]);
        assert_eq!(same_repository_numbers(&value, "owner/repo"), vec![3, 5]);
    }
}
