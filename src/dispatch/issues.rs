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
pub(super) struct IssueDetails {
    pub(super) number: String,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) labels: Vec<String>,
    pub(super) state: Option<String>,
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

pub(super) fn github_issue_author_is_allowed(
    profile: &Profile,
    response: &serde_json::Value,
) -> bool {
    let Some(author) = response["author"]["login"].as_str() else {
        return false;
    };
    match profile.publishing.github_issue_author_allowlist.as_deref() {
        Some(allowlist) => allowlist
            .iter()
            .any(|login| login.eq_ignore_ascii_case(author)),
        None => profile
            .repo
            .split_once('/')
            .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(author)),
    }
}

fn fetch_github_issue(profile: &Profile, issue_number: &str) -> Result<IssueDetails> {
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
    if !github_issue_author_is_allowed(profile, &resp) {
        anyhow::bail!(
            "GitHub issue #{} author is not allowed by this profile's github_issue_author_allowlist",
            issue_number
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

fn fetch_gitlab_issue(profile: &Profile, issue_number: &str) -> Result<IssueDetails> {
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

fn list_open_github_issues(profile: &Profile) -> Result<Vec<IssueDetails>> {
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

    Ok(items
        .into_iter()
        .filter(|resp| github_issue_author_is_allowed(profile, resp))
        .map(|resp| {
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
            IssueDetails {
                number,
                title,
                body,
                labels,
                state,
            }
        })
        .collect())
}

fn list_open_gitlab_issues(profile: &Profile) -> Result<Vec<IssueDetails>> {
    const PAGE_SIZE: usize = 100;
    let mut all = Vec::new();
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
            all.push(IssueDetails {
                number,
                title,
                body,
                labels,
                state,
            });
        }

        if count < PAGE_SIZE {
            break;
        }
        page += 1;
    }
    Ok(all)
}

pub(super) fn list_open_issues(profile: &Profile) -> Vec<IssueDetails> {
    let result = match profile.provider_cli() {
        Some("gh") => list_open_github_issues(profile),
        Some("glab") => list_open_gitlab_issues(profile),
        _ => return vec![],
    };
    result.unwrap_or_else(|e| {
        eprintln!("warning: failed to list open issues for ticket scan: {e:#}");
        vec![]
    })
}

pub(super) fn fetch_issue_details(profile: &Profile, issue_number: &str) -> Result<IssueDetails> {
    let cli = profile.provider_cli().ok_or_else(|| {
        anyhow::anyhow!(
            "provider '{}' does not support issue fetching",
            profile.provider
        )
    })?;

    match cli {
        "gh" => fetch_github_issue(profile, issue_number),
        "glab" => fetch_gitlab_issue(profile, issue_number),
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
    meta.problem = extract_markdown_section(&issue.body, "Problem")
        .or_else(|| extract_markdown_section(&issue.body, "Background"))
        .or_else(|| extract_markdown_section(&issue.body, "Description"));
    meta.acceptance_criteria = extract_markdown_list_section(&issue.body, "Acceptance Criteria");
    meta.constraints = extract_markdown_list_section(&issue.body, "Constraints");
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
) -> Result<Option<IssueDetails>> {
    if is_issue_number_reference(target) {
        if let Some(issue_number) = extract_issue_number(target) {
            return Ok(Some(fetch_issue_details(profile, &issue_number)?));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Defaults, GahConfig, Profile, RoutingPolicy};
    use crate::dispatch::scan_available_tickets;
    use crate::ledger;
    use crate::test_support::{ExecGuard, PathGuard};
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    fn profile(local_path: &Path) -> Profile {
        Profile {
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: local_path.display().to_string(),
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
            vibe_args: vec![],
            vibe_path: None,
            opencode_args: vec![],
            opencode_path: None,
            agy_second_home: None,
            agy_print_timeout_seconds: HashMap::new(),
            agy_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds_by_model: HashMap::new(),
            max_concurrent_per_model: HashMap::new(),
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            auto_fix_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            notify_command: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
            publishing: Default::default(),
            prune_older_than_days: None,
        }
    }

    fn ticket_cfg(root: &Path) -> GahConfig {
        GahConfig {
            context: Default::default(),
            defaults: Defaults {
                current_manager: None,
                artifact_root: root.to_string_lossy().into_owned(),
                worktree_base: root.to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: RoutingPolicy::default(),
            },
            profiles: HashMap::new(),
        }
    }

    #[test]
    fn parses_ticket_metadata_for_routing() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-058-descriptive-mr-titles.md");
        fs::write(
            &ticket,
            "# TICKET-058: Descriptive MR Titles\n\nDifficulty: hard\nRisk: high\nRecommended backend: codex\nRecommended model: gpt-x\n\n## Affected Files\n- src/auth.rs\n\n## Verification Commands\n- `pytest tests/test_auth.py -x`\n",
        )
        .unwrap();
        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-058"));
        assert_eq!(meta.title.as_deref(), Some("Descriptive MR Titles"));
        assert_eq!(meta.recommended_backend.as_deref(), Some("codex"));
        assert_eq!(meta.recommended_model.as_deref(), Some("gpt-x"));
        assert_eq!(meta.difficulty.as_deref(), Some("hard"));
        assert_eq!(meta.risk.as_deref(), Some("high"));
        assert_eq!(
            meta.verification_commands,
            vec!["pytest tests/test_auth.py -x"]
        );
    }

    #[test]
    fn parses_structured_ticket_sections_into_typed_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-092-structured-work-metadata.md");
        fs::write(
            &ticket,
            "# TICKET-092: Structured work metadata\n\n\
Goal: Represent task metadata as typed structured fields rather than prompt parsing.\n\n\
Difficulty: medium\n\
Risk: medium\n\
Recommended backend: codex\n\
Recommended model: gpt-5.4\n\
Source: docs/tickets/TICKET-092-structured-work-metadata.md\n\n\
## Problem\n\
The parser should retain structured sections.\n\n\
## Acceptance Criteria\n\
- Define a single structured metadata type\n\
- Missing fields handled explicitly\n\n\
## Constraints\n\
- Do not require a new file format\n\
- No database\n\n\
## Affected Files\n\
- src/dispatch.rs\n\
- src/models.rs\n\n\
## Verification Commands\n\
- `cargo fmt --check`\n\
- `cargo test`\n",
        )
        .unwrap();

        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-092"));
        assert_eq!(meta.work_id.as_deref(), Some("TICKET-092"));
        assert_eq!(meta.summary.as_deref(), Some("Structured work metadata"));
        assert_eq!(
            meta.problem.as_deref(),
            Some("The parser should retain structured sections.")
        );
        assert_eq!(
            meta.acceptance_criteria,
            vec![
                "Define a single structured metadata type",
                "Missing fields handled explicitly"
            ]
        );
        assert_eq!(
            meta.constraints,
            vec!["Do not require a new file format", "No database"]
        );
        assert_eq!(
            meta.affected_files,
            vec!["src/dispatch.rs", "src/models.rs"]
        );
        assert_eq!(
            meta.verification_commands,
            vec!["cargo fmt --check", "cargo test"]
        );
        assert_eq!(
            meta.source.as_deref(),
            Some("docs/tickets/TICKET-092-structured-work-metadata.md")
        );
    }

    #[test]
    fn parses_ticket_metadata_preserves_colons_in_normal_headings() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-104-auth-hardening.md");
        fs::write(
            &ticket,
            "# Auth: reject empty token\n\nDifficulty: medium\nRisk: low\n",
        )
        .unwrap();

        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-104"));
        assert_eq!(meta.title.as_deref(), Some("Auth: reject empty token"));
    }

    #[test]
    fn parses_ticket_metadata_strips_ticket_prefix_from_heading_title() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-105-heading-title.md");
        fs::write(&ticket, "# TICKET-105: Keep title intact\n").unwrap();

        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-105"));
        assert_eq!(meta.title.as_deref(), Some("Keep title intact"));
    }

    #[test]
    fn parse_ticket_metadata_ignores_incidental_manager_memory_prose_mentions() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let tickets_dir = repo.join("docs/tickets");
        fs::create_dir_all(&tickets_dir).unwrap();

        fs::write(
            repo.join("docs/MANAGER_MEMORY.md"),
            "- **TICKET-114 is a serving-integrity control**\n\
             - **TICKET-110 before TICKET-112**\n",
        )
        .unwrap();

        let ticket_path = tickets_dir.join("TICKET-114-artifact-load-integrity.md");
        fs::write(
            &ticket_path,
            "# TICKET-114 — Artifact load integrity verification\n\nGoal: test\n",
        )
        .unwrap();

        let meta = parse_ticket_metadata(&ticket_path).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-114"));
        assert_eq!(meta.work_id.as_deref(), Some("TICKET-114"));
        assert!(meta.is_authoritative);
    }

    #[test]
    fn is_issue_number_reference_recognizes_plain_numbers() {
        assert!(is_issue_number_reference("42"));
        assert!(is_issue_number_reference("123"));
        assert!(!is_issue_number_reference("abc"));
        assert!(!is_issue_number_reference(""));
        assert!(!is_issue_number_reference("42abc"));
    }

    #[test]
    fn is_issue_number_reference_recognizes_hash_numbers() {
        assert!(is_issue_number_reference("#42"));
        assert!(is_issue_number_reference("#123"));
        assert!(!is_issue_number_reference("#"));
        assert!(!is_issue_number_reference("#abc"));
        assert!(is_issue_number_reference(" #42 "));
    }

    #[test]
    fn extract_issue_number_from_plain_number() {
        assert_eq!(extract_issue_number("42"), Some("42".to_string()));
        assert_eq!(extract_issue_number("123"), Some("123".to_string()));
        assert_eq!(extract_issue_number("abc"), None);
        assert_eq!(extract_issue_number(""), None);
    }

    #[test]
    fn extract_issue_number_from_hash_number() {
        assert_eq!(extract_issue_number("#42"), Some("42".to_string()));
        assert_eq!(extract_issue_number("#123"), Some("123".to_string()));
        assert_eq!(extract_issue_number("#"), None);
        assert_eq!(extract_issue_number("#abc"), None);
    }

    #[test]
    fn parse_ticket_metadata_from_issue_extracts_basic_fields() {
        let issue = IssueDetails {
            number: "42".to_string(),
            title: "TICKET-42: Fix the bug".to_string(),
            body:
                "## Problem\n\nSomething is broken\n\n## Acceptance Criteria\n\n- Fix the issue\n- Add tests"
                    .to_string(),
            labels: vec!["bug".to_string()],
            state: None,
        };

        let meta = parse_ticket_metadata_from_issue(&issue);
        assert_eq!(meta.ticket_id.as_deref(), Some("#42"));
        assert_eq!(meta.work_id.as_deref(), Some("#42"));
        assert_eq!(meta.issue_number.as_deref(), Some("42"));
        assert_eq!(meta.title.as_deref(), Some("Fix the bug"));
        assert!(meta.is_authoritative);
        assert!(meta
            .acceptance_criteria
            .contains(&"Fix the issue".to_string()));
        assert!(meta.acceptance_criteria.contains(&"Add tests".to_string()));
    }

    #[test]
    fn parse_ticket_metadata_from_issue_handles_metadata_fields() {
        let issue = IssueDetails {
            number: "42".to_string(),
            title: "Test Issue".to_string(),
            body: "Difficulty: High\nRisk: Medium\nRecommended backend: agy\nWork ID: TICKET-999\nGoal: Fix everything"
                .to_string(),
            labels: vec![],
            state: None,
        };

        let meta = parse_ticket_metadata_from_issue(&issue);
        assert_eq!(meta.difficulty.as_deref(), Some("High"));
        assert_eq!(meta.risk.as_deref(), Some("Medium"));
        assert_eq!(meta.recommended_backend.as_deref(), Some("agy"));
        assert_eq!(meta.goal.as_deref(), Some("Fix everything"));
        assert_eq!(meta.work_id.as_deref(), Some("#42"));
    }

    #[test]
    fn github_issue_intake_author_allowlist_is_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.repo = "Kh1ng/git-agent-harness".into();
        let owner = serde_json::json!({"author": {"login": "kh1ng"}});
        let outsider = serde_json::json!({"author": {"login": "untrusted"}});
        let missing = serde_json::json!({});

        assert!(github_issue_author_is_allowed(&prof, &owner));
        assert!(!github_issue_author_is_allowed(&prof, &outsider));
        assert!(!github_issue_author_is_allowed(&prof, &missing));

        prof.publishing.github_issue_author_allowlist = Some(vec!["teammate".into()]);
        let teammate = serde_json::json!({"author": {"login": "TEAMMATE"}});
        assert!(github_issue_author_is_allowed(&prof, &teammate));
        assert!(!github_issue_author_is_allowed(&prof, &owner));

        prof.publishing.github_issue_author_allowlist = Some(vec![]);
        assert!(!github_issue_author_is_allowed(&prof, &teammate));
    }

    #[test]
    fn scan_available_tickets_includes_open_github_issues() {
        let _exec_guard = ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\nRecommended model: Gemini 3.5 Flash (Medium)\n","labels":[],"author":{"login":"owner"}}]"#;
        let gh_path = bin_dir.join("gh");
        fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&gh_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&gh_path, perms).unwrap();
        }
        let _guard = PathGuard::set(&bin_dir);

        let cfg = ticket_cfg(tmp.path());
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = "github".to_string();
        prof.repo = "owner/repo".to_string();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].ticket_path, "118");
        assert_eq!(candidates[0].work_id.as_deref(), Some("#118"));
        assert_eq!(candidates[0].recommended_backend.as_deref(), Some("agy"));
        assert_eq!(candidates[0].prior_attempt_count, 0);
        assert!(!candidates[0].has_active_mr);
    }

    #[test]
    fn scan_available_tickets_uses_native_identity_for_gitlab_issues() {
        let _exec_guard = ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let issue_json = r#"[{"iid":77,"title":"TICKET-9: Legacy title must not become identity","description":"Work ID: TICKET-9\nRecommended backend: codex","labels":[],"state":"opened"}]"#;
        let glab_path = bin_dir.join("glab");
        fs::write(
            &glab_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&glab_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&glab_path, perms).unwrap();
        }
        let _guard = PathGuard::set(&bin_dir);

        let cfg = ticket_cfg(tmp.path());
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = "gitlab".to_string();
        prof.repo = "group/project".to_string();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].ticket_path, "77");
        assert_eq!(candidates[0].work_id.as_deref(), Some("#77"));
        assert_eq!(
            candidates[0].title.as_deref(),
            Some("Legacy title must not become identity")
        );
    }

    #[test]
    fn scan_available_tickets_excludes_owner_decision_github_issues() {
        let _exec_guard = ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let issue_json = r#"[{"number":92,"title":"MS-5: Fleet ledger","body":"","labels":[{"name":"EXEC:OWNER-DECISION"}],"author":{"login":"owner"}}]"#;
        let gh_path = bin_dir.join("gh");
        fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&gh_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&gh_path, perms).unwrap();
        }
        let _guard = PathGuard::set(&bin_dir);
        let cfg = ticket_cfg(tmp.path());
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = "github".to_string();
        prof.repo = "owner/repo".to_string();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn scan_available_tickets_excludes_issue_already_archived_locally() {
        let _exec_guard = ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\n","labels":[],"author":{"login":"owner"}}]"#;
        let gh_path = bin_dir.join("gh");
        fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&gh_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&gh_path, perms).unwrap();
        }
        let _guard = PathGuard::set(&bin_dir);

        let closed_dir = tmp.path().join("docs/tickets/closed");
        fs::create_dir_all(&closed_dir).unwrap();
        fs::write(
            closed_dir.join("TICKET-101-fail-closed-version-drift.md"),
            "# TICKET-101: Fail closed\n\nGoal: test\n",
        )
        .unwrap();

        let cfg = ticket_cfg(tmp.path());
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = "github".to_string();
        prof.repo = "owner/repo".to_string();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &ledger::index_entries_by_work_id(&ledger::read_entries(&cfg).unwrap()),
        );
        assert!(
            candidates.is_empty(),
            "expected locally-archived TICKET-101 issue to be excluded, got {candidates:?}"
        );
    }
}
