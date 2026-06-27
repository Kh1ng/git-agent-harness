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
    let out = Command::new("glab")
        .args([
            "mr",
            "create",
            "--source-branch",
            branch,
            "--target-branch",
            &profile.default_target_branch,
            "--title",
            &format!("Draft: {}", title),
            "--description",
            body,
            "--draft",
            "--yes",
        ])
        .current_dir(&profile.local_path)
        .output()
        .context("glab mr create; is glab installed and authenticated?")?;

    if !out.status.success() {
        anyhow::bail!(
            "glab mr create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.starts_with("http"))
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(MrResult {
        url,
        id: String::new(),
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
