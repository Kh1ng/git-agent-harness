use super::{github_api, gitlab_api, Profile};
use anyhow::Result;

#[allow(dead_code)]
const PLANNING_ISSUE_PAGE_SIZE: usize = 100;
#[allow(dead_code)]
const PLANNING_ISSUE_DEPENDENCY_LINK_TYPE: &str = "is_blocked_by";

#[allow(dead_code)]
fn planning_issue_idempotency_marker(packet: &PlanningIssuePacket) -> String {
    format!(
        "project={};plan={};packet={}",
        packet.target_project.trim(),
        packet.plan_id.trim(),
        packet.packet_key.trim()
    )
}

#[allow(dead_code)]
fn is_separator(ch: char) -> bool {
    ch == '-' || ch == '_' || ch == '/' || ch.is_whitespace()
}

#[allow(dead_code)]
fn normalize_planning_label_value(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if is_separator(ch) {
            if !last_dash && !out.is_empty() {
                out.push('-');
            }
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[allow(dead_code)]
fn normalize_priority_value(raw: &str) -> String {
    let value = raw.trim().trim_start_matches('p');
    if value.chars().all(|c| c.is_ascii_digit()) && !value.is_empty() {
        format!("p{value}")
    } else {
        normalize_planning_label_value(value)
    }
}

#[allow(dead_code)]
fn canonicalize_planning_label(label: &str) -> Option<(u8, String)> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(raw) = lower.strip_prefix("priority:") {
        return Some((0, format!("priority:{}", normalize_priority_value(raw))));
    }
    if let Some(raw) = lower.strip_prefix("prio:") {
        return Some((0, format!("priority:{}", normalize_priority_value(raw))));
    }
    if lower.starts_with('p') && lower.len() > 1 && lower[1..].chars().all(|c| c.is_ascii_digit()) {
        return Some((0, format!("priority:{}", normalize_priority_value(&lower))));
    }
    if lower.chars().all(|c| c.is_ascii_digit()) {
        return Some((0, format!("priority:p{lower}")));
    }
    if let Some(raw) = lower.strip_prefix("area:") {
        return Some((1, format!("area:{}", normalize_planning_label_value(raw))));
    }
    if let Some(raw) = lower.strip_prefix("risk:") {
        return Some((2, format!("risk:{}", normalize_planning_label_value(raw))));
    }
    if let Some(raw) = lower.strip_prefix("exec:") {
        return Some((3, format!("exec:{}", normalize_planning_label_value(raw))));
    }
    if let Some(raw) = lower.strip_prefix("execution:") {
        return Some((3, format!("exec:{}", normalize_planning_label_value(raw))));
    }
    Some((4, trimmed.to_string()))
}

#[allow(dead_code)]
fn canonicalize_planning_labels(labels: &[String]) -> Vec<String> {
    let mut labels = labels
        .iter()
        .filter_map(|label| canonicalize_planning_label(label))
        .collect::<Vec<_>>();
    labels.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    labels.dedup_by(|left, right| left.1 == right.1);
    labels.into_iter().map(|(_, label)| label).collect()
}

#[allow(dead_code)]
fn canonicalize_dependency_references(references: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for reference in references {
        let trimmed = reference.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !normalized.iter().any(|existing| existing == trimmed) {
            normalized.push(trimmed.to_string());
        }
    }
    normalized
}

#[allow(dead_code)]
fn planning_issue_preview(packet: &PlanningIssuePacket) -> PlanningIssuePreview {
    let dependency_references = canonicalize_dependency_references(&packet.dependency_references);
    let idempotency_marker = planning_issue_idempotency_marker(packet);
    let mut body = packet.body.trim_end().to_string();
    if !body.is_empty() {
        body.push_str("\n\n");
    }
    body.push_str("## Planning Metadata\n");
    body.push_str(&format!("- Plan ID: {}\n", packet.plan_id.trim()));
    body.push_str(&format!("- Packet Key: {}\n", packet.packet_key.trim()));
    body.push_str(&format!(
        "- Target Project: {}\n",
        packet.target_project.trim()
    ));
    body.push_str(&format!("- Idempotency Key: {}\n", idempotency_marker));
    if !dependency_references.is_empty() {
        body.push('\n');
        body.push_str("## Dependency References\n");
        for reference in &dependency_references {
            body.push_str(&format!("- {}\n", reference));
        }
    }

    PlanningIssuePreview {
        target_project: packet.target_project.trim().to_string(),
        title: packet.title.trim().to_string(),
        body: body.trim_end().to_string(),
        labels: canonicalize_planning_labels(&packet.labels),
        dependency_references,
        idempotency_marker,
    }
}

#[allow(dead_code)]
fn planning_issue_record_from_github_value(
    preview: &PlanningIssuePreview,
    value: &serde_json::Value,
) -> Result<PlanningIssueRecord> {
    let issue_number = value
        .get("number")
        .and_then(serde_json::Value::as_i64)
        .filter(|number| *number > 0)
        .map(|number| number.to_string())
        .ok_or_else(|| anyhow::anyhow!("GitHub issue payload missing number"))?;
    let url = value
        .get("html_url")
        .or_else(|| value.get("url"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("GitHub issue payload missing url"))?;
    let title = value
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let body = value
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let labels = value
        .get("labels")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(PlanningIssueRecord {
        target_project: preview.target_project.clone(),
        issue_number,
        url,
        title,
        body,
        labels,
        dependency_references: preview.dependency_references.clone(),
        idempotency_marker: preview.idempotency_marker.clone(),
    })
}

#[allow(dead_code)]
fn planning_issue_record_from_gitlab_value(
    preview: &PlanningIssuePreview,
    value: &serde_json::Value,
) -> Result<PlanningIssueRecord> {
    let issue_number = value
        .get("iid")
        .and_then(serde_json::Value::as_i64)
        .filter(|number| *number > 0)
        .map(|number| number.to_string())
        .ok_or_else(|| anyhow::anyhow!("GitLab issue payload missing iid"))?;
    let url = value
        .get("web_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("GitLab issue payload missing web_url"))?;
    let title = value
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let body = value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let labels = value
        .get("labels")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(PlanningIssueRecord {
        target_project: preview.target_project.clone(),
        issue_number,
        url,
        title,
        body,
        labels,
        dependency_references: preview.dependency_references.clone(),
        idempotency_marker: preview.idempotency_marker.clone(),
    })
}

#[allow(dead_code)]
fn planning_issue_matches_preview(
    record: &PlanningIssueRecord,
    preview: &PlanningIssuePreview,
) -> bool {
    record.title == preview.title
        && record.body == preview.body
        && record.labels == preview.labels
        && record.dependency_references == preview.dependency_references
}

#[allow(dead_code)]
fn provider_failed(reason: String) -> PlanningIssueWriteResult {
    PlanningIssueWriteResult::ProviderFailed { reason }
}

#[allow(dead_code)]
fn denied(reason: String) -> PlanningIssueWriteResult {
    PlanningIssueWriteResult::Denied { reason }
}

#[allow(dead_code)]
fn planning_issue_number_from_reference(reference: &str) -> Option<String> {
    let trimmed = reference.trim();
    if trimmed.is_empty() {
        return None;
    }
    let number = trimmed.strip_prefix('#').unwrap_or(trimmed);
    number
        .chars()
        .all(|c| c.is_ascii_digit())
        .then(|| number.to_string())
}

#[allow(dead_code)]
fn github_planning_issue_create(
    profile: &Profile,
    preview: &PlanningIssuePreview,
) -> Result<PlanningIssueRecord> {
    let endpoint = format!("repos/{}/issues", profile.repo);
    let mut fields = vec![
        ("title".to_string(), preview.title.clone()),
        ("body".to_string(), preview.body.clone()),
    ];
    for label in &preview.labels {
        fields.push(("labels[]".to_string(), label.clone()));
    }
    let refs = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let resp = github_api(profile, &endpoint, "POST", &refs)?;
    planning_issue_record_from_github_value(preview, &resp)
}

#[allow(dead_code)]
fn github_planning_issue_update(
    profile: &Profile,
    preview: &PlanningIssuePreview,
    issue_number: &str,
) -> Result<PlanningIssueRecord> {
    let endpoint = format!("repos/{}/issues/{issue_number}", profile.repo);
    let mut fields = vec![
        ("title".to_string(), preview.title.clone()),
        ("body".to_string(), preview.body.clone()),
    ];
    for label in &preview.labels {
        fields.push(("labels[]".to_string(), label.clone()));
    }
    let refs = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let resp = github_api(profile, &endpoint, "PATCH", &refs)?;
    planning_issue_record_from_github_value(preview, &resp)
}

#[allow(dead_code)]
fn gitlab_planning_issue_create(
    profile: &Profile,
    preview: &PlanningIssuePreview,
) -> Result<PlanningIssueRecord> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/issues");
    let labels = preview.labels.join(",");
    let mut fields = vec![
        ("title".to_string(), preview.title.clone()),
        ("description".to_string(), preview.body.clone()),
    ];
    if !labels.is_empty() {
        fields.push(("labels".to_string(), labels));
    }
    let refs = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let resp = gitlab_api(profile, &endpoint, "POST", &refs)?;
    planning_issue_record_from_gitlab_value(preview, &resp)
}

#[allow(dead_code)]
fn gitlab_planning_issue_update(
    profile: &Profile,
    preview: &PlanningIssuePreview,
    issue_number: &str,
) -> Result<PlanningIssueRecord> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/issues/{issue_number}");
    let labels = preview.labels.join(",");
    let mut fields = vec![
        ("title".to_string(), preview.title.clone()),
        ("description".to_string(), preview.body.clone()),
    ];
    if !labels.is_empty() {
        fields.push(("labels".to_string(), labels));
    }
    let refs = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let resp = gitlab_api(profile, &endpoint, "PUT", &refs)?;
    planning_issue_record_from_gitlab_value(preview, &resp)
}

#[allow(dead_code)]
fn github_planning_issue_list(profile: &Profile, page: usize) -> Result<Vec<serde_json::Value>> {
    let endpoint = format!(
        "repos/{}/issues?state=all&per_page={}&page={}",
        profile.repo, PLANNING_ISSUE_PAGE_SIZE, page
    );
    let resp = github_api(profile, &endpoint, "GET", &[])?;
    let items = resp
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("GitHub issue list did not return an array"))?;
    Ok(items)
}

#[allow(dead_code)]
fn github_find_planning_issue(
    profile: &Profile,
    preview: &PlanningIssuePreview,
) -> Result<Option<PlanningIssueRecord>> {
    let mut page = 1;
    loop {
        let items = github_planning_issue_list(profile, page)?;
        if items.is_empty() {
            return Ok(None);
        }
        for item in &items {
            if item.get("pull_request").is_some() {
                continue;
            }
            let body = item
                .get("body")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if !body.contains(&preview.idempotency_marker) {
                continue;
            }
            let record = planning_issue_record_from_github_value(preview, item)?;
            return Ok(Some(record));
        }
        if items.len() < PLANNING_ISSUE_PAGE_SIZE {
            return Ok(None);
        }
        page += 1;
    }
}

#[allow(dead_code)]
fn gitlab_planning_issue_list(profile: &Profile, page: usize) -> Result<Vec<serde_json::Value>> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!(
        "projects/{project_id}/issues?state=all&per_page={}&page={page}",
        PLANNING_ISSUE_PAGE_SIZE
    );
    let resp = gitlab_api(profile, &endpoint, "GET", &[])?;
    let items = resp
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("GitLab issue list did not return an array"))?;
    Ok(items)
}

#[allow(dead_code)]
fn gitlab_find_planning_issue(
    profile: &Profile,
    preview: &PlanningIssuePreview,
) -> Result<Option<PlanningIssueRecord>> {
    let mut page = 1;
    loop {
        let items = gitlab_planning_issue_list(profile, page)?;
        if items.is_empty() {
            return Ok(None);
        }
        for item in &items {
            let body = item
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if !body.contains(&preview.idempotency_marker) {
                continue;
            }
            let record = planning_issue_record_from_gitlab_value(preview, item)?;
            return Ok(Some(record));
        }
        if items.len() < PLANNING_ISSUE_PAGE_SIZE {
            return Ok(None);
        }
        page += 1;
    }
}

#[allow(dead_code)]
fn gitlab_planning_issue_current_links(
    profile: &Profile,
    issue_number: &str,
) -> Result<Vec<String>> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/issues/{issue_number}/links");
    let resp = gitlab_api(profile, &endpoint, "GET", &[])?;
    let items = resp
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("GitLab issue links did not return an array"))?;
    let mut refs = Vec::new();
    for item in items {
        if item.get("link_type").and_then(serde_json::Value::as_str)
            != Some(PLANNING_ISSUE_DEPENDENCY_LINK_TYPE)
        {
            continue;
        }
        if let Some(iid) = item
            .get("iid")
            .and_then(serde_json::Value::as_i64)
            .filter(|iid| *iid > 0)
        {
            refs.push(iid.to_string());
        }
    }
    Ok(refs)
}

#[allow(dead_code)]
fn gitlab_planning_issue_sync_links(
    profile: &Profile,
    issue_number: &str,
    dependency_references: &[String],
) -> Result<()> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let current = gitlab_planning_issue_current_links(profile, issue_number)?;
    for reference in dependency_references {
        let Some(target_issue_iid) = planning_issue_number_from_reference(reference) else {
            continue;
        };
        if current.iter().any(|existing| existing == &target_issue_iid) {
            continue;
        }
        let endpoint = format!("projects/{project_id}/issues/{issue_number}/links");
        gitlab_api(
            profile,
            &endpoint,
            "POST",
            &[
                ("target_project_id", project_id),
                ("target_issue_iid", &target_issue_iid),
                ("link_type", PLANNING_ISSUE_DEPENDENCY_LINK_TYPE),
            ],
        )?;
    }
    Ok(())
}

#[allow(dead_code)]
fn github_apply_planning_issue(
    profile: &Profile,
    preview: &PlanningIssuePreview,
) -> PlanningIssueWriteResult {
    match github_find_planning_issue(profile, preview) {
        Ok(Some(existing)) => {
            if planning_issue_matches_preview(&existing, preview) {
                return PlanningIssueWriteResult::AlreadyCurrent(existing);
            }
            match github_planning_issue_update(profile, preview, &existing.issue_number) {
                Ok(record) => PlanningIssueWriteResult::Updated(record),
                Err(err) => provider_failed(format!(
                    "gh api update planning issue failed: {}",
                    crate::redact::redact(&err.to_string())
                )),
            }
        }
        Ok(None) => match github_planning_issue_create(profile, preview) {
            Ok(record) => PlanningIssueWriteResult::Created(record),
            Err(err) => provider_failed(format!(
                "gh api create planning issue failed: {}",
                crate::redact::redact(&err.to_string())
            )),
        },
        Err(err) => provider_failed(format!(
            "gh api search planning issue failed: {}",
            crate::redact::redact(&err.to_string())
        )),
    }
}

#[allow(dead_code)]
fn gitlab_apply_planning_issue(
    profile: &Profile,
    preview: &PlanningIssuePreview,
) -> PlanningIssueWriteResult {
    if profile.provider_project_id.is_none() {
        return provider_failed("profile missing provider_project_id for gitlab".to_string());
    }
    match gitlab_find_planning_issue(profile, preview) {
        Ok(Some(existing)) => {
            let mut needs_update = !planning_issue_matches_preview(&existing, preview);
            match gitlab_planning_issue_current_links(profile, &existing.issue_number) {
                Ok(current_links) => {
                    let desired_links = preview
                        .dependency_references
                        .iter()
                        .filter_map(|reference| planning_issue_number_from_reference(reference))
                        .collect::<std::collections::BTreeSet<_>>();
                    let current_links = current_links
                        .into_iter()
                        .collect::<std::collections::BTreeSet<_>>();
                    if desired_links != current_links {
                        needs_update = true;
                    }
                }
                Err(err) => {
                    return provider_failed(format!(
                        "glab api read planning issue links failed: {}",
                        crate::redact::redact(&err.to_string())
                    ))
                }
            }

            if !needs_update {
                return PlanningIssueWriteResult::AlreadyCurrent(existing);
            }

            match gitlab_planning_issue_update(profile, preview, &existing.issue_number) {
                Ok(record) => {
                    if let Err(err) = gitlab_planning_issue_sync_links(
                        profile,
                        &record.issue_number,
                        &record.dependency_references,
                    ) {
                        return provider_failed(format!(
                            "glab api sync planning issue links failed: {}",
                            crate::redact::redact(&err.to_string())
                        ));
                    }
                    PlanningIssueWriteResult::Updated(record)
                }
                Err(err) => provider_failed(format!(
                    "glab api update planning issue failed: {}",
                    crate::redact::redact(&err.to_string())
                )),
            }
        }
        Ok(None) => match gitlab_planning_issue_create(profile, preview) {
            Ok(record) => {
                if let Err(err) = gitlab_planning_issue_sync_links(
                    profile,
                    &record.issue_number,
                    &record.dependency_references,
                ) {
                    return provider_failed(format!(
                        "glab api sync planning issue links failed: {}",
                        crate::redact::redact(&err.to_string())
                    ));
                }
                PlanningIssueWriteResult::Created(record)
            }
            Err(err) => provider_failed(format!(
                "glab api create planning issue failed: {}",
                crate::redact::redact(&err.to_string())
            )),
        },
        Err(err) => provider_failed(format!(
            "glab api search planning issue failed: {}",
            crate::redact::redact(&err.to_string())
        )),
    }
}

/// Preview the exact native issue payload that would be written for a
/// validated planning work packet.
#[allow(dead_code)]
pub fn preview_planning_issue(packet: &PlanningIssuePacket) -> PlanningIssueWriteResult {
    PlanningIssueWriteResult::Previewed(planning_issue_preview(packet))
}

/// Apply one validated planning packet as a provider-native issue, or return a
/// typed denial/provider-failure result.
#[allow(dead_code)]
pub fn apply_planning_issue(
    profile: &Profile,
    packet: &PlanningIssuePacket,
    allow_issue_write: bool,
    caller_authorized: bool,
) -> PlanningIssueWriteResult {
    if !allow_issue_write {
        return denied("repository policy disallows issue writes".to_string());
    }
    if !caller_authorized {
        return denied("caller did not provide explicit issue-write authorization".to_string());
    }

    let preview = planning_issue_preview(packet);
    match profile.provider.as_str() {
        "github" => github_apply_planning_issue(profile, &preview),
        "gitlab" => gitlab_apply_planning_issue(profile, &preview),
        other => provider_failed(format!("unsupported provider: {other}")),
    }
}
/// Provider-neutral packet for writing one validated planning work item as a
/// native GitHub/GitLab issue.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PlanningIssuePacket {
    pub target_project: String,
    pub plan_id: String,
    pub packet_key: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub dependency_references: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PlanningIssuePreview {
    pub target_project: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub dependency_references: Vec<String>,
    pub idempotency_marker: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PlanningIssueRecord {
    pub target_project: String,
    pub issue_number: String,
    pub url: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub dependency_references: Vec<String>,
    pub idempotency_marker: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PlanningIssueWriteResult {
    Previewed(PlanningIssuePreview),
    Created(PlanningIssueRecord),
    Updated(PlanningIssueRecord),
    AlreadyCurrent(PlanningIssueRecord),
    Denied { reason: String },
    ProviderFailed { reason: String },
}
