use crate::config::Profile;
use anyhow::{Context, Result};
use std::process::Command;

#[cfg(test)]
thread_local! {
    /// Per-thread PATH override for provider CLI tests. Thread-local (not a
    /// process-global env var) so parallel tests in other modules that need
    /// the real PATH (git, sh, ...) are never affected — see
    /// tests::PathOverride below for why this exists.
    static TEST_PATH_OVERRIDE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Construct a Command for an external provider CLI (`gh`, `curl`). In test
/// builds, honors a thread-local PATH override so tests can hide/replace
/// these binaries without touching the process-wide PATH.
fn provider_command(name: &str) -> Command {
    // `mut` is only needed under #[cfg(test)] below; clippy flags it as
    // unused in non-test builds where that block is compiled out.
    #[allow(unused_mut)]
    let mut cmd = Command::new(name);
    #[cfg(test)]
    {
        TEST_PATH_OVERRIDE.with(|p| {
            if let Some(path) = p.borrow().as_ref() {
                cmd.env("PATH", path);
            }
        });
    }
    cmd
}

#[derive(Debug)]
pub struct MrResult {
    pub url: String,
    pub id: String,
}

pub struct ReviewTarget {
    pub id: String,
    pub url: String,
    pub source_branch: String,
    pub target_branch: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub ci_status: Option<String>,
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

pub fn find_review_target_by_branch(profile: &Profile, branch: &str) -> Result<ReviewTarget> {
    match profile.provider.as_str() {
        "gitlab" => gitlab_review_target_by_branch(profile, branch),
        "github" => github_review_target_by_branch(profile, branch),
        other => anyhow::bail!("unsupported provider: {}", other),
    }
}

pub fn find_review_target_by_mr(profile: &Profile, mr: &str) -> Result<ReviewTarget> {
    match profile.provider.as_str() {
        "gitlab" => gitlab_review_target_by_iid(profile, mr),
        "github" => github_review_target_by_number(profile, mr),
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

    let out = provider_command("curl")
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
    let out = provider_command("gh")
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
    let out = provider_command("gh")
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
        let _ = provider_command("gh")
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
    let target = gitlab_review_target_by_branch(profile, branch)?;
    Ok(MrResult {
        url: target.url,
        id: target.id,
    })
}

fn gitlab_review_target_by_branch(profile: &Profile, branch: &str) -> Result<ReviewTarget> {
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
    Ok(gitlab_target_from_value(first))
}

fn gitlab_review_target_by_iid(profile: &Profile, mr: &str) -> Result<ReviewTarget> {
    let api_base = profile
        .provider_api_base
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_api_base for gitlab"))?;
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let pat = profile.pat();
    let url = format!("{}/projects/{}/merge_requests/{}", api_base, project_id, mr);
    let out = run_curl_json(&["-s", "-H", &format!("PRIVATE-TOKEN: {}", pat), &url])?;
    let resp: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    Ok(gitlab_target_from_value(&resp))
}

fn github_find_pr_number_by_branch(profile: &Profile, branch: &str) -> Result<String> {
    let out = provider_command("gh")
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

fn github_review_target_by_branch(profile: &Profile, branch: &str) -> Result<ReviewTarget> {
    let number = github_find_pr_number_by_branch(profile, branch)?;
    github_review_target_by_number(profile, &number)
}

fn github_review_target_by_number(profile: &Profile, number: &str) -> Result<ReviewTarget> {
    let out = provider_command("gh")
        .args([
            "pr",
            "view",
            number,
            "--repo",
            &profile.repo,
            "--json",
            "number,url,title,body,headRefName,baseRefName,statusCheckRollup",
        ])
        .output()
        .context("gh pr view")?;
    if !out.status.success() {
        anyhow::bail!(
            "gh pr view failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let resp: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let ci_status = resp["statusCheckRollup"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| {
            item["conclusion"]
                .as_str()
                .or_else(|| item["status"].as_str())
        })
        .map(str::to_string);
    Ok(ReviewTarget {
        id: resp["number"]
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| number.to_string()),
        url: resp["url"].as_str().unwrap_or("").to_string(),
        source_branch: resp["headRefName"].as_str().unwrap_or("").to_string(),
        target_branch: resp["baseRefName"].as_str().unwrap_or("").to_string(),
        title: resp["title"].as_str().map(str::to_string),
        body: resp["body"].as_str().map(str::to_string),
        ci_status,
    })
}

fn gitlab_target_from_value(value: &serde_json::Value) -> ReviewTarget {
    let ci_status = value["detailed_merge_status"]
        .as_str()
        .or_else(|| value["merge_status"].as_str())
        .map(str::to_string);
    ReviewTarget {
        id: value["iid"].to_string().trim_matches('"').to_string(),
        url: value["web_url"].as_str().unwrap_or("").to_string(),
        source_branch: value["source_branch"].as_str().unwrap_or("").to_string(),
        target_branch: value["target_branch"].as_str().unwrap_or("").to_string(),
        title: value["title"].as_str().map(str::to_string),
        body: value["description"].as_str().map(str::to_string),
        ci_status,
    }
}

fn run_curl_json(args: &[&str]) -> Result<std::process::Output> {
    let out = provider_command("curl")
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

#[cfg(test)]
mod tests {
    use super::{create_draft_mr, TEST_PATH_OVERRIDE};
    use crate::config::{Profile, RoutingPolicy};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tempfile::TempDir;

    /// Sets the PATH override consulted by `provider_command()` for the
    /// *current test thread only* (a thread-local, not `std::env::set_var`).
    /// Rust runs tests in parallel threads within one process, and PATH is
    /// process-global — mutating it directly corrupts unrelated tests in
    /// other modules (worktree, dispatch, routing) that need the real PATH
    /// for `git`/`sh` mid-run. This was tried and reproduced that exact
    /// failure before being replaced with this seam.
    struct PathOverride;

    impl PathOverride {
        fn set(path: String) -> Self {
            TEST_PATH_OVERRIDE.with(|p| *p.borrow_mut() = Some(path));
            PathOverride
        }
    }

    impl Drop for PathOverride {
        fn drop(&mut self) {
            TEST_PATH_OVERRIDE.with(|p| *p.borrow_mut() = None);
        }
    }

    fn make_fake_bin(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }

    fn github_profile() -> Profile {
        Profile {
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: "/tmp/repo".into(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: None,
            provider_project_id: None,
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            test_file_patterns: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
        }
    }

    fn gitlab_profile() -> Profile {
        Profile {
            provider: "gitlab".into(),
            provider_api_base: Some("https://gitlab.example.com/api/v4".into()),
            provider_project_id: Some("42".into()),
            ..github_profile()
        }
    }

    #[test]
    fn github_mr_missing_gh_produces_actionable_error() {
        let tmp = TempDir::new().unwrap();
        let empty_bin = tmp.path().join("bin");
        fs::create_dir_all(&empty_bin).unwrap();
        // PATH deliberately has no fallback to the real system PATH: this
        // must fail even on a machine where `gh` happens to be installed.
        let _guard = PathOverride::set(empty_bin.to_str().unwrap().to_string());

        let err = create_draft_mr(&github_profile(), "gah/test", "title", "body").unwrap_err();

        assert!(format!("{:#}", err).contains("gh pr create"));
    }

    #[test]
    fn github_mr_nonzero_exit_surfaces_stderr() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        make_fake_bin(
            &bin_dir,
            "gh",
            "#!/bin/sh\necho 'insufficient scope' >&2\nexit 1\n",
        );
        let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

        let err = create_draft_mr(&github_profile(), "gah/test", "title", "body").unwrap_err();

        let msg = format!("{:#}", err);
        assert!(msg.contains("gh pr create failed"));
        assert!(msg.contains("insufficient scope"));
    }

    #[test]
    fn gitlab_mr_malformed_curl_response_fails_to_parse() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        make_fake_bin(
            &bin_dir,
            "curl",
            "#!/bin/sh\necho 'not json at all'\nexit 0\n",
        );
        let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

        let err = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap_err();

        assert!(format!("{:#}", err).contains("parsing gitlab MR response"));
    }

    #[test]
    fn gitlab_mr_missing_curl_produces_actionable_error() {
        let tmp = TempDir::new().unwrap();
        let empty_bin = tmp.path().join("bin");
        fs::create_dir_all(&empty_bin).unwrap();
        let _guard = PathOverride::set(empty_bin.to_str().unwrap().to_string());

        let err = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap_err();

        assert!(format!("{:#}", err).contains("curl gitlab create mr"));
    }
}
