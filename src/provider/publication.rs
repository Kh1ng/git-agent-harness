use super::{
    github_find_pr_number_by_branch, github_set_review_state_labels, gitlab_api,
    gitlab_find_mr_by_branch, gitlab_hostname, gitlab_mr_result_from_value, gitlab_project_id,
    gitlab_set_review_state_labels, provider_command, provider_network_retry_backoff,
    provider_output_with_transient_retry, redacted_provider_output, redacted_stderr, MrResult,
    Profile, ProviderKind, Result, PROVIDER_NETWORK_ATTEMPTS, PROVIDER_TITLE_MAX_CHARS,
};
use anyhow::Context;
use std::sync::OnceLock;
use std::thread;

#[cfg(test)]
thread_local! {
    /// Per-thread configured-home override for publication filter tests.
    /// Thread-local for the same reason as provider's PATH override: HOME is
    /// process-global, and mutating it would corrupt unrelated tests that
    /// run in parallel with this one.
    pub(super) static TEST_HOME_OVERRIDE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// The operator's configured home directory. A home can live outside the
/// /home, /Users, and /root roots (for example /var/home/op), so the
/// publication filter must redact this directory itself rather than trust
/// those hardcoded roots, which stay only as defense-in-depth.
fn operator_home() -> Option<String> {
    #[cfg(test)]
    {
        let overridden = TEST_HOME_OVERRIDE.with(|home| home.borrow().clone());
        if overridden.is_some() {
            return overridden;
        }
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| home.to_string_lossy().into_owned())
}

fn standard_home_path_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r#"(?:/home/[^/\s`"'<>]+|/Users/[^/\s`"'<>]+|/root\b)(?:/[^\s`"'<>]*)?"#)
            .expect("valid home path regex")
    })
}

/// Redact the configured home directory and anything beneath it. A
/// trailing-slash home (or a bare `/`) matches nothing rather than every
/// absolute path in the text.
fn configured_home_path_re(home: &str) -> Option<regex::Regex> {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return None;
    }
    regex::Regex::new(&format!(r#"{}(?:/[^\s`"'<>]*)?"#, regex::escape(home))).ok()
}

/// Keep machine-local home paths out of provider-visible text, including
/// agent summaries copied into PR bodies and issue comments. The
/// operator's actual home directory is redacted first; the common /home,
/// /Users, and /root roots then remain as defense-in-depth.
pub(super) fn publication_body(body: &str) -> String {
    let redacted = crate::redact::redact(body);
    // Web URL paths can resemble local home paths. Apply both home filters
    // only outside HTTP(S) links; file URLs still contain local paths.
    static WEB_URL_RE: OnceLock<regex::Regex> = OnceLock::new();
    let web_url_re = WEB_URL_RE.get_or_init(|| {
        regex::Regex::new(r#"(?i)https?://[^\s`"'<>]+"#).expect("valid web URL regex")
    });
    let home_re = operator_home().as_deref().and_then(configured_home_path_re);
    let redact_paths = |text: &str| {
        let text = match &home_re {
            Some(re) => re.replace_all(text, "[local path removed]"),
            None => std::borrow::Cow::Borrowed(text),
        };
        standard_home_path_re()
            .replace_all(&text, "[local path removed]")
            .into_owned()
    };
    let mut published = String::with_capacity(redacted.len());
    let mut offset = 0;
    for url in web_url_re.find_iter(&redacted) {
        published.push_str(&redact_paths(&redacted[offset..url.start()]));
        published.push_str(url.as_str());
        offset = url.end();
    }
    published.push_str(&redact_paths(&redacted[offset..]));
    published
}

pub(super) fn draft_mr_title(title: &str) -> String {
    let prefixed = format!("Draft: {title}");
    if prefixed.chars().count() <= PROVIDER_TITLE_MAX_CHARS {
        return prefixed;
    }

    let keep = PROVIDER_TITLE_MAX_CHARS - 3;
    let mut truncated: String = prefixed.chars().take(keep).collect();
    truncated.push_str("...");
    truncated
}

pub fn create_draft_mr(
    profile: &Profile,
    branch: &str,
    title: &str,
    body: &str,
) -> Result<MrResult> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: create_draft_mr is disallowed in handoff mode");
    }
    let body = publication_body(body);
    match ProviderKind::parse(&profile.provider) {
        Ok(ProviderKind::Gitlab) => gitlab_mr(profile, branch, title, &body),
        Ok(ProviderKind::Github) => github_mr(profile, branch, title, &body),
        Err(_) => anyhow::bail!("unsupported provider: {}", profile.provider),
    }
}

pub fn post_review_comment(
    profile: &Profile,
    branch: &str,
    body: &str,
    labels: &[&str],
) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: post_review_comment is disallowed in handoff mode");
    }
    let body = publication_body(body);
    match ProviderKind::parse(&profile.provider) {
        Ok(ProviderKind::Gitlab) => gitlab_post_review_comment(profile, branch, &body, labels),
        Ok(ProviderKind::Github) => github_post_review_comment(profile, branch, &body, labels),
        Err(_) => anyhow::bail!("unsupported provider: {}", profile.provider),
    }
}

/// Post an idempotent comment to a source issue using the configured provider.
/// The body is redacted before it crosses the provider boundary.
pub fn post_issue_comment(profile: &Profile, issue_number: &str, body: &str) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: post_issue_comment is disallowed in handoff mode");
    }
    let body = publication_body(body);
    match ProviderKind::parse(&profile.provider) {
        Ok(ProviderKind::Github) => github_post_issue_comment(profile, issue_number, &body),
        Ok(ProviderKind::Gitlab) => {
            let project_id = gitlab_project_id(profile)?;
            let endpoint = format!("projects/{project_id}/issues/{issue_number}/notes");
            let existing = gitlab_api(profile, &endpoint, "GET", &[])?;
            if existing.as_array().is_some_and(|notes| {
                notes
                    .iter()
                    .any(|note| note["body"].as_str() == Some(body.as_str()))
            }) {
                return Ok(());
            }
            gitlab_api(profile, &endpoint, "POST", &[("body", &body)])?;
            Ok(())
        }
        Err(_) => anyhow::bail!("unsupported provider: {}", profile.provider),
    }
}

fn gitlab_mr(profile: &Profile, branch: &str, title: &str, body: &str) -> Result<MrResult> {
    let api_base = profile
        .provider_api_base
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_api_base for gitlab"))?;
    let project_id = gitlab_project_id(profile)?;
    let hostname = gitlab_hostname(api_base)?;
    let endpoint = format!("projects/{project_id}/merge_requests");
    let source_branch = format!("source_branch={branch}");
    let target_branch = format!("target_branch={}", profile.default_target_branch);
    // Apply the provider boundary after adding the draft prefix. Truncating
    // the unprefixed title first can still produce an invalid provider value.
    let title = format!("title={}", draft_mr_title(title));
    let description = format!("description={body}");

    // Use the same host-scoped provider CLI session that `gah doctor`
    // validates. Requiring a second GITLAB_PAT environment variable here made
    // a doctor-clean, authenticated profile fail publication with HTTP 401.
    let out = provider_command("glab")
        .args([
            "api",
            &endpoint,
            "--hostname",
            hostname,
            "--method",
            "POST",
            "--raw-field",
            &source_branch,
            "--raw-field",
            &target_branch,
            "--raw-field",
            &title,
            "--raw-field",
            &description,
        ])
        .output()
        .context("glab api gitlab create mr")?;

    if !out.status.success() {
        anyhow::bail!(
            "glab api gitlab create mr failed: {}",
            redacted_provider_output(&out)
        );
    }

    let resp: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing gitlab MR response")?;
    gitlab_mr_result_from_value(&resp)
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
            &draft_mr_title(title),
            "--body",
            body,
            "--draft",
        ])
        .output()
        .context("gh pr create")?;

    if !out.status.success() {
        anyhow::bail!("gh pr create failed: {}", redacted_stderr(&out));
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
    let project_id = gitlab_project_id(profile)?;
    let mr = gitlab_find_mr_by_branch(profile, branch)?;
    let endpoint = format!("projects/{project_id}/merge_requests/{}/notes", mr.id);
    gitlab_api(profile, &endpoint, "POST", &[("body", body)])?;
    gitlab_set_review_state_labels(profile, &mr.id, labels)
        .with_context(|| format!("applying review labels to MR {}", mr.id))?;
    Ok(())
}

fn github_post_review_comment(
    profile: &Profile,
    branch: &str,
    body: &str,
    labels: &[&str],
) -> Result<()> {
    let pr_number = github_find_pr_number_by_branch(profile, branch)?;
    github_post_issue_comment(profile, &pr_number, body)
        .with_context(|| format!("posting review comment to PR {}", pr_number))?;
    github_set_review_state_labels(profile, &pr_number, labels)
        .with_context(|| format!("applying review labels to PR {}", pr_number))?;
    Ok(())
}

fn github_post_issue_comment(profile: &Profile, pr_number: &str, body: &str) -> Result<()> {
    let endpoint = format!("repos/{}/issues/{pr_number}/comments", profile.repo);

    // A timed-out POST may still have reached GitHub. Check for this exact
    // run's rendered comment before every POST attempt so retrying transport
    // failures does not normally duplicate review comments.
    for attempt in 1..=PROVIDER_NETWORK_ATTEMPTS {
        let existing =
            provider_output_with_transient_retry("read existing review comments", || {
                let mut command = provider_command("gh");
                command.args(["api", "--method", "GET", &endpoint, "-f", "per_page=100"]);
                command
            })?;
        if !existing.status.success() {
            anyhow::bail!(
                "reading existing review comments failed: {}",
                redacted_provider_output(&existing)
            );
        }
        let comments: serde_json::Value = serde_json::from_slice(&existing.stdout)
            .context("parsing existing GitHub review comments")?;
        if comments.as_array().is_some_and(|comments| {
            comments
                .iter()
                .any(|comment| comment["body"].as_str() == Some(body))
        }) {
            return Ok(());
        }

        let mut command = provider_command("gh");
        command.args([
            "api",
            "--method",
            "POST",
            &endpoint,
            "--raw-field",
            &format!("body={body}"),
        ]);
        let post = command
            .output()
            .context("launching provider operation post review comment")?;
        if post.status.success() {
            return Ok(());
        }
        let output = redacted_provider_output(&post);
        if attempt < PROVIDER_NETWORK_ATTEMPTS
            && crate::worktree::is_transient_network_error(&output)
        {
            eprintln!(
                "transient provider network failure during post review comment; retrying {}/{} after {}s: {}",
                attempt + 1,
                PROVIDER_NETWORK_ATTEMPTS,
                provider_network_retry_backoff().as_secs(),
                output
            );
            thread::sleep(provider_network_retry_backoff());
            continue;
        }
        anyhow::bail!("posting review comment failed: {}", output);
    }
    unreachable!("bounded GitHub comment retry loop always returns")
}
