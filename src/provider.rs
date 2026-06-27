use crate::config::Profile;
use anyhow::{Context, Result};
use std::process::Command;

pub struct MrResult {
    pub url: String,
    pub id: String,
}

pub fn create_draft_mr(
    profile: &Profile,
    branch: &str,
    title: &str,
    body: &str,
) -> Result<MrResult> {
    match profile.provider.as_str() {
        "gitlab" => gitlab_mr(profile, branch, title, body),
        "github" => github_mr(profile, branch, title, body),
        other => anyhow::bail!("unsupported provider: {}", other),
    }
}

fn gitlab_mr(profile: &Profile, branch: &str, title: &str, body: &str) -> Result<MrResult> {
    let api_base = profile
        .provider_api_base
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_api_base for gitlab"))?;
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let pat = profile.pat();
    let url = format!("{}/projects/{}/merge_requests", api_base, project_id);
    let payload = serde_json::json!({
        "source_branch": branch,
        "target_branch": profile.default_target_branch,
        "title": format!("Draft: {}", title),
        "description": body,
    });

    let out = Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            "-H",
            &format!("PRIVATE-TOKEN: {}", pat),
            "-H",
            "Content-Type: application/json",
            "-d",
            &payload.to_string(),
            &url,
        ])
        .output()
        .context("curl gitlab create mr")?;

    let resp: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing gitlab MR response")?;
    Ok(MrResult {
        url: resp["web_url"].as_str().unwrap_or("").to_string(),
        id: resp["iid"].to_string(),
    })
}

fn github_mr(profile: &Profile, branch: &str, title: &str, body: &str) -> Result<MrResult> {
    let out = Command::new("gh")
        .args([
            "pr",
            "create",
            "--repo",
            &profile.repo,
            "--base",
            &profile.default_target_branch,
            "--head",
            branch,
            "--title",
            &format!("Draft: {}", title),
            "--body",
            body,
            "--draft",
        ])
        .output()
        .context("gh pr create")?;

    if !out.status.success() {
        anyhow::bail!(
            "gh pr create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(MrResult {
        url,
        id: String::new(),
    })
}
