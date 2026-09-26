use super::{
    gitlab_api, gitlab_find_mr_by_branch, gitlab_project_id, provider_command,
    provider_output_with_transient_retry, redacted_provider_output, Profile, ProviderKind, Result,
};

/// Identifies the provider conversation that owns a comment. Review threads
/// use the source branch because that is the stable review identifier already
/// accepted by the rest of the provider module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentThread<'a> {
    Issue(&'a str),
    Review(&'a str),
}

/// Replace a provider comment body. The body is redacted before it leaves GAH.
pub fn update_comment(
    profile: &Profile,
    thread: CommentThread<'_>,
    comment_id: &str,
    body: &str,
) -> Result<()> {
    ensure_write_allowed(profile, "update_comment")?;
    let comment_id = numeric_id("comment", comment_id)?;
    let body = crate::redact::redact(body);
    match ProviderKind::parse(&profile.provider) {
        Ok(ProviderKind::Github) => mutate_github(profile, comment_id, "PATCH", Some(&body)),
        Ok(ProviderKind::Gitlab) => {
            let endpoint = gitlab_endpoint(profile, thread, comment_id)?;
            gitlab_api(profile, &endpoint, "PUT", &[("body", &body)])?;
            Ok(())
        }
        Err(_) => anyhow::bail!("unsupported provider: {}", profile.provider),
    }
}

/// Delete one provider comment by its provider-assigned ID.
pub fn delete_comment(
    profile: &Profile,
    thread: CommentThread<'_>,
    comment_id: &str,
) -> Result<()> {
    ensure_write_allowed(profile, "delete_comment")?;
    let comment_id = numeric_id("comment", comment_id)?;
    match ProviderKind::parse(&profile.provider) {
        Ok(ProviderKind::Github) => mutate_github(profile, comment_id, "DELETE", None),
        Ok(ProviderKind::Gitlab) => {
            let endpoint = gitlab_endpoint(profile, thread, comment_id)?;
            gitlab_api(profile, &endpoint, "DELETE", &[])?;
            Ok(())
        }
        Err(_) => anyhow::bail!("unsupported provider: {}", profile.provider),
    }
}

fn ensure_write_allowed(profile: &Profile, operation: &str) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: {operation} is disallowed in handoff mode");
    }
    Ok(())
}

fn numeric_id<'a>(label: &str, value: &'a str) -> Result<&'a str> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        anyhow::bail!("invalid {label} id '{value}': expected digits only");
    }
    Ok(value)
}

fn gitlab_endpoint(
    profile: &Profile,
    thread: CommentThread<'_>,
    comment_id: &str,
) -> Result<String> {
    let project_id = gitlab_project_id(profile)?;
    let (resource, number) = match thread {
        CommentThread::Issue(number) => ("issues", numeric_id("issue", number)?.to_string()),
        CommentThread::Review(branch) => (
            "merge_requests",
            gitlab_find_mr_by_branch(profile, branch)?.id,
        ),
    };
    Ok(format!(
        "projects/{project_id}/{resource}/{number}/notes/{comment_id}"
    ))
}

fn mutate_github(
    profile: &Profile,
    comment_id: &str,
    method: &'static str,
    body: Option<&str>,
) -> Result<()> {
    let endpoint = format!("repos/{}/issues/comments/{comment_id}", profile.repo);
    let operation = if method == "DELETE" {
        "delete provider comment"
    } else {
        "update provider comment"
    };
    let out = provider_output_with_transient_retry(operation, || {
        let mut command = provider_command("gh");
        command.args(["api", "--method", method, &endpoint]);
        if let Some(body) = body {
            command.args(["--raw-field", &format!("body={body}")]);
        }
        command
    })?;
    if !out.status.success() {
        anyhow::bail!("{operation} failed: {}", redacted_provider_output(&out));
    }
    Ok(())
}
