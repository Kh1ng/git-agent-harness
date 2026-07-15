use super::text::utf8_safe_suffix;
use super::text::{first_markdown_heading, normalize_match};
use crate::config::Profile;
use crate::models::WorkMetadata;
use crate::provider::provider_command;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub(super) type TicketMetadata = WorkMetadata;

/// Details about an issue fetched from GitHub/GitLab.
#[derive(Debug, Clone)]
pub(crate) struct IssueDetails {
    pub(super) number: String,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) labels: Vec<String>,
    pub(super) state: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueAuthorKind {
    Human,
    Bot,
    Unknown,
}

#[derive(Debug, Clone)]
struct IssueAuthorIdentity {
    login: String,
    kind: IssueAuthorKind,
}

#[derive(Debug, Clone)]
struct IssueRecord {
    details: IssueDetails,
    author: Option<IssueAuthorIdentity>,
}

#[derive(Debug, Clone)]
pub(crate) struct IssueIntakeDiscovery {
    pub(crate) allowed: Vec<IssueDetails>,
    pub(crate) rejected: Vec<crate::models::IssueIntakeRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueDisposition {
    OwnerDecision,
    Blocked,
    Planning,
}

impl IssueDisposition {
    fn reason_code(self) -> &'static str {
        match self {
            Self::OwnerDecision => "owner_decision",
            Self::Blocked => "blocked",
            Self::Planning => "planning",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::OwnerDecision => "owner-decision label present",
            Self::Blocked => "blocked label present",
            Self::Planning => "planning label present",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntakeRejection {
    MissingAuthor,
    MalformedAuthorIdentity,
    UntrustedAuthor,
    CanonicalLabelRequired,
    Disposition(IssueDisposition),
}

impl IntakeRejection {
    fn reason_code(self) -> &'static str {
        match self {
            Self::MissingAuthor => "missing_author",
            Self::MalformedAuthorIdentity => "malformed_author_identity",
            Self::UntrustedAuthor => "untrusted_author",
            Self::CanonicalLabelRequired => "canonical_autonomous_label_required",
            Self::Disposition(disposition) => disposition.reason_code(),
        }
    }

    fn reason(self, canonical_label: &str) -> String {
        match self {
            Self::MissingAuthor => "author identity missing or unreadable".to_string(),
            Self::MalformedAuthorIdentity => {
                "provider returned a malformed author identity".to_string()
            }
            Self::UntrustedAuthor => "author is not trusted by this profile".to_string(),
            Self::CanonicalLabelRequired => {
                format!("missing canonical autonomous label '{canonical_label}'")
            }
            Self::Disposition(disposition) => disposition.reason().to_string(),
        }
    }
}

pub(super) fn ticket_number_prefix(work_id: &str) -> Option<&str> {
    let rest = work_id
        .strip_prefix("TICKET-")
        .or_else(|| work_id.strip_prefix('#'))?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

pub(super) fn is_issue_number_reference(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }

    if let Some(number_part) = trimmed.strip_prefix('#') {
        return !number_part.is_empty() && number_part.chars().all(|c| c.is_ascii_digit());
    }

    trimmed.chars().all(|c| c.is_ascii_digit())
}

pub(super) fn extract_issue_number(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }

    let number_str = if let Some(number_part) = trimmed.strip_prefix('#') {
        if number_part.is_empty() {
            return None;
        }
        number_part
    } else {
        trimmed
    };

    if number_str.chars().all(|c| c.is_ascii_digit()) {
        Some(number_str.to_string())
    } else {
        None
    }
}

fn login_matches(login: &str, candidates: &[String]) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(login))
}

fn issue_author_kind_to_str(kind: IssueAuthorKind) -> &'static str {
    match kind {
        IssueAuthorKind::Human => "human",
        IssueAuthorKind::Bot => "bot",
        IssueAuthorKind::Unknown => "unknown",
    }
}

fn parse_github_author(response: &serde_json::Value) -> Option<IssueAuthorIdentity> {
    let author = response.get("author")?;
    let login = author.get("login")?.as_str()?.trim();
    if login.is_empty() {
        return None;
    }
    let kind = match (
        author.get("is_bot").and_then(|value| value.as_bool()),
        author.get("type").and_then(|value| value.as_str()),
    ) {
        (Some(true), _) | (_, Some("Bot")) => IssueAuthorKind::Bot,
        (Some(false), _) | (_, Some("User")) | (_, Some("Organization")) => IssueAuthorKind::Human,
        _ => IssueAuthorKind::Unknown,
    };
    Some(IssueAuthorIdentity {
        login: login.to_string(),
        kind,
    })
}

fn parse_gitlab_author(response: &serde_json::Value) -> Option<IssueAuthorIdentity> {
    let author = response.get("author")?;
    let login = author
        .get("username")
        .and_then(|value| value.as_str())
        .or_else(|| author.get("login").and_then(|value| value.as_str()))?
        .trim();
    if login.is_empty() {
        return None;
    }
    let kind = match (
        author.get("bot").and_then(|value| value.as_bool()),
        author.get("is_bot").and_then(|value| value.as_bool()),
    ) {
        (Some(true), _) | (_, Some(true)) => IssueAuthorKind::Bot,
        (Some(false), _) | (_, Some(false)) => IssueAuthorKind::Human,
        _ => IssueAuthorKind::Unknown,
    };
    Some(IssueAuthorIdentity {
        login: login.to_string(),
        kind,
    })
}

fn issue_author_is_trusted(profile: &Profile, author: &IssueAuthorIdentity) -> bool {
    match author.kind {
        IssueAuthorKind::Unknown => false,
        IssueAuthorKind::Bot => profile
            .publishing
            .trusted_issue_bot_authors
            .as_deref()
            .is_some_and(|authors| login_matches(&author.login, authors)),
        IssueAuthorKind::Human => {
            if let Some(authors) = profile.publishing.trusted_issue_human_authors.as_deref() {
                login_matches(&author.login, authors)
            } else if let Some(allowlist) =
                profile.publishing.github_issue_author_allowlist.as_deref()
            {
                login_matches(&author.login, allowlist)
            } else if profile.provider.eq_ignore_ascii_case("github") {
                profile
                    .repo
                    .split_once('/')
                    .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(&author.login))
            } else {
                false
            }
        }
    }
}

fn issue_disposition_from_labels(labels: &[String]) -> Option<IssueDisposition> {
    for label in labels {
        match label.trim().to_ascii_lowercase().as_str() {
            "executive:owner-decision" | "exec:owner-decision" => {
                return Some(IssueDisposition::OwnerDecision)
            }
            "blocked" | "gah:blocked" => return Some(IssueDisposition::Blocked),
            "planning" | "plan" => return Some(IssueDisposition::Planning),
            _ => {}
        }
    }
    None
}

fn issue_is_canonical_autonomous(labels: &[String], canonical_label: &str) -> bool {
    labels
        .iter()
        .any(|label| label.trim().eq_ignore_ascii_case(canonical_label.trim()))
}

fn evaluate_issue_intake(
    profile: &Profile,
    author: Option<&IssueAuthorIdentity>,
    labels: &[String],
    allow_label_override: bool,
) -> Result<(), IntakeRejection> {
    let Some(author) = author else {
        return Err(IntakeRejection::MissingAuthor);
    };
    if author.kind == IssueAuthorKind::Unknown {
        return Err(IntakeRejection::MalformedAuthorIdentity);
    }
    if !issue_author_is_trusted(profile, author) {
        return Err(IntakeRejection::UntrustedAuthor);
    }

    if let Some(disposition) = issue_disposition_from_labels(labels) {
        return Err(IntakeRejection::Disposition(disposition));
    }

    if matches!(
        profile.publishing.issue_intake_mode,
        crate::config::IssueIntakeMode::CanonicalAutonomousOnly
    ) && !issue_is_canonical_autonomous(labels, &profile.publishing.canonical_autonomous_label)
        && !allow_label_override
    {
        return Err(IntakeRejection::CanonicalLabelRequired);
    }

    Ok(())
}

#[cfg(test)]
pub(super) fn github_issue_author_is_allowed(
    profile: &Profile,
    response: &serde_json::Value,
) -> bool {
    let Some(author) = parse_github_author(response) else {
        return false;
    };
    issue_author_is_trusted(profile, &author)
}

fn issue_details_from_github_response(
    profile: &Profile,
    issue_number: &str,
    resp: &serde_json::Value,
    allow_label_override: bool,
) -> Result<IssueDetails> {
    let author = parse_github_author(resp);
    if let Err(rejection) = evaluate_issue_intake(
        profile,
        author.as_ref(),
        resp["labels"]
            .as_array()
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| label["name"].as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .as_slice(),
        allow_label_override,
    ) {
        anyhow::bail!(
            "GitHub issue #{} rejected for intake: {} ({})",
            issue_number,
            rejection.reason(&profile.publishing.canonical_autonomous_label),
            rejection.reason_code()
        );
    }

    let number = resp["number"]
        .as_i64()
        .map(|n| n.to_string())
        .unwrap_or_else(|| issue_number.to_string());

    let title = resp["title"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Issue #{}", issue_number));

    let body = resp["body"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();

    let labels = resp["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let state = resp["state"].as_str().map(str::to_string);

    Ok(IssueDetails {
        number,
        title,
        body,
        labels,
        state,
    })
}

fn issue_details_from_gitlab_response(
    profile: &Profile,
    issue_number: &str,
    resp: &serde_json::Value,
    allow_label_override: bool,
) -> Result<IssueDetails> {
    let author = parse_gitlab_author(resp);
    if let Err(rejection) = evaluate_issue_intake(
        profile,
        author.as_ref(),
        resp["labels"]
            .as_array()
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| label.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .as_slice(),
        allow_label_override,
    ) {
        anyhow::bail!(
            "GitLab issue #{} rejected for intake: {} ({})",
            issue_number,
            rejection.reason(&profile.publishing.canonical_autonomous_label),
            rejection.reason_code()
        );
    }

    let number = resp["iid"]
        .as_i64()
        .map(|n| n.to_string())
        .unwrap_or_else(|| issue_number.to_string());

    let title = resp["title"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Issue #{}", issue_number));

    let body = resp["description"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();

    let labels = resp["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let state = resp["state"].as_str().map(str::to_string);

    Ok(IssueDetails {
        number,
        title,
        body,
        labels,
        state,
    })
}

fn fetch_github_issue(
    profile: &Profile,
    issue_number: &str,
    allow_label_override: bool,
) -> Result<IssueDetails> {
    let out = provider_command("gh")
        .arg("issue")
        .arg("view")
        .arg(issue_number)
        .arg("--repo")
        .arg(&profile.repo)
        .arg("--json")
        .arg("title,body,labels,author,state")
        .output()
        .context("gh issue view")?;

    if !out.status.success() {
        anyhow::bail!(
            "gh issue view failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let resp: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing GitHub issue response")?;
    issue_details_from_github_response(profile, issue_number, &resp, allow_label_override)
}

fn fetch_gitlab_issue(
    profile: &Profile,
    issue_number: &str,
    allow_label_override: bool,
) -> Result<IssueDetails> {
    let out = provider_command("glab")
        .arg("issue")
        .arg("view")
        .arg(issue_number)
        .arg("--repo")
        .arg(&profile.repo)
        .arg("-F")
        .arg("json")
        .output()
        .context("glab issue view")?;

    if !out.status.success() {
        anyhow::bail!(
            "glab issue view failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let resp: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing GitLab issue response")?;
    issue_details_from_gitlab_response(profile, issue_number, &resp, allow_label_override)
}

fn issue_record_from_github_value(resp: &serde_json::Value) -> IssueRecord {
    let number = resp["number"]
        .as_i64()
        .map(|n| n.to_string())
        .unwrap_or_default();
    let title = resp["title"].as_str().unwrap_or_default().to_string();
    let body = resp["body"].as_str().unwrap_or_default().to_string();
    let labels = resp["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let state = resp["state"].as_str().map(str::to_string);
    IssueRecord {
        details: IssueDetails {
            number,
            title,
            body,
            labels,
            state,
        },
        author: parse_github_author(resp),
    }
}

fn issue_record_from_gitlab_value(resp: &serde_json::Value) -> IssueRecord {
    let number = resp["iid"]
        .as_i64()
        .map(|n| n.to_string())
        .unwrap_or_default();
    let title = resp["title"].as_str().unwrap_or_default().to_string();
    let body = resp["description"].as_str().unwrap_or_default().to_string();
    let labels = resp["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let state = resp["state"].as_str().map(str::to_string);
    IssueRecord {
        details: IssueDetails {
            number,
            title,
            body,
            labels,
            state,
        },
        author: parse_gitlab_author(resp),
    }
}

fn issue_rejection_snapshot(
    profile: &Profile,
    record: &IssueRecord,
    rejection: IntakeRejection,
) -> crate::models::IssueIntakeRejection {
    crate::models::IssueIntakeRejection {
        ticket_path: record.details.number.clone(),
        work_id: Some(format!("#{}", record.details.number)),
        title: Some(record.details.title.clone()),
        provider: profile.provider.clone(),
        author_login: record.author.as_ref().map(|author| author.login.clone()),
        author_kind: record
            .author
            .as_ref()
            .map(|author| issue_author_kind_to_str(author.kind).to_string()),
        reason_code: rejection.reason_code().to_string(),
        reason: rejection.reason(&profile.publishing.canonical_autonomous_label),
        labels: record.details.labels.clone(),
    }
}

fn discover_open_github_issues(profile: &Profile) -> Result<IssueIntakeDiscovery> {
    let out = provider_command("gh")
        .arg("issue")
        .arg("list")
        .arg("--repo")
        .arg(&profile.repo)
        .arg("--state")
        .arg("open")
        .arg("--json")
        .arg("number,title,body,labels,author,state")
        .arg("--limit")
        .arg("1000")
        .output()
        .context("gh issue list")?;

    if !out.status.success() {
        anyhow::bail!(
            "gh issue list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let items: Vec<serde_json::Value> =
        serde_json::from_slice(&out.stdout).context("parsing GitHub issue list response")?;
    let mut allowed = Vec::new();
    let mut rejected = Vec::new();
    for resp in items {
        let record = issue_record_from_github_value(&resp);
        match evaluate_issue_intake(
            profile,
            record.author.as_ref(),
            &record.details.labels,
            false,
        ) {
            Ok(()) => allowed.push(record.details),
            Err(rejection) => rejected.push(issue_rejection_snapshot(profile, &record, rejection)),
        }
    }

    Ok(IssueIntakeDiscovery { allowed, rejected })
}

fn discover_open_gitlab_issues(profile: &Profile) -> Result<IssueIntakeDiscovery> {
    const PAGE_SIZE: usize = 100;
    let mut allowed = Vec::new();
    let mut rejected = Vec::new();
    let mut page = 1;
    loop {
        let out = provider_command("glab")
            .arg("issue")
            .arg("list")
            .arg("--repo")
            .arg(&profile.repo)
            .arg("--per-page")
            .arg(PAGE_SIZE.to_string())
            .arg("--page")
            .arg(page.to_string())
            .arg("-O")
            .arg("json")
            .output()
            .context("glab issue list")?;

        if !out.status.success() {
            anyhow::bail!(
                "glab issue list failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }

        let items: Vec<serde_json::Value> =
            serde_json::from_slice(&out.stdout).context("parsing GitLab issue list response")?;
        let count = items.len();

        for resp in items {
            let record = issue_record_from_gitlab_value(&resp);
            match evaluate_issue_intake(
                profile,
                record.author.as_ref(),
                &record.details.labels,
                false,
            ) {
                Ok(()) => allowed.push(record.details),
                Err(rejection) => {
                    rejected.push(issue_rejection_snapshot(profile, &record, rejection))
                }
            }
        }

        if count < PAGE_SIZE {
            break;
        }
        page += 1;
    }
    Ok(IssueIntakeDiscovery { allowed, rejected })
}

pub(super) fn list_open_issues(profile: &Profile) -> Vec<IssueDetails> {
    let result = match profile.provider_cli() {
        Some("gh") => discover_open_github_issues(profile),
        Some("glab") => discover_open_gitlab_issues(profile),
        _ => return vec![],
    };
    result
        .map(|discovery| discovery.allowed)
        .unwrap_or_else(|e| {
            eprintln!("warning: failed to list open issues for ticket scan: {e:#}");
            vec![]
        })
}

pub(crate) fn discover_open_issues(profile: &Profile) -> IssueIntakeDiscovery {
    match profile.provider_cli() {
        Some("gh") => discover_open_github_issues(profile).unwrap_or_else(|e| {
            eprintln!("warning: failed to list open issues for ticket scan: {e:#}");
            IssueIntakeDiscovery {
                allowed: vec![],
                rejected: vec![],
            }
        }),
        Some("glab") => discover_open_gitlab_issues(profile).unwrap_or_else(|e| {
            eprintln!("warning: failed to list open issues for ticket scan: {e:#}");
            IssueIntakeDiscovery {
                allowed: vec![],
                rejected: vec![],
            }
        }),
        _ => IssueIntakeDiscovery {
            allowed: vec![],
            rejected: vec![],
        },
    }
}

pub(super) fn fetch_issue_details(
    profile: &Profile,
    issue_number: &str,
    allow_label_override: bool,
) -> Result<IssueDetails> {
    let cli = profile.provider_cli().ok_or_else(|| {
        anyhow::anyhow!(
            "provider '{}' does not support issue fetching",
            profile.provider
        )
    })?;

    match cli {
        "gh" => fetch_github_issue(profile, issue_number, allow_label_override),
        "glab" => fetch_gitlab_issue(profile, issue_number, allow_label_override),
        other => anyhow::bail!("unsupported provider CLI: {}", other),
    }
}

fn extract_field_value(body: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}:");
    body.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn extract_markdown_section(body: &str, heading: &str) -> Option<String> {
    let mut capture = false;
    let mut lines = Vec::new();
    for raw_line in body.lines() {
        let trimmed = raw_line.trim();
        if trimmed.starts_with('#') {
            let normalized = trimmed.trim_start_matches('#').trim();
            if capture {
                break;
            }
            capture = normalized.eq_ignore_ascii_case(heading);
            continue;
        }
        if capture {
            lines.push(raw_line.trim_end().to_string());
        }
    }
    let section = lines.join("\n").trim().to_string();
    if section.is_empty() {
        None
    } else {
        Some(section)
    }
}

pub(super) fn extract_markdown_list_section(body: &str, heading: &str) -> Vec<String> {
    extract_markdown_section(body, heading)
        .map(|section| {
            section
                .lines()
                .map(str::trim)
                .filter_map(|line| {
                    line.strip_prefix("- ")
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Bullet-list items under `heading`, falling back to the whole section as a
/// single entry when its content isn't formatted as a list (issue #405:
/// `Invariants`/`Required Behavior` sections written as prose were
/// previously discarded entirely by the list-only extractor).
pub(super) fn extract_markdown_requirement_items(body: &str, heading: &str) -> Vec<String> {
    let items = extract_markdown_list_section(body, heading);
    if !items.is_empty() {
        return items;
    }
    extract_markdown_section(body, heading)
        .into_iter()
        .collect()
}

pub(super) fn extract_markdown_code_list_section(body: &str, heading: &str) -> Vec<String> {
    extract_markdown_list_section(body, heading)
        .into_iter()
        .map(|item| {
            item.strip_prefix('`')
                .and_then(|value| value.strip_suffix('`'))
                .unwrap_or(item.as_str())
                .to_string()
        })
        .collect()
}

fn normalize_ticket_title(title: String) -> String {
    let trimmed = title.trim();
    let Some(rest) = trimmed.strip_prefix("TICKET-") else {
        return title;
    };

    let digit_byte_count = rest
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, _)| i + 1)
        .unwrap_or(0);
    if digit_byte_count == 0 {
        return title;
    }

    let remainder = utf8_safe_suffix(rest, rest.len() - digit_byte_count).trim_start();
    let normalized = remainder
        .trim_start_matches([':', '-'])
        .trim_start()
        .to_string();

    if normalized.is_empty() {
        title
    } else {
        normalized
    }
}

pub(super) fn parse_ticket_metadata(path: &Path) -> Result<Option<TicketMetadata>> {
    if path.extension().and_then(|e| e.to_str()) != Some("md") || !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(path)?;
    let ticket_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| {
            let mut parts = stem.split('-');
            match (parts.next(), parts.next()) {
                (Some("TICKET"), Some(number)) if !number.is_empty() => {
                    Some(format!("TICKET-{number}"))
                }
                _ => None,
            }
        });
    let raw_heading = first_markdown_heading(&body);
    let mut work_id_from_heading = None;
    if let Some(heading) = raw_heading {
        let trimmed = heading.trim();
        if trimmed.starts_with("TICKET-") {
            // Tickets are titled either "TICKET-N: Title" or "TICKET-N — Title"
            // (em dash, no colon) -- both are in real use across this repo's
            // own ticket backlog, so both must be recognized or the em-dash
            // style silently fails is_authoritative and never gets dispatched.
            if let Some((id, _)) = trimmed
                .split_once(':')
                .or_else(|| trimmed.split_once(" — "))
            {
                work_id_from_heading = Some(id.trim().to_string());
            }
        }
    }
    let title = raw_heading.map(|title| normalize_ticket_title(title.into()));
    let mut meta = TicketMetadata {
        ticket_id,
        title,
        ..TicketMetadata::default()
    };
    meta.summary = meta.title.clone();
    meta.problem = extract_markdown_section(&body, "Problem");
    meta.acceptance_criteria = extract_markdown_list_section(&body, "Acceptance Criteria");
    meta.constraints = extract_markdown_list_section(&body, "Constraints");
    meta.verification_commands = extract_markdown_code_list_section(&body, "Verification Commands");
    meta.affected_files = extract_markdown_list_section(&body, "Affected Files");
    meta.source = extract_field_value(&body, "Source")
        .or_else(|| extract_markdown_section(&body, "Source"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    for line in body.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Difficulty:") {
            meta.difficulty = Some(value.trim().to_string());
        } else if let Some(value) = line
            .strip_prefix("Task class:")
            .or_else(|| line.strip_prefix("Task Class:"))
        {
            meta.task_class = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Risk:") {
            meta.risk = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Recommended backend:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_backend = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Recommended model:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_model = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Goal:") {
            let value = value.trim();
            if !value.is_empty() {
                meta.goal = Some(value.to_string());
            }
            if meta.title.is_none() && !value.is_empty() {
                meta.title = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Suggested MR Title:") {
            meta.suggested_mr_title = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Work ID:") {
            meta.work_id = Some(value.trim().to_string());
        }
    }
    if meta.work_id.is_none() {
        meta.work_id = work_id_from_heading;
    }

    let mut is_authoritative = false;
    if let Some(ref file_id) = meta.ticket_id {
        if let Some(ref cont_id) = meta.work_id {
            if file_id == cont_id {
                is_authoritative = true;
            }
        }
    }
    if is_authoritative {
        let repo_dir = path.parent().and_then(|p| p.parent());
        let manager_memory_path = repo_dir.map(|p| p.join("MANAGER_MEMORY.md"));
        if let Some(ref p) = manager_memory_path {
            if p.exists() {
                if let Ok(content) = fs::read_to_string(p) {
                    let file_id = meta.ticket_id.as_ref().unwrap();
                    for line in content.lines() {
                        let is_table_row = line.trim_start().starts_with('|');
                        if is_table_row && line.contains(file_id) {
                            if let Some(ref title) = meta.title {
                                let norm_line = normalize_match(line);
                                let norm_title = normalize_match(title);
                                if !norm_line.contains(&norm_title) {
                                    is_authoritative = false;
                                    break;
                                }
                            } else {
                                is_authoritative = false;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    meta.is_authoritative = is_authoritative;

    Ok(Some(meta))
}

pub(super) fn parse_ticket_metadata_from_issue(issue: &IssueDetails) -> TicketMetadata {
    let issue_identity = format!("#{}", issue.number);
    let mut meta = TicketMetadata {
        ticket_id: Some(issue_identity.clone()),
        work_id: Some(issue_identity),
        issue_number: Some(issue.number.clone()),
        ..TicketMetadata::default()
    };

    for line in issue.body.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Difficulty:") {
            meta.difficulty = Some(value.trim().to_string());
        } else if let Some(value) = line
            .strip_prefix("Task class:")
            .or_else(|| line.strip_prefix("Task Class:"))
        {
            meta.task_class = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Risk:") {
            meta.risk = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Recommended backend:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_backend = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Recommended model:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_model = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Goal:") {
            let value = value.trim();
            if !value.is_empty() {
                meta.goal = Some(value.to_string());
            }
            if meta.title.is_none() && !value.is_empty() {
                meta.title = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Suggested MR Title:") {
            meta.suggested_mr_title = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Source:") {
            meta.source = Some(value.trim().to_string());
        }
    }

    meta.is_authoritative = meta.ticket_id.is_some() || meta.work_id.is_some();
    if meta.goal.is_none() {
        meta.goal = extract_markdown_section(&issue.body, "Goal");
    }
    meta.problem = extract_markdown_section(&issue.body, "Problem")
        .or_else(|| extract_markdown_section(&issue.body, "Background"))
        .or_else(|| extract_markdown_section(&issue.body, "Description"));
    // Issue #405: `Scope` is common requirement-shaped phrasing but is only
    // ever a stand-in for problem/goal content -- an issue that already has
    // an explicit Problem/Background/Description, `Goal:` field, or `Goal`
    // section
    // keeps that as authoritative rather than being overridden by Scope.
    if meta.problem.is_none() && meta.goal.is_none() {
        meta.problem = extract_markdown_section(&issue.body, "Scope");
    }
    meta.acceptance_criteria = extract_markdown_list_section(&issue.body, "Acceptance Criteria");
    meta.constraints = extract_markdown_list_section(&issue.body, "Constraints");
    // Issue #405: `Invariants` and `Required Behavior` were silently dropped
    // because only `Constraints` was recognized. Fold both into the bounded
    // constraints list rather than discarding them; fall back to the whole
    // section as a single entry when the heading's content isn't formatted
    // as a bullet list.
    for heading in ["Invariants", "Required Behavior"] {
        meta.constraints
            .extend(extract_markdown_requirement_items(&issue.body, heading));
    }
    meta.verification_commands =
        extract_markdown_code_list_section(&issue.body, "Verification Commands");
    meta.affected_files = extract_markdown_list_section(&issue.body, "Affected Files");

    if !issue.labels.is_empty() {
        for label in &issue.labels {
            if label.contains('/') || label.contains('.') {
                if !meta.affected_files.contains(label) {
                    meta.affected_files.push(label.clone());
                }
            } else if !meta.constraints.contains(label) {
                meta.constraints.push(label.clone());
            }
        }
    }

    if meta.title.is_none() {
        meta.title = Some(normalize_ticket_title(issue.title.trim().to_string()));
    }
    meta.summary = meta.title.clone();

    meta
}

pub(super) fn issue_is_auto_dispatch_blocked(labels: &[String]) -> bool {
    labels.iter().any(|label| {
        matches!(
            label.trim().to_ascii_lowercase().as_str(),
            "executive:owner-decision"
                | "exec:owner-decision"
                | "blocked"
                | "planning"
                | "plan"
                | "gah:blocked"
        )
    })
}

pub(super) fn resolve_target_to_issue_or_string(
    profile: &Profile,
    target: &str,
    allow_label_override: bool,
) -> Result<Option<IssueDetails>> {
    if is_issue_number_reference(target) {
        if let Some(issue_number) = extract_issue_number(target) {
            return Ok(Some(fetch_issue_details(
                profile,
                &issue_number,
                allow_label_override,
            )?));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
