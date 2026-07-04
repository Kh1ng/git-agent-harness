use crate::config::{self, GahConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

/// TICKET-070: `json = true` prints one JSON array to stdout and nothing
/// else (the existing human-readable output is otherwise unchanged when
/// `json = false`) -- Hermes and other automation should never have to
/// parse pretty-printed terminal output to learn MR state.
pub fn run(cfg: &GahConfig, profile_name: &str, json: bool) -> Result<()> {
    let profile = config::get_profile(cfg, profile_name)?;
    let mrs = fetch_mrs(profile)?;

    if json {
        let rows: Vec<SyncMrJson> = mrs
            .iter()
            .map(|mr| SyncMrJson {
                profile: Some(profile_name.to_string()),
                branch: mr.branch.clone(),
                id: mr.id.clone(),
                url: mr.url.clone(),
                state: mr.state.clone(),
                draft: mr.draft,
                merge_status: mr.merge_status.clone(),
                merged: mr.merged,
                classification: classify(mr).to_string(),
                recommended_action: RecommendedAction::from_class(classify(mr)),
            })
            .collect();
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }

    println!("Profile: {}", profile_name);
    for mr in &mrs {
        let class = classify(mr);
        println!(
            "{}  {}  {}  {}",
            class,
            mr.branch,
            mr.title,
            mr.url.as_deref().unwrap_or("")
        );
        println!("  recommended: {}", recommended_action(class));
    }
    Ok(())
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecommendedAction {
    ReuseBranch,
    HumanMergeDecision,
    RunReview,
    None,
    InspectBranch,
    InspectManually,
}

impl RecommendedAction {
    pub fn from_class(class: &str) -> Self {
        match class {
            "CI_FAILED" | "NEEDS_FIX" => RecommendedAction::ReuseBranch,
            "READY_FOR_HUMAN" => RecommendedAction::HumanMergeDecision,
            "NEEDS_REVIEW" => RecommendedAction::RunReview,
            "MERGED" | "CLOSED_UNMERGED" => RecommendedAction::None,
            "STALE" => RecommendedAction::InspectBranch,
            _ => RecommendedAction::InspectManually,
        }
    }
}

/// Machine-readable row for `gah sync --json`. Field set is the ticket's
/// "at minimum" list: profile, branch, MR identifier, MR URL, state, draft,
/// merge status if available, classification, recommended next action.
#[derive(Debug, Serialize, Clone)]
pub struct SyncMrJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub branch: String,
    pub id: Option<String>,
    pub url: Option<String>,
    pub state: Option<String>,
    pub draft: bool,
    pub merge_status: Option<String>,
    pub merged: bool,
    pub classification: String,
    pub recommended_action: RecommendedAction,
}

#[derive(Debug, Clone)]
pub struct SyncMr {
    pub title: String,
    pub branch: String,
    pub labels: Vec<String>,
    pub url: Option<String>,
    pub id: Option<String>,
    pub state: Option<String>,
    pub draft: bool,
    pub merge_status: Option<String>,
    pub merged: bool,
    pub updated_at: Option<String>,
    pub ci_failed: bool,
}

pub fn fetch_mrs(profile: &config::Profile) -> Result<Vec<SyncMr>> {
    match profile.provider.as_str() {
        "github" => github_prs(profile),
        "gitlab" => gitlab_mrs(profile),
        other => anyhow::bail!("unsupported provider: {}", other),
    }
}

pub fn classify(mr: &SyncMr) -> &'static str {
    if mr.merged {
        return "MERGED";
    }
    if is_closed_unmerged(mr.state.as_deref(), mr.merged) {
        return "CLOSED_UNMERGED";
    }
    if mr.ci_failed {
        return "CI_FAILED";
    }
    if mr.labels.iter().any(|l| l == "gah-needs-fix") {
        return "NEEDS_FIX";
    }
    if mr.labels.iter().any(|l| l == "gah-ready-for-human") {
        return "READY_FOR_HUMAN";
    }
    if mr
        .labels
        .iter()
        .any(|l| l == "gah-human-review" || l == "gah-review-weak")
    {
        return "NEEDS_REVIEW";
    }
    if is_stale(mr.updated_at.as_deref()) {
        return "STALE";
    }
    if mr.branch.starts_with("gah/") {
        return "NEEDS_REVIEW";
    }
    "UNKNOWN"
}

pub fn recommended_action(class: &str) -> &'static str {
    match class {
        "CI_FAILED" => "reuse same branch/MR for a fix run",
        "NEEDS_FIX" => "reuse same branch/MR for a fix run",
        "READY_FOR_HUMAN" => "human review and merge decision",
        "NEEDS_REVIEW" => "run review or request human review",
        "MERGED" => "none",
        "CLOSED_UNMERGED" => "none",
        "STALE" => "inspect before reusing branch",
        _ => "inspect manually",
    }
}

fn is_closed_unmerged(state: Option<&str>, merged: bool) -> bool {
    !merged && matches!(state.map(|s| s.to_ascii_lowercase()), Some(ref s) if s == "closed")
}

fn is_stale(updated_at: Option<&str>) -> bool {
    let Some(updated_at) = updated_at else {
        return false;
    };
    let cutoff = (OffsetDateTime::now_utc() - Duration::days(14))
        .format(&Rfc3339)
        .unwrap_or_default();
    updated_at < cutoff.as_str()
}

#[derive(Debug, Deserialize)]
struct GithubPr {
    title: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    labels: Vec<GithubLabel>,
    #[serde(default)]
    number: Option<i64>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(default)]
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(default)]
    #[serde(rename = "mergedAt")]
    merged_at: Option<String>,
    #[serde(default)]
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(default)]
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Vec<GithubCheck>,
}

#[derive(Debug, Deserialize)]
struct GithubLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GithubCheck {
    #[serde(default)]
    conclusion: Option<String>,
}

fn github_prs(profile: &crate::config::Profile) -> Result<Vec<SyncMr>> {
    let out = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            &profile.repo,
            "--state",
            "all",
            "--json",
            "title,headRefName,url,labels,number,state,isDraft,mergeStateStatus,mergedAt,updatedAt,statusCheckRollup",
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
        .filter(|pr| pr.head_ref_name.starts_with("gah/"))
        .map(|pr| SyncMr {
            title: pr.title,
            branch: pr.head_ref_name,
            labels: pr.labels.into_iter().map(|l| l.name).collect(),
            url: pr.url,
            id: pr.number.map(|n| n.to_string()),
            state: pr.state,
            draft: pr.is_draft,
            merge_status: pr.merge_state_status,
            merged: pr.merged_at.is_some(),
            updated_at: pr.updated_at,
            ci_failed: pr
                .status_check_rollup
                .iter()
                .any(|check| check.conclusion.as_deref() == Some("FAILURE")),
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct GitlabMr {
    title: String,
    source_branch: String,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    iid: Option<serde_json::Value>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    detailed_merge_status: Option<String>,
    #[serde(default)]
    merge_status: Option<String>,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

fn gitlab_mrs(profile: &crate::config::Profile) -> Result<Vec<SyncMr>> {
    let out = Command::new("glab")
        .args([
            "mr",
            "list",
            "--repo",
            &profile.repo,
            "--all",
            "--output",
            "json",
        ])
        .output()
        .context("glab mr list")?;
    if !out.status.success() {
        anyhow::bail!(
            "glab mr list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mrs: Vec<GitlabMr> = serde_json::from_slice(&out.stdout)?;
    Ok(mrs
        .into_iter()
        .filter(|mr| mr.source_branch.starts_with("gah/"))
        .map(|mr| SyncMr {
            title: mr.title,
            branch: mr.source_branch,
            labels: mr.labels,
            url: mr.web_url,
            id: mr.iid.map(|v| v.to_string().trim_matches('"').to_string()),
            state: mr.state,
            draft: mr.draft,
            merge_status: mr.detailed_merge_status.or(mr.merge_status),
            merged: mr.merged_at.is_some(),
            updated_at: mr.updated_at,
            ci_failed: false,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{classify, recommended_action, SyncMr};

    fn base_mr() -> SyncMr {
        SyncMr {
            title: "x".into(),
            branch: "gah/test".into(),
            labels: vec![],
            url: None,
            id: None,
            state: Some("OPEN".into()),
            draft: false,
            merge_status: None,
            merged: false,
            updated_at: None,
            ci_failed: false,
        }
    }

    #[test]
    fn ready_label_maps_to_ready_for_human() {
        let mut mr = base_mr();
        mr.labels = vec!["gah-ready-for-human".into()];
        assert_eq!(classify(&mr), "READY_FOR_HUMAN");
    }

    #[test]
    fn closed_unmerged_takes_precedence_over_labels_and_other_state() {
        let mut mr = base_mr();
        mr.state = Some("closed".into());
        mr.draft = true;
        mr.merge_status = Some("DIRTY".into());
        mr.labels = vec!["gah-ready-for-human".into(), "gah-human-review".into()];
        mr.ci_failed = true;

        assert_eq!(classify(&mr), "CLOSED_UNMERGED");
        assert_eq!(recommended_action(classify(&mr)), "none");
    }

    #[test]
    fn merged_still_wins_over_closed_state() {
        let mut mr = base_mr();
        mr.state = Some("closed".into());
        mr.merged = true;

        assert_eq!(classify(&mr), "MERGED");
    }
}
