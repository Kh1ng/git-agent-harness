use super::{
    extract_work_id_from_title, fetch_mrs_for_scope, github_ci_failed, github_ci_passed,
    GithubCheck, GithubPr, MrFetchScope, SyncMr,
};
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
    labels: Vec<GithubRestLabel>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRestLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GithubRestHead {
    #[serde(rename = "ref")]
    branch: String,
    #[serde(default)]
    sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubCheckRunsResponse {
    total_count: usize,
    #[serde(default)]
    check_runs: Vec<GithubRestCheckRun>,
}

#[derive(Debug, Deserialize)]
struct GithubRestCheckRun {
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
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

pub(super) fn fetch_active_github_mrs(
    profile: &config::Profile,
    filter_gah_branches: bool,
) -> Result<Vec<SyncMr>> {
    let open_prs = github_open_prs_rest(profile, "active observation")?;
    let mut rows = Vec::new();
    for pr in open_prs
        .into_iter()
        .filter(|pr| !filter_gah_branches || pr.head.branch.starts_with("gah/"))
    {
        let checks = github_check_runs_rest(profile, pr.head.sha.as_deref())?;
        rows.push(SyncMr {
            work_id: extract_work_id_from_title(&pr.title),
            title: pr.title,
            body: pr.body,
            branch: pr.head.branch,
            labels: pr.labels.into_iter().map(|label| label.name).collect(),
            url: Some(pr.html_url),
            id: Some(pr.number.to_string()),
            state: Some(pr.state),
            draft: pr.draft,
            source_sha: pr.head.sha,
            merge_status: None,
            merged: false,
            updated_at: pr.updated_at,
            merged_at: None,
            ci_failed: github_ci_failed(Some(&checks)),
            ci_passed: github_ci_passed(Some(&checks)),
            ci_pending: false,
        });
    }
    Ok(rows)
}

pub(super) fn fetch_historical_github_mrs(
    profile: &config::Profile,
    filter_gah_branches: bool,
) -> Result<Vec<SyncMr>> {
    let out = crate::provider::provider_command("gh")
        .args([
            "pr",
            "list",
            "--repo",
            &profile.repo,
            "--state",
            "all",
            "--limit",
            "1000",
            "--json",
            "title,body,headRefName,headRefOid,url,labels,number,state,isDraft,mergeStateStatus,mergedAt,updatedAt,statusCheckRollup",
        ])
        .output()
        .context("gh pr list")?;
    if !out.status.success() {
        anyhow::bail!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let prs: Vec<GithubPr> = serde_json::from_slice(&out.stdout)?;
    Ok(prs
        .into_iter()
        .filter(|pr| !filter_gah_branches || pr.head_ref_name.starts_with("gah/"))
        .map(|pr| SyncMr {
            work_id: extract_work_id_from_title(&pr.title),
            title: pr.title,
            body: pr.body,
            branch: pr.head_ref_name,
            labels: pr.labels.into_iter().map(|label| label.name).collect(),
            url: pr.url,
            id: pr.number.map(|number| number.to_string()),
            state: pr.state,
            draft: pr.is_draft,
            source_sha: pr.head_ref_oid,
            merge_status: pr.merge_state_status,
            merged: pr.merged_at.is_some(),
            updated_at: pr.updated_at,
            merged_at: pr.merged_at,
            ci_failed: github_ci_failed(pr.status_check_rollup.as_deref()),
            ci_passed: github_ci_passed(pr.status_check_rollup.as_deref()),
            ci_pending: false,
        })
        .collect())
}

fn github_open_prs_rest(profile: &config::Profile, purpose: &str) -> Result<Vec<GithubRestPr>> {
    let endpoint = format!(
        "repos/{}/pulls?state=open&per_page={OPEN_PR_LIMIT}&sort=updated&direction=desc",
        profile.repo
    );
    let out = crate::provider::provider_command("gh")
        .args(["api", "--method", "GET", &endpoint])
        .output()
        .with_context(|| format!("GitHub REST {purpose} open-PR snapshot"))?;
    if !out.status.success() {
        anyhow::bail!(
            "GitHub REST {purpose} open-PR snapshot failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let prs: Vec<GithubRestPr> = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing GitHub REST {purpose} open-PR snapshot"))?;
    if prs.len() >= OPEN_PR_LIMIT {
        anyhow::bail!(
            "GitHub REST {purpose} open-PR snapshot reached its cap ({OPEN_PR_LIMIT}); refusing incomplete state"
        );
    }
    Ok(prs)
}

fn github_check_runs_rest(
    profile: &config::Profile,
    sha: Option<&str>,
) -> Result<Vec<GithubCheck>> {
    let Some(sha) = sha.filter(|sha| !sha.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let endpoint = format!(
        "repos/{}/commits/{sha}/check-runs?per_page={OPEN_PR_LIMIT}",
        profile.repo
    );
    let out = crate::provider::provider_command("gh")
        .args(["api", "--method", "GET", &endpoint])
        .output()
        .context("GitHub REST active-PR check-run snapshot")?;
    if !out.status.success() {
        anyhow::bail!(
            "GitHub REST active-PR check-run snapshot failed for {sha}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let response: GithubCheckRunsResponse = serde_json::from_slice(&out.stdout)
        .context("parsing GitHub REST active-PR check-run snapshot")?;
    if response.total_count > response.check_runs.len() {
        anyhow::bail!(
            "GitHub REST active-PR check-run snapshot reached its cap ({OPEN_PR_LIMIT}) for {sha}; refusing incomplete CI state"
        );
    }
    Ok(response
        .check_runs
        .into_iter()
        .map(|check| GithubCheck {
            conclusion: (check.status.eq_ignore_ascii_case("completed"))
                .then(|| check.conclusion.map(|value| value.to_ascii_uppercase()))
                .flatten(),
        })
        .collect())
}

fn github_repository_prs_rest(profile: &config::Profile) -> Result<Vec<SyncMr>> {
    let open_prs = github_open_prs_rest(profile, "PM context")?;

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
            labels: pr.labels.into_iter().map(|label| label.name).collect(),
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
