use crate::config::{self, GahConfig};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::process::Command;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

pub fn run(cfg: &GahConfig, profile_name: &str, json: bool) -> Result<()> {
    let profile = config::get_profile(cfg, profile_name)?;
    let mrs = match profile.provider.as_str() {
        "github" => github_prs(profile)?,
        "gitlab" => gitlab_mrs(profile)?,
        other => anyhow::bail!("unsupported provider: {}", other),
    };

    if json {
        let items: Vec<_> = mrs
            .into_iter()
            .map(|mr| {
                let class = classify(&mr);
                serde_json::json!({
                    "class": class,
                    "branch": mr.branch,
                    "title": mr.title,
                    "url": mr.url,
                    "recommended": recommended_action(class),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    println!("Profile: {}", profile_name);
    for mr in mrs {
        let class = classify(&mr);
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

#[derive(Debug, Clone)]
struct SyncMr {
    title: String,
    branch: String,
    labels: Vec<String>,
    url: Option<String>,
    merged: bool,
    updated_at: Option<String>,
    ci_failed: bool,
}

fn classify(mr: &SyncMr) -> &'static str {
    if mr.merged {
        return "MERGED";
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

fn recommended_action(class: &str) -> &'static str {
    match class {
        "CI_FAILED" => "reuse same branch/MR for a fix run",
        "NEEDS_FIX" => "reuse same branch/MR for a fix run",
        "READY_FOR_HUMAN" => "human review and merge decision",
        "NEEDS_REVIEW" => "run review or request human review",
        "MERGED" => "none",
        "STALE" => "inspect before reusing branch",
        _ => "inspect manually",
    }
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
            "title,headRefName,url,labels,mergedAt,updatedAt,statusCheckRollup",
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
            merged: mr.merged_at.is_some(),
            updated_at: mr.updated_at,
            ci_failed: false,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{classify, SyncMr};

    #[test]
    fn ready_label_maps_to_ready_for_human() {
        let mr = SyncMr {
            title: "x".into(),
            branch: "gah/test".into(),
            labels: vec!["gah-ready-for-human".into()],
            url: None,
            merged: false,
            updated_at: None,
            ci_failed: false,
        };
        assert_eq!(classify(&mr), "READY_FOR_HUMAN");
    }
}
