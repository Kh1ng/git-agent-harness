use super::{extract_work_id_from_title, fetch_mrs_for_scope, MrFetchScope, SyncMr};
use crate::config;
use anyhow::{Context, Result};
use serde::Deserialize;

const OPEN_PR_LIMIT: usize = 100;
const RECENT_MERGED_PR_LIMIT: usize = 20;

#[derive(Debug, Deserialize)]
struct GithubRestPr {
    number: i64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    state: String,
    #[serde(default)]
    draft: bool,
    head: GithubRestHead,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRestHead {
    #[serde(rename = "ref")]
    branch: String,
    #[serde(default)]
    sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubMergedSearchResponse {
    #[serde(default)]
    incomplete_results: bool,
    #[serde(default)]
    items: Vec<GithubMergedSearchItem>,
}

#[derive(Debug, Deserialize)]
struct GithubMergedSearchItem {
    number: i64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
}

/// Fetch all repository merge/pull requests regardless of branch naming,
/// required for PM preflight duplicate ticket detection.
pub fn fetch_repository_mrs(profile: &config::Profile) -> Result<Vec<SyncMr>> {
    match profile.provider.as_str() {
        "github" => github_repository_prs_rest(profile),
        "gitlab" => fetch_mrs_for_scope(profile, MrFetchScope::FullHistory, false),
        other => anyhow::bail!("unsupported provider: {other}"),
    }
}

fn github_repository_prs_rest(profile: &config::Profile) -> Result<Vec<SyncMr>> {
    let open_endpoint = format!(
        "repos/{}/pulls?state=open&per_page={OPEN_PR_LIMIT}&sort=updated&direction=desc",
        profile.repo
    );
    let open_out = crate::provider::provider_command("gh")
        .args(["api", "--method", "GET", &open_endpoint])
        .output()
        .context("GitHub REST open-PR snapshot")?;
    if !open_out.status.success() {
        anyhow::bail!(
            "GitHub REST open-PR snapshot failed: {}",
            String::from_utf8_lossy(&open_out.stderr).trim()
        );
    }
    let open_prs: Vec<GithubRestPr> = serde_json::from_slice(&open_out.stdout)?;
    if open_prs.len() >= OPEN_PR_LIMIT {
        anyhow::bail!(
            "GitHub REST open-PR snapshot reached its cap ({OPEN_PR_LIMIT}); refusing incomplete PM context"
        );
    }

    let query = format!("q=repo:{} is:pr is:merged", profile.repo);
    let merged_out = crate::provider::provider_command("gh")
        .args([
            "api",
            "--method",
            "GET",
            "search/issues",
            "-f",
            &query,
            "-f",
            "sort=updated",
            "-f",
            "order=desc",
            "-f",
            &format!("per_page={RECENT_MERGED_PR_LIMIT}"),
        ])
        .output()
        .context("GitHub REST recent-merged-PR snapshot")?;
    if !merged_out.status.success() {
        anyhow::bail!(
            "GitHub REST recent-merged-PR snapshot failed: {}",
            String::from_utf8_lossy(&merged_out.stderr).trim()
        );
    }
    let merged: GithubMergedSearchResponse = serde_json::from_slice(&merged_out.stdout)?;
    if merged.incomplete_results {
        anyhow::bail!(
            "GitHub REST recent-merged-PR search was incomplete; refusing incomplete PM context"
        );
    }

    let mut rows = open_prs
        .into_iter()
        .map(|pr| SyncMr {
            work_id: extract_work_id_from_title(&pr.title),
            title: pr.title,
            body: pr.body,
            branch: pr.head.branch,
            labels: Vec::new(),
            url: Some(pr.html_url),
            id: Some(pr.number.to_string()),
            state: Some(pr.state),
            draft: pr.draft,
            source_sha: pr.head.sha,
            merge_status: None,
            merged: false,
            updated_at: pr.updated_at,
            merged_at: None,
            ci_failed: false,
            ci_passed: false,
            ci_pending: false,
        })
        .collect::<Vec<_>>();
    rows.extend(
        merged
            .items
            .into_iter()
            .take(RECENT_MERGED_PR_LIMIT)
            .map(|pr| SyncMr {
                work_id: extract_work_id_from_title(&pr.title),
                title: pr.title,
                body: pr.body,
                branch: "(merged)".into(),
                labels: Vec::new(),
                url: Some(pr.html_url),
                id: Some(pr.number.to_string()),
                state: Some("closed".into()),
                draft: false,
                source_sha: None,
                merge_status: None,
                merged: true,
                updated_at: pr.updated_at,
                merged_at: pr.closed_at,
                ci_failed: false,
                ci_passed: false,
                ci_pending: false,
            }),
    );
    Ok(rows)
}
