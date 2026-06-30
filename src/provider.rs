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

pub fn post_review_comment(
    profile: &Profile,
    branch: &str,
    body: &str,
    labels: &[&str],
) -> Result<()> {
    match profile.provider.as_str() {
        "gitlab" => gitlab_post_review_comment(profile, branch, body, labels),
        "github" => github_post_review_comment(profile, branch, body, labels),
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

fn gitlab_post_review_comment(
    profile: &Profile,
    branch: &str,
    body: &str,
    labels: &[&str],
) -> Result<()> {
    let api_base = profile
        .provider_api_base
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_api_base for gitlab"))?;
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let pat = profile.pat();
    let mr = gitlab_find_mr_by_branch(profile, branch)?;
    let note_url = format!(
        "{}/projects/{}/merge_requests/{}/notes",
        api_base, project_id, mr.id
    );
    let payload = serde_json::json!({ "body": body });
    run_curl_json(&[
        "-s",
        "-X",
        "POST",
        "-H",
        &format!("PRIVATE-TOKEN: {}", pat),
        "-H",
        "Content-Type: application/json",
        "-d",
        &payload.to_string(),
        &note_url,
    ])?;
    if !labels.is_empty() {
        let labels_url = format!(
            "{}/projects/{}/merge_requests/{}",
            api_base, project_id, mr.id
        );
        let payload = serde_json::json!({ "add_labels": labels.join(",") });
        let _ = run_curl_json(&[
            "-s",
            "-X",
            "PUT",
            "-H",
            &format!("PRIVATE-TOKEN: {}", pat),
            "-H",
            "Content-Type: application/json",
            "-d",
            &payload.to_string(),
            &labels_url,
        ]);
    }
    Ok(())
}

fn github_post_review_comment(
    profile: &Profile,
    branch: &str,
    body: &str,
    labels: &[&str],
) -> Result<()> {
    let pr_number = github_find_pr_number_by_branch(profile, branch)?;
    let out = Command::new("gh")
        .args([
            "pr",
            "comment",
            &pr_number,
            "--repo",
            &profile.repo,
            "--body",
            body,
        ])
        .output()
        .context("gh pr comment")?;
    if !out.status.success() {
        anyhow::bail!(
            "gh pr comment failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if !labels.is_empty() {
        let _ = Command::new("gh")
            .args([
                "pr",
                "edit",
                &pr_number,
                "--repo",
                &profile.repo,
                "--add-label",
                &labels.join(","),
            ])
            .output();
    }
    Ok(())
}

pub fn gitlab_find_mr_by_branch(profile: &Profile, branch: &str) -> Result<MrResult> {
    let api_base = profile
        .provider_api_base
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_api_base for gitlab"))?;
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let pat = profile.pat();
    let url = format!(
        "{}/projects/{}/merge_requests?state=opened&source_branch={}",
        api_base, project_id, branch
    );
    let out = run_curl_json(&["-s", "-H", &format!("PRIVATE-TOKEN: {}", pat), &url])?;
    let resp: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let first = resp
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| anyhow::anyhow!("no open GitLab MR found for branch '{}'", branch))?;
    Ok(MrResult {
        url: first["web_url"].as_str().unwrap_or("").to_string(),
        id: first["iid"].to_string(),
    })
}

fn github_find_pr_number_by_branch(profile: &Profile, branch: &str) -> Result<String> {
    let out = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            &profile.repo,
            "--head",
            branch,
            "--state",
            "open",
            "--json",
            "number",
        ])
        .output()
        .context("gh pr list")?;
    if !out.status.success() {
        anyhow::bail!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let resp: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let number = resp
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["number"].as_i64())
        .ok_or_else(|| anyhow::anyhow!("no open GitHub PR found for branch '{}'", branch))?;
    Ok(number.to_string())
}

fn run_curl_json(args: &[&str]) -> Result<std::process::Output> {
    let out = Command::new("curl")
        .args(args)
        .output()
        .context("curl request")?;
    if !out.status.success() {
        anyhow::bail!(
            "curl failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out)
}
