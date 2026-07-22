use crate::config::Profile;
use anyhow::{Context, Result};
use std::fmt;
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;
use url::Url;

mod relations;
pub(crate) use relations::{link_provider_child, link_provider_dependency};

const GAH_REVIEW_STATE_LABELS: [&str; 5] = [
    "gah-needs-fix",
    "gah-ready-for-human",
    "gah-human-review",
    "gah-review-weak",
    "gah-review-escalating",
];
const PROVIDER_ERROR_MAX_CHARS: usize = 4_096;
const PROVIDER_MR_TITLE_MAX_CHARS: usize = 255;
const PROVIDER_NETWORK_ATTEMPTS: u8 = 3;
#[cfg(not(test))]
const PROVIDER_NETWORK_RETRY_BACKOFF: Duration = Duration::from_secs(2);

fn provider_network_retry_backoff() -> Duration {
    #[cfg(test)]
    {
        Duration::ZERO
    }
    #[cfg(not(test))]
    {
        PROVIDER_NETWORK_RETRY_BACKOFF
    }
}

/// Run a provider CLI operation with a small, bounded retry only for
/// transport weather. Authentication, authorization, schema, and ordinary
/// provider errors return immediately so unattended loops cannot hide a real
/// configuration problem behind repeated writes.
fn provider_output_with_transient_retry(
    operation: &str,
    mut command: impl FnMut() -> Command,
) -> Result<Output> {
    for attempt in 1..=PROVIDER_NETWORK_ATTEMPTS {
        let out = command()
            .output()
            .with_context(|| format!("launching provider operation {operation}"))?;
        if out.status.success()
            || attempt == PROVIDER_NETWORK_ATTEMPTS
            || !crate::worktree::is_transient_network_error(&redacted_provider_output(&out))
        {
            return Ok(out);
        }

        eprintln!(
            "transient provider network failure during {operation}; retrying {}/{} after {}s: {}",
            attempt + 1,
            PROVIDER_NETWORK_ATTEMPTS,
            provider_network_retry_backoff().as_secs(),
            redacted_provider_output(&out)
        );
        thread::sleep(provider_network_retry_backoff());
    }
    unreachable!("bounded provider retry loop always returns")
}

fn draft_mr_title(title: &str) -> String {
    let prefixed = format!("Draft: {title}");
    if prefixed.chars().count() <= PROVIDER_MR_TITLE_MAX_CHARS {
        return prefixed;
    }

    let keep = PROVIDER_MR_TITLE_MAX_CHARS - 3;
    let mut truncated: String = prefixed.chars().take(keep).collect();
    truncated.push_str("...");
    truncated
}

fn redacted_stderr(out: &Output) -> String {
    crate::redact::redact(&String::from_utf8_lossy(&out.stderr))
        .trim()
        .to_string()
}

fn redacted_provider_output(out: &Output) -> String {
    crate::redact::redact(&format!(
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    ))
    .trim()
    .chars()
    .take(PROVIDER_ERROR_MAX_CHARS)
    .collect()
}

fn gitlab_hostname(api_base: &str) -> Result<&str> {
    let without_scheme = api_base
        .strip_prefix("https://")
        .or_else(|| api_base.strip_prefix("http://"))
        .ok_or_else(|| anyhow::anyhow!("invalid GitLab provider_api_base: expected http(s) URL"))?;
    let hostname = without_scheme.split('/').next().unwrap_or_default().trim();
    if hostname.is_empty() {
        anyhow::bail!("invalid GitLab provider_api_base: missing hostname");
    }
    Ok(hostname)
}

pub(crate) fn gitlab_api(
    profile: &Profile,
    endpoint: &str,
    method: &str,
    raw_fields: &[(&str, &str)],
) -> Result<serde_json::Value> {
    let api_base = profile
        .provider_api_base
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_api_base for gitlab"))?;
    let hostname = gitlab_hostname(api_base)?;
    let mut command = provider_command("glab");
    command
        .arg("api")
        .arg(endpoint)
        .args(["--hostname", hostname, "--method", method]);
    for (name, value) in raw_fields {
        command.args(["--raw-field", &format!("{name}={value}")]);
    }
    let out = command.output().context("glab api GitLab request")?;
    if !out.status.success() {
        anyhow::bail!(
            "glab api GitLab request failed for {}: {}",
            endpoint,
            redacted_provider_output(&out)
        );
    }
    serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing GitLab API response for {endpoint}"))
}

#[cfg(test)]
thread_local! {
    /// Per-thread PATH override for provider CLI tests. Thread-local (not a
    /// process-global env var) so parallel tests in other modules that need
    /// the real PATH (git, sh, ...) are never affected — see
    /// tests::PathOverride below for why this exists.
    static TEST_PATH_OVERRIDE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[allow(dead_code)]
/// Integration tests that use `provider_command` can set a thread-local PATH
/// override so fake provider scripts (`gh`, `glab`) resolve from a temp
/// directory without mutating the process-global PATH.
pub fn set_test_provider_path(path: &str) {
    TEST_PATH_OVERRIDE.with(|p| *p.borrow_mut() = Some(path.to_string()));
}

#[cfg(test)]
#[allow(dead_code)]
pub fn clear_test_provider_path() {
    TEST_PATH_OVERRIDE.with(|p| *p.borrow_mut() = None);
}

/// Construct a Command for an external provider CLI (`gh`, `glab`). In test
/// builds, honors a thread-local PATH override so tests can hide/replace
/// these binaries without touching the process-wide PATH.
pub fn provider_command(name: &str) -> Command {
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

/// Secret-safe provider issue identity used by PM publication. `id` is the
/// provider's opaque database/node identifier; `number` is the project-local
/// issue number operators see in URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderIssue {
    pub(crate) id: String,
    pub(crate) number: String,
    pub(crate) url: String,
    pub(crate) state: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) labels: Vec<String>,
}

fn provider_issue_from_value(provider: &str, value: &serde_json::Value) -> Result<ProviderIssue> {
    let (number, url, body) = match provider {
        "github" => (
            value["number"].as_u64().map(|v| v.to_string()),
            value["html_url"].as_str(),
            value["body"].as_str().unwrap_or_default(),
        ),
        "gitlab" => (
            value["iid"].as_u64().map(|v| v.to_string()),
            value["web_url"].as_str(),
            value["description"].as_str().unwrap_or_default(),
        ),
        other => anyhow::bail!("unsupported provider: {other}"),
    };
    let id = value["id"]
        .as_u64()
        .map(|v| v.to_string())
        .or_else(|| value["id"].as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow::anyhow!("provider issue response missing id"))?;
    let labels = value["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| {
                    label
                        .as_str()
                        .or_else(|| label["name"].as_str())
                        .map(ToOwned::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ProviderIssue {
        id,
        number: number.ok_or_else(|| anyhow::anyhow!("provider issue response missing number"))?,
        url: url
            .ok_or_else(|| anyhow::anyhow!("provider issue response missing URL"))?
            .to_string(),
        state: value["state"].as_str().unwrap_or("unknown").to_string(),
        title: value["title"].as_str().unwrap_or_default().to_string(),
        body: body.to_string(),
        labels,
    })
}

fn github_json_api(
    _profile: &Profile,
    method: &str,
    endpoint: &str,
    fields: &[(&str, &str)],
) -> Result<serde_json::Value> {
    let mut command = provider_command("gh");
    command.args(["api", "--method", method, endpoint]);
    for (name, value) in fields {
        command.args(["-f", &format!("{name}={value}")]);
    }
    let out = command.output().context("gh api issue operation")?;
    if !out.status.success() {
        anyhow::bail!(
            "gh api issue operation failed for {endpoint}: {}",
            redacted_provider_output(&out)
        );
    }
    serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing GitHub API response for {endpoint}"))
}

pub(crate) fn get_provider_issue(profile: &Profile, number: &str) -> Result<ProviderIssue> {
    let value = match profile.provider.as_str() {
        "github" => github_json_api(
            profile,
            "GET",
            &format!("repos/{}/issues/{number}", profile.repo),
            &[],
        )?,
        "gitlab" => {
            let project_id = profile
                .provider_project_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
            gitlab_api(
                profile,
                &format!("projects/{project_id}/issues/{number}"),
                "GET",
                &[],
            )?
        }
        other => anyhow::bail!("unsupported provider: {other}"),
    };
    provider_issue_from_value(&profile.provider, &value)
}

pub(crate) fn list_provider_issues(profile: &Profile) -> Result<Vec<ProviderIssue>> {
    const PAGE_SIZE: usize = 100;
    const MAX_PAGES: usize = 100;
    let mut issues = Vec::new();
    for page in 1..=MAX_PAGES {
        let value = match profile.provider.as_str() {
            "github" => github_json_api(
                profile,
                "GET",
                &format!(
                    "repos/{}/issues?state=all&per_page={PAGE_SIZE}&page={page}",
                    profile.repo
                ),
                &[],
            )?,
            "gitlab" => {
                let project_id = profile.provider_project_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("profile missing provider_project_id for gitlab")
                })?;
                gitlab_api(
                    profile,
                    &format!(
                        "projects/{project_id}/issues?scope=all&state=all&per_page={PAGE_SIZE}&page={page}"
                    ),
                    "GET",
                    &[],
                )?
            }
            other => anyhow::bail!("unsupported provider: {other}"),
        };
        let page_values = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("provider issue listing was not an array"))?;
        for value in page_values {
            if profile.provider == "github" && value.get("pull_request").is_some() {
                continue;
            }
            issues.push(provider_issue_from_value(&profile.provider, value)?);
        }
        if page_values.len() < PAGE_SIZE {
            return Ok(issues);
        }
    }
    anyhow::bail!(
        "provider issue listing reached {} pages; refusing an incomplete idempotency snapshot",
        MAX_PAGES
    )
}

pub(crate) fn list_provider_label_names(profile: &Profile) -> Result<Vec<String>> {
    const PAGE_SIZE: usize = 100;
    const MAX_PAGES: usize = 100;
    let mut labels = Vec::new();
    for page in 1..=MAX_PAGES {
        let value = match profile.provider.as_str() {
            "github" => github_json_api(
                profile,
                "GET",
                &format!(
                    "repos/{}/labels?per_page={PAGE_SIZE}&page={page}",
                    profile.repo
                ),
                &[],
            )?,
            "gitlab" => {
                let project_id = profile.provider_project_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("profile missing provider_project_id for gitlab")
                })?;
                gitlab_api(
                    profile,
                    &format!("projects/{project_id}/labels?per_page={PAGE_SIZE}&page={page}"),
                    "GET",
                    &[],
                )?
            }
            other => anyhow::bail!("unsupported provider: {other}"),
        };
        let page_values = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("provider label listing was not an array"))?;
        labels.extend(
            page_values
                .iter()
                .filter_map(|value| value["name"].as_str().map(ToOwned::to_owned)),
        );
        if page_values.len() < PAGE_SIZE {
            return Ok(labels);
        }
    }
    anyhow::bail!(
        "provider label listing reached {} pages; refusing an incomplete taxonomy snapshot",
        MAX_PAGES
    )
}

pub(crate) fn create_provider_issue(
    profile: &Profile,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<ProviderIssue> {
    let title = crate::redact::redact(title);
    let body = crate::redact::redact(body);
    let value = match profile.provider.as_str() {
        "github" => {
            let mut fields = vec![("title", title.as_str()), ("body", body.as_str())];
            fields.extend(labels.iter().map(|label| ("labels[]", label.as_str())));
            github_json_api(
                profile,
                "POST",
                &format!("repos/{}/issues", profile.repo),
                &fields,
            )?
        }
        "gitlab" => {
            let project_id = profile
                .provider_project_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
            let labels = labels.join(",");
            let mut fields = vec![("title", title.as_str()), ("description", body.as_str())];
            if !labels.is_empty() {
                fields.push(("labels", labels.as_str()));
            }
            gitlab_api(
                profile,
                &format!("projects/{project_id}/issues"),
                "POST",
                &fields,
            )?
        }
        other => anyhow::bail!("unsupported provider: {other}"),
    };
    provider_issue_from_value(&profile.provider, &value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewTarget {
    pub id: String,
    pub url: String,
    pub source_branch: String,
    pub target_branch: String,
    pub state: Option<String>,
    pub merged_at: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub draft: bool,
    pub ci_status: Option<String>,
    /// GitLab mergeability state (for the same target), kept separate from
    /// CI/pipeline status so draft/mergeability isn't misread as `ci:passed`.
    pub merge_status: Option<String>,
    /// Immutable source commit that the reviewer inspected. Optional so
    /// older provider responses and local/fallback targets remain valid.
    pub source_sha: Option<String>,
    /// Immutable target/base commit used to construct the reviewed diff.
    pub target_sha: Option<String>,
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
    let body = crate::redact::redact(body);
    match profile.provider.as_str() {
        "gitlab" => gitlab_mr(profile, branch, title, &body),
        "github" => github_mr(profile, branch, title, &body),
        other => anyhow::bail!("unsupported provider: {}", other),
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
    let body = crate::redact::redact(body);
    match profile.provider.as_str() {
        "gitlab" => gitlab_post_review_comment(profile, branch, &body, labels),
        "github" => github_post_review_comment(profile, branch, &body, labels),
        other => anyhow::bail!("unsupported provider: {}", other),
    }
}

/// Post an idempotent comment to a source issue using the configured provider.
/// The body is redacted before it crosses the provider boundary.
pub fn post_issue_comment(profile: &Profile, issue_number: &str, body: &str) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: post_issue_comment is disallowed in handoff mode");
    }
    let body = crate::redact::redact(body);
    match profile.provider.as_str() {
        "github" => github_post_issue_comment(profile, issue_number, &body),
        "gitlab" => {
            let project_id = profile
                .provider_project_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
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
        other => anyhow::bail!("unsupported provider: {other}"),
    }
}

/// Replace the mutually-exclusive GAH review/controller labels while
/// preserving every unrelated provider label. Review state is a state
/// machine, not an append-only set: leaving `gah-needs-fix` attached after a
/// repair makes it outrank `gah-review-escalating` forever and exhausts the
/// fix cap without reviewing the new source commit.
pub fn set_review_state_labels(profile: &Profile, branch: &str, labels: &[&str]) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!(
            "delivery_mode=handoff: set_review_state_labels is disallowed in handoff mode"
        );
    }
    match profile.provider.as_str() {
        "gitlab" => {
            let mr = gitlab_find_mr_by_branch(profile, branch)?;
            gitlab_set_review_state_labels(profile, &mr.id, labels)
        }
        "github" => {
            let pr_number = github_find_pr_number_by_branch(profile, branch)?;
            github_set_review_state_labels(profile, &pr_number, labels)
        }
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

/// Resolve an explicit `--mr` dispatch to its review target. `mr` accepts
/// either a bare numeric IID/PR number or the canonical provider MR/PR URL
/// operators already receive in notifications and status output; GitHub's
/// `gh pr view` accepts both natively, GitLab's IID-keyed API requires
/// normalizing the URL first (see `parse_gitlab_mr_reference`).
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
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
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

fn gitlab_set_review_state_labels(profile: &Profile, iid: &str, labels: &[&str]) -> Result<()> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/merge_requests/{iid}");
    let remove_labels = GAH_REVIEW_STATE_LABELS
        .iter()
        .copied()
        .filter(|candidate| !labels.contains(candidate))
        .collect::<Vec<_>>()
        .join(",");
    gitlab_api(
        profile,
        &endpoint,
        "PUT",
        &[
            ("add_labels", &labels.join(",")),
            ("remove_labels", &remove_labels),
        ],
    )?;
    Ok(())
}

fn github_set_review_state_labels(
    profile: &Profile,
    pr_number: &str,
    labels: &[&str],
) -> Result<()> {
    // `gh pr edit` uses a GraphQL mutation that fails on repositories with
    // the retired Projects (classic) surface. Keep this state transition on
    // the REST issue-label endpoints used successfully by review publishing.
    let endpoint = format!("repos/{}/issues/{}/labels", profile.repo, pr_number);
    let current = provider_output_with_transient_retry("read current review labels", || {
        let mut command = provider_command("gh");
        command.args(["api", &endpoint, "--jq", ".[].name"]);
        command
    })?;
    if !current.status.success() {
        anyhow::bail!(
            "reading current review labels failed: {}",
            redacted_stderr(&current)
        );
    }
    // Let `gh` parse provider JSON and emit one name per line. An empty,
    // successful response is the valid "no labels" state; malformed provider
    // JSON still makes `gh api` fail before this point.
    let current_text = String::from_utf8_lossy(&current.stdout);
    let current = current_text
        .lines()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .collect::<std::collections::HashSet<_>>();

    for stale in GAH_REVIEW_STATE_LABELS
        .iter()
        .copied()
        .filter(|label| current.contains(label) && !labels.contains(label))
    {
        let stale_endpoint = format!("{endpoint}/{stale}");
        let remove = provider_output_with_transient_retry("remove stale review label", || {
            let mut command = provider_command("gh");
            command.args(["api", "--method", "DELETE", &stale_endpoint]);
            command
        })?;
        if !remove.status.success() {
            anyhow::bail!(
                "removing stale review label {stale} failed: {}",
                redacted_stderr(&remove)
            );
        }
    }

    let missing = labels
        .iter()
        .copied()
        .filter(|label| !current.contains(label))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let mut args = vec!["api".to_string(), endpoint];
        for label in missing {
            args.push("-f".to_string());
            args.push(format!("labels[]={label}"));
        }
        let add = provider_output_with_transient_retry("add review labels", || {
            let mut command = provider_command("gh");
            command.args(&args);
            command
        })?;
        if !add.status.success() {
            anyhow::bail!("adding review labels failed: {}", redacted_stderr(&add));
        }
    }
    Ok(())
}

/// TICKET-127: un-draft then merge the MR/PR for `branch`. gah always
/// creates MRs as drafts, so both providers require an explicit
/// "ready"/un-draft step before their merge endpoint will accept the MR.
/// GitLab uses the same `glab api` path as the rest of the adapter and clears
/// any draft-style title prefix before updating the MR title.
pub fn mark_ready_for_review(profile: &Profile, branch: &str) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: mark_ready_for_review is disallowed in handoff mode");
    }
    let target = find_review_target_by_branch(profile, branch)?;
    match profile.provider.as_str() {
        "gitlab" => {
            let title = target.title.as_deref().ok_or_else(|| {
                anyhow::anyhow!("GitLab MR missing title for ready-for-review transition")
            })?;
            gitlab_mark_ready_for_review(profile, &target.id, title)
        }
        "github" => github_mark_ready_for_review(profile, &target.id),
        other => anyhow::bail!("unsupported provider: {}", other),
    }
}

fn ensure_review_generation(target: &ReviewTarget, expected: Option<&str>) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    // Tolerates GAH's own one-way draft-to-ready transition (mark_ready_for_review
    // runs before merge_mr and flips `draft`, which would otherwise invalidate
    // every review's generation right before merge — see review_state.rs).
    if !crate::sync::review_generation_matches(
        expected,
        target.source_sha.as_deref(),
        target.title.as_deref(),
        target.body.as_deref(),
        target.draft,
    ) {
        let metadata_fingerprint = crate::sync::review_metadata_fingerprint(
            target.source_sha.as_deref(),
            target.title.as_deref(),
            target.body.as_deref(),
            target.draft,
        );
        let live = crate::ledger::review_generation(
            target.source_sha.as_deref(),
            Some(&metadata_fingerprint),
        );
        anyhow::bail!(
            "MR source or metadata changed after review: expected generation '{expected}', live generation is '{}'; re-run review before merge",
            live.as_deref().unwrap_or("unknown")
        );
    }
    Ok(())
}

pub fn merge_mr(
    profile: &Profile,
    branch: &str,
    expected_review_generation: Option<&str>,
) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: merge_mr is disallowed in handoff mode");
    }
    let target = find_review_target_by_branch(profile, branch)?;
    ensure_review_generation(&target, expected_review_generation)?;
    match profile.provider.as_str() {
        "gitlab" => {
            let title = target.title.as_deref().ok_or_else(|| {
                anyhow::anyhow!("GitLab MR missing title for ready-for-review transition")
            })?;
            gitlab_mark_ready_for_review(profile, &target.id, title)?;
            gitlab_merge_mr(profile, &target.id, target.source_sha.as_deref())
        }
        "github" => {
            github_mark_ready_for_review(profile, &target.id)?;
            github_merge_mr(profile, &target.id, target.source_sha.as_deref())
        }
        other => anyhow::bail!("unsupported provider: {}", other),
    }
}

fn gitlab_ready_title(title: &str) -> &str {
    title
        .strip_prefix("Draft: ")
        .or_else(|| title.strip_prefix("[Draft] "))
        .or_else(|| title.strip_prefix("(Draft) "))
        .unwrap_or(title)
}

fn gitlab_mark_ready_for_review(profile: &Profile, iid: &str, title: &str) -> Result<()> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/merge_requests/{iid}");
    let ready_title = gitlab_ready_title(title);
    gitlab_api(profile, &endpoint, "PUT", &[("title", ready_title)])?;
    Ok(())
}

fn gitlab_merge_mr(profile: &Profile, iid: &str, source_sha: Option<&str>) -> Result<()> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/merge_requests/{iid}/merge");
    let mut fields = vec![("squash", "true"), ("should_remove_source_branch", "true")];
    if let Some(source_sha) = source_sha {
        fields.push(("sha", source_sha));
    }
    gitlab_api(profile, &endpoint, "PUT", &fields)?;
    Ok(())
}

/// Issue #124 / TICKET-127: set GitLab's "merge when pipeline succeeds" (MWPS)
/// flag on the MR and return without merging. GitLab then enforces the CI gate
/// natively: the MR only merges once its pipeline turns green. Used by the
/// `gitlab_mwps` merge policy so GAH does not merge the MR itself.
pub fn gitlab_set_mwps(
    profile: &Profile,
    branch: &str,
    expected_review_generation: &str,
) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: gitlab_set_mwps is disallowed in handoff mode");
    }
    let target = find_review_target_by_branch(profile, branch)?;
    ensure_review_generation(&target, Some(expected_review_generation))?;
    let title = target.title.as_deref().ok_or_else(|| {
        anyhow::anyhow!("GitLab MR missing title for ready-for-review transition")
    })?;
    gitlab_mark_ready_for_review(profile, &target.id, title)?;
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/merge_requests/{}/merge", target.id);
    let mut fields = vec![
        ("auto_merge", "true"),
        ("squash", "true"),
        ("should_remove_source_branch", "true"),
    ];
    if let Some(source_sha) = target.source_sha.as_deref() {
        fields.push(("sha", source_sha));
    }
    gitlab_api(profile, &endpoint, "PUT", &fields)?;
    Ok(())
}

/// Close a GitHub issue by number.
pub fn github_close_issue(profile: &Profile, issue_number: &str) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: github_close_issue is disallowed in handoff mode");
    }
    let out = provider_command("gh")
        .args(["issue", "close", issue_number, "--repo", &profile.repo])
        .output()
        .context("gh issue close")?;

    if !out.status.success() {
        anyhow::bail!("gh issue close failed: {}", redacted_stderr(&out));
    }
    Ok(())
}

/// Close a GitLab issue by number.
pub fn gitlab_close_issue(profile: &Profile, issue_number: &str) -> Result<()> {
    if profile.delivery_mode == crate::config::DeliveryMode::Handoff {
        anyhow::bail!("delivery_mode=handoff: gitlab_close_issue is disallowed in handoff mode");
    }
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/issues/{issue_number}");
    gitlab_api(profile, &endpoint, "PUT", &[("state_event", "close")])?;

    Ok(())
}

/// Get the state of a GitHub issue.
pub fn github_get_issue_state(profile: &Profile, issue_number: &str) -> Result<Option<String>> {
    let out = provider_command("gh")
        .args([
            "api",
            &format!("repos/{}/issues/{}", profile.repo, issue_number),
            "--jq",
            ".state",
        ])
        .output()
        .context("gh api get issue state")?;

    if !out.status.success() {
        return Ok(None);
    }

    let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(if state.is_empty() { None } else { Some(state) })
}

/// Get the state of a GitLab issue.
pub fn gitlab_get_issue_state(profile: &Profile, issue_number: &str) -> Result<Option<String>> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/issues/{issue_number}");
    let resp = gitlab_api(profile, &endpoint, "GET", &[])?;
    let state = resp["state"].as_str().map(|s| s.to_string());
    Ok(state)
}

fn github_merge_mr(profile: &Profile, number: &str, source_sha: Option<&str>) -> Result<()> {
    // --admin: gah's own review step only ever posts a plain issue comment,
    // never a formal GitHub PR review (GitHub disallows self-approval, and
    // gah's PRs are authored by the same identity that would review them) --
    // so branch protection's required-approving-review count can never be
    // satisfied by gah itself. `decide_next_action`'s review-verdict/CI gate
    // has already decided this merge is safe by the time we get here; this
    // flag just lets that decision execute instead of failing with "the base
    // branch policy prohibits the merge" (confirmed live on PR #255).
    let mut args = vec!["pr", "merge", number, "--squash", "--admin"];
    if let Some(source_sha) = source_sha {
        args.extend(["--match-head-commit", source_sha]);
    }
    args.extend(["--delete-branch", "--repo", &profile.repo]);
    let merge = provider_command("gh")
        .args(args)
        .output()
        .context("gh pr merge")?;
    if !merge.status.success() {
        anyhow::bail!(
            "gh pr merge failed: {}",
            String::from_utf8_lossy(&merge.stderr).trim()
        );
    }
    Ok(())
}

fn github_mark_ready_for_review(profile: &Profile, number: &str) -> Result<()> {
    let ready = provider_command("gh")
        .args(["pr", "ready", number, "--repo", &profile.repo])
        .output()
        .context("gh pr ready")?;
    if !ready.status.success() {
        anyhow::bail!(
            "gh pr ready failed: {}",
            String::from_utf8_lossy(&ready.stderr).trim()
        );
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

/// Best-effort resolution of the MR/PR URL for a given source branch,
/// across both GitLab and GitHub providers. Returns `None` when no open
/// MR/PR can be resolved (e.g. the branch has no MR yet, or the provider
/// CLI/API is unavailable). Intended for non-fatal enrichment like
/// notifications -- callers must not depend on this succeeding.
pub fn mr_url_for_branch(profile: &Profile, branch: &str) -> Option<String> {
    match profile.provider.as_str() {
        "gitlab" => gitlab_find_mr_by_branch(profile, branch)
            .ok()
            .map(|mr| mr.url),
        _ => github_find_pr_number_by_branch(profile, branch)
            .ok()
            .and_then(|number| {
                format!("https://github.com/{}/pull/{}", profile.repo, number).into()
            }),
    }
}

fn gitlab_review_target_by_branch(profile: &Profile, branch: &str) -> Result<ReviewTarget> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/merge_requests");
    let resp = gitlab_api(
        profile,
        &endpoint,
        "GET",
        &[("state", "opened"), ("source_branch", branch)],
    )?;
    let first = resp
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| anyhow::anyhow!("no open GitLab MR found for branch '{}'", branch))?;
    let mut target = gitlab_target_from_value(first)?;
    // Extract the raw iid string directly from the response value; this
    // ensures the pipelines lookup always uses the raw MR iid even if the
    // representation inside `ReviewTarget::id` is changed in the future.
    let iid = first["iid"].to_string().trim_matches('"').to_string();
    target.ci_status = gitlab_ci_status_for_merge_request(profile, project_id, &iid, first)?;
    Ok(target)
}

/// Explicit `--mr` dispatch typed failure: the operator-supplied MR
/// reference is not a bare IID and does not resolve to this profile's
/// project. Kept distinct from provider API errors so it fails at
/// argument/preflight validation, before any `glab api` call or backend
/// launch.
#[derive(Debug)]
pub enum MrReferenceError {
    MalformedUrl(String),
    CrossProject { expected: String, found: String },
    InvalidProfile(String),
}

impl fmt::Display for MrReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MrReferenceError::MalformedUrl(raw) => write!(
                f,
                "malformed --mr value '{raw}': expected a numeric IID or a canonical GitLab MR URL (http(s)://<host>/<namespace>/<project>/-/merge_requests/<iid>)"
            ),
            MrReferenceError::CrossProject { expected, found } => write!(
                f,
                "--mr URL does not match this profile's project: expected '{expected}', found '{found}'"
            ),
            MrReferenceError::InvalidProfile(reason) => {
                write!(f, "cannot validate --mr URL for this GitLab profile: {reason}")
            }
        }
    }
}

impl std::error::Error for MrReferenceError {}

fn is_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Normalize an explicit `--mr` value to a GitLab IID. Accepts a bare
/// numeric IID unchanged (existing behavior), or a canonical MR URL whose
/// host and project path must match this profile -- anything else is a
/// typed, non-network validation failure. Trailing subpaths (such as `/diffs`),
/// query parameters (`?`), and URL fragments (`#`) following the IID are ignored.
fn parse_gitlab_mr_reference(profile: &Profile, raw: &str) -> Result<String, MrReferenceError> {
    let trimmed = raw.trim();
    if is_ascii_digits(trimmed) {
        return Ok(trimmed.to_string());
    }

    let parsed =
        Url::parse(trimmed).map_err(|_| MrReferenceError::MalformedUrl(raw.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(MrReferenceError::MalformedUrl(raw.to_string()));
    }

    let segments: Vec<_> = parsed
        .path_segments()
        .ok_or_else(|| MrReferenceError::MalformedUrl(raw.to_string()))?
        .filter(|segment| !segment.is_empty())
        .collect();
    let marker = segments
        .windows(2)
        .position(|pair| pair == ["-", "merge_requests"])
        .ok_or_else(|| MrReferenceError::MalformedUrl(raw.to_string()))?;
    let project_path = segments[..marker].join("/");
    let iid = segments
        .get(marker + 2)
        .copied()
        .filter(|value| is_ascii_digits(value))
        .ok_or_else(|| MrReferenceError::MalformedUrl(raw.to_string()))?;

    let api_base = profile
        .provider_api_base
        .as_deref()
        .ok_or_else(|| MrReferenceError::InvalidProfile("missing provider_api_base".to_string()))?;
    let expected = Url::parse(api_base).map_err(|_| {
        MrReferenceError::InvalidProfile("provider_api_base is not a valid URL".to_string())
    })?;
    if !matches!(expected.scheme(), "http" | "https") || expected.host_str().is_none() {
        return Err(MrReferenceError::InvalidProfile(
            "provider_api_base must be an http(s) URL with a hostname".to_string(),
        ));
    }

    let expected_host = expected.host_str().expect("checked above");
    let found_host = parsed
        .host_str()
        .ok_or_else(|| MrReferenceError::MalformedUrl(raw.to_string()))?;
    // The operator-facing web URL may use a different http(s) scheme from
    // the configured API base behind a reverse proxy. The host and any
    // explicitly configured port identify the instance; scheme alone does not.
    let host_matches =
        expected_host.eq_ignore_ascii_case(found_host) && expected.port() == parsed.port();
    let project_matches = project_path == profile.repo;
    if !host_matches || !project_matches {
        let expected_authority = expected.port().map_or_else(
            || expected_host.to_string(),
            |port| format!("{expected_host}:{port}"),
        );
        let found_authority = parsed.port().map_or_else(
            || found_host.to_string(),
            |port| format!("{found_host}:{port}"),
        );
        return Err(MrReferenceError::CrossProject {
            expected: format!("{expected_authority}/{}", profile.repo),
            found: format!("{found_authority}/{project_path}"),
        });
    }

    Ok(iid.to_string())
}

fn gitlab_review_target_by_iid(profile: &Profile, mr: &str) -> Result<ReviewTarget> {
    let iid = parse_gitlab_mr_reference(profile, mr)?;
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/merge_requests/{iid}");
    let resp = gitlab_api(profile, &endpoint, "GET", &[])?;
    let mut target = gitlab_target_from_value(&resp)?;
    // The operator may have supplied a full URL; all provider endpoints must
    // use the normalized IID resolved above.
    target.ci_status = gitlab_ci_status_for_merge_request(profile, project_id, &iid, &resp)?;
    Ok(target)
}

fn gitlab_ci_status_for_merge_request(
    profile: &Profile,
    project_id: &str,
    iid: &str,
    value: &serde_json::Value,
) -> Result<Option<String>> {
    let source_sha = value["sha"].as_str().unwrap_or("");
    if source_sha.is_empty() {
        return Ok(Some("missing".to_string()));
    }

    let head_pipeline = &value["head_pipeline"];
    if head_pipeline["sha"].as_str() == Some(source_sha) {
        if let Some(status) = head_pipeline["status"].as_str() {
            return Ok(Some(normalize_gitlab_ci_status(status)));
        }
    }

    gitlab_pipeline_status_for_sha(profile, project_id, iid, source_sha)?
        .map_or(Ok(Some("missing".to_string())), |status| {
            Ok(Some(normalize_gitlab_ci_status(&status)))
        })
}

fn gitlab_pipeline_status_for_sha(
    profile: &Profile,
    project_id: &str,
    iid: &str,
    source_sha: &str,
) -> Result<Option<String>> {
    let endpoint = format!("projects/{project_id}/merge_requests/{iid}/pipelines");
    let response = gitlab_api(profile, &endpoint, "GET", &[("per_page", "100")])?;
    let pipelines = response.as_array().ok_or_else(|| {
        anyhow::anyhow!(
            "GitLab pipeline lookup returned a non-list response for MR !{}: {}",
            iid,
            crate::redact::redact(&response.to_string())
        )
    })?;
    Ok(pipelines
        .iter()
        .find(|pipeline| pipeline["sha"].as_str() == Some(source_sha))
        .and_then(|pipeline| pipeline.get("status").and_then(serde_json::Value::as_str))
        .map(str::to_string))
}

fn normalize_gitlab_ci_status(status: &str) -> String {
    match status.trim().to_ascii_lowercase().as_str() {
        "running" | "created" | "pending" | "manual" => "pending".into(),
        "canceled" | "cancelled" => "canceled".into(),
        "success" => "passed".into(),
        "skipped" => "skipped".into(),
        status => status.into(),
    }
}

fn github_find_pr_number_by_branch(profile: &Profile, branch: &str) -> Result<String> {
    let (owner, _) = profile.repo.split_once('/').ok_or_else(|| {
        anyhow::anyhow!(
            "invalid GitHub repo '{}': expected owner/repository",
            profile.repo
        )
    })?;
    let endpoint = format!("repos/{}/pulls", profile.repo);
    let head = format!("head={owner}:{branch}");
    let out = provider_output_with_transient_retry("find GitHub PR by branch", || {
        let mut command = provider_command("gh");
        command.args([
            "api",
            "--method",
            "GET",
            &endpoint,
            "-f",
            "state=open",
            "-f",
            &head,
        ]);
        command
    })?;
    if !out.status.success() {
        anyhow::bail!(
            "GitHub REST PR lookup failed: {}",
            redacted_provider_output(&out)
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
            "number,url,title,body,isDraft,headRefName,baseRefName,state,mergedAt,headRefOid,statusCheckRollup",
        ])
        .output()
        .context("gh pr view")?;
    if !out.status.success() {
        anyhow::bail!("gh pr view failed: {}", redacted_stderr(&out));
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
        state: resp["state"].as_str().map(str::to_string),
        merged_at: resp["mergedAt"].as_str().map(str::to_string),
        title: resp["title"].as_str().map(str::to_string),
        body: resp["body"].as_str().map(str::to_string),
        draft: resp["isDraft"].as_bool().unwrap_or(false),
        ci_status,
        merge_status: None,
        source_sha: resp["headRefOid"].as_str().map(str::to_string),
        // `gh pr view` does not expose baseRefOid. The dispatch review path
        // resolves this from the fetched target ref used to build the diff.
        target_sha: None,
    })
}

fn gitlab_target_from_value(value: &serde_json::Value) -> Result<ReviewTarget> {
    // A provider/API proxy can still return an error-shaped JSON object;
    // silently defaulting branch fields to "" would turn that into an invalid
    // git refspec several layers downstream. Genuine MRs always carry these
    // fields, so their absence must fail at this boundary.
    if value["iid"].is_null()
        || value["source_branch"].is_null()
        || value["target_branch"].is_null()
    {
        anyhow::bail!(
            "GitLab API did not return a merge request (likely an auth/lookup failure): {value}"
        );
    }
    let web_url = value
        .get("web_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "GitLab API did not return a merge request (likely an auth/lookup failure): {}",
                crate::redact::redact(&value.to_string())
            )
        })?;
    let merge_status = value["detailed_merge_status"]
        .as_str()
        .or_else(|| value["merge_status"].as_str())
        .map(str::to_string);
    Ok(ReviewTarget {
        id: value["iid"].to_string().trim_matches('"').to_string(),
        url: web_url.to_string(),
        source_branch: value["source_branch"].as_str().unwrap_or("").to_string(),
        target_branch: value["target_branch"].as_str().unwrap_or("").to_string(),
        state: value["state"].as_str().map(str::to_string),
        merged_at: value["merged_at"].as_str().map(str::to_string),
        title: value["title"].as_str().map(str::to_string),
        body: value["description"].as_str().map(str::to_string),
        draft: value["draft"].as_bool().unwrap_or(false),
        ci_status: None,
        merge_status,
        source_sha: value["sha"].as_str().map(str::to_string),
        target_sha: value["diff_refs"]["base_sha"].as_str().map(str::to_string),
    })
}

fn gitlab_mr_result_from_value(value: &serde_json::Value) -> Result<MrResult> {
    let id = value
        .get("iid")
        .and_then(serde_json::Value::as_i64)
        .filter(|iid| *iid > 0)
        .map(|iid| iid.to_string());
    let url = value
        .get("web_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string);

    match (id, url) {
        (Some(id), Some(url)) => Ok(MrResult { id, url }),
        _ => anyhow::bail!(
            "GitLab API returned an invalid merge request payload (missing or empty iid/web_url): {}",
            crate::redact::redact(&value.to_string())
        ),
    }
}

#[cfg(test)]
#[path = "provider/tests.rs"]
mod tests;
