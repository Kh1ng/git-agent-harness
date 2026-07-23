use super::{gitlab_api, provider_command, redacted_provider_output, ProviderIssue};
use crate::config::Profile;
use anyhow::{Context, Result};

/// Attach a GitHub child using its native sub-issue relationship. GitLab does
/// not expose an equivalent project-issue child primitive, so the deterministic
/// parent marker in the issue body remains its authoritative relationship.
pub(crate) fn link_provider_child(
    profile: &Profile,
    parent_number: &str,
    child: &ProviderIssue,
) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        return Ok(());
    }
    if profile.provider != "github" {
        return Ok(());
    }
    let endpoint = format!("repos/{}/issues/{parent_number}/sub_issues", profile.repo);
    let existing = provider_command("gh")
        .args(["api", "--method", "GET", &endpoint])
        .output()
        .context("gh api list sub-issues")?;
    if !existing.status.success() {
        anyhow::bail!(
            "gh api list sub-issues failed for {endpoint}: {}",
            redacted_provider_output(&existing)
        );
    }
    let values: serde_json::Value =
        serde_json::from_slice(&existing.stdout).context("parsing GitHub sub-issue response")?;
    if values.as_array().is_some_and(|issues| {
        issues.iter().any(|issue| {
            issue["id"].as_u64().map(|id| id.to_string()).as_deref() == Some(child.id.as_str())
        })
    }) {
        return Ok(());
    }

    // `-F` sends a JSON number; GitHub rejects a string-valued sub_issue_id.
    let out = provider_command("gh")
        .args([
            "api",
            "--method",
            "POST",
            &endpoint,
            "-F",
            &format!("sub_issue_id={}", child.id),
        ])
        .output()
        .context("gh api attach sub-issue")?;
    if out.status.success()
        || redacted_provider_output(&out)
            .to_ascii_lowercase()
            .contains("already")
    {
        Ok(())
    } else {
        anyhow::bail!(
            "gh api attach sub-issue failed for {endpoint}: {}",
            redacted_provider_output(&out)
        )
    }
}

/// Use GitLab's native issue-link relation for dependency edges. GitHub child
/// bodies retain deterministic dependency URLs because its issue dependency
/// API is not universally available across repositories.
pub(crate) fn link_provider_dependency(
    profile: &Profile,
    dependency: &ProviderIssue,
    child: &ProviderIssue,
) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        return Ok(());
    }
    match profile.provider.as_str() {
        "gitlab" => {
            let project_id = profile
                .provider_project_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
            let result = gitlab_api(
                profile,
                &format!("projects/{project_id}/issues/{}/links", dependency.number),
                "POST",
                &[
                    ("target_project_id", project_id),
                    ("target_issue_iid", child.number.as_str()),
                    ("link_type", "blocks"),
                ],
            );
            match result {
                Ok(_) => Ok(()),
                Err(error) if error.to_string().to_ascii_lowercase().contains("already") => Ok(()),
                Err(error) => Err(error),
            }
        }
        "github" => {
            let endpoint = format!(
                "repos/{}/issues/{}/dependencies/blocked_by",
                profile.repo, child.number
            );
            let existing = provider_command("gh")
                .args(["api", "--method", "GET", &endpoint])
                .output()
                .context("gh api list GitHub issue dependencies")?;
            if !existing.status.success() {
                anyhow::bail!(
                    "gh api list GitHub issue dependencies failed for {endpoint}: {}",
                    redacted_provider_output(&existing)
                );
            }
            let value: serde_json::Value = serde_json::from_slice(&existing.stdout)
                .context("parsing GitHub issue dependency response")?;
            if value.as_array().is_some_and(|issues| {
                issues.iter().any(|issue| {
                    issue["id"].as_u64().map(|id| id.to_string()).as_deref()
                        == Some(dependency.id.as_str())
                })
            }) {
                return Ok(());
            }

            let out = provider_command("gh")
                .args([
                    "api",
                    "--method",
                    "POST",
                    &endpoint,
                    "-F",
                    &format!("issue_id={}", dependency.id),
                ])
                .output()
                .context("gh api add GitHub issue dependency")?;
            if out.status.success()
                || redacted_provider_output(&out)
                    .to_ascii_lowercase()
                    .contains("already")
            {
                Ok(())
            } else {
                anyhow::bail!(
                    "gh api add GitHub issue dependency failed for {endpoint}: {}",
                    redacted_provider_output(&out)
                )
            }
        }
        _ => Ok(()),
    }
}
