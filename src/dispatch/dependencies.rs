use super::issues::{fetch_dependency_issue, DependencyIssue, IssueDetails};
use crate::config::Profile;
use crate::models::{DependencyBlocker, DependencyObservation};
use crate::provider::{gitlab_api, provider_command};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone)]
enum CachedIssue {
    Found(DependencyIssue),
    Error(String),
}

/// Typed errors for dependency relationship failures that fail closed.
#[derive(Debug, Clone)]
pub enum DependencyRelationshipError {
    /// Permission denied when accessing provider API
    PermissionDenied { message: String },
    /// Pagination failed or exceeded limits
    PaginationError { message: String },
    /// Provider API version is unsupported
    UnsupportedServerVersion { message: String },
    /// Response from provider was malformed or invalid
    MalformedResponse { message: String },
    /// Other provider-specific error
    ProviderError { message: String },
}

impl std::fmt::Display for DependencyRelationshipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DependencyRelationshipError::PermissionDenied { message } => {
                write!(f, "permission denied: {message}")
            }
            DependencyRelationshipError::PaginationError { message } => {
                write!(f, "pagination error: {message}")
            }
            DependencyRelationshipError::UnsupportedServerVersion { message } => {
                write!(f, "unsupported server version: {message}")
            }
            DependencyRelationshipError::MalformedResponse { message } => {
                write!(f, "malformed response: {message}")
            }
            DependencyRelationshipError::ProviderError { message } => {
                write!(f, "provider error: {message}")
            }
        }
    }
}

impl std::error::Error for DependencyRelationshipError {}

/// Represents a dependency relationship source with provenance
#[derive(Debug, Clone)]
pub(crate) struct DependencySource {
    /// The issue number that has this dependency
    pub source_issue: String,
    /// The target dependency issue number
    pub target_issue: String,
    /// The provenance of this relationship
    pub provenance: String,
}

fn is_local_issue_reference(reference: &str) -> bool {
    !reference.is_empty() && reference.bytes().all(|byte| byte.is_ascii_digit())
}

fn dependency_identity(reference: &str) -> String {
    if is_local_issue_reference(reference) {
        format!("#{reference}")
    } else {
        reference.to_string()
    }
}

fn dependency_provider(reference: &str, fallback: &str) -> String {
    reference
        .split_once(':')
        .map(|(provider, _)| provider.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

/// Fetch native GitHub sub-issue relationships for a given issue.
/// Returns a list of issue numbers that are sub-issues (children) of the parent.
/// This reads the native GitHub sub-issue API relationship.
#[allow(dead_code)]
pub(crate) fn fetch_github_sub_issues(
    profile: &Profile,
    parent_number: &str,
) -> Result<Vec<String>, DependencyRelationshipError> {
    if profile.provider != "github" {
        return Ok(Vec::new());
    }

    let endpoint = format!("repos/{}/issues/{parent_number}/sub_issues", profile.repo);
    let output = provider_command("gh")
        .args(["api", "--method", "GET", &endpoint])
        .output()
        .map_err(|e| DependencyRelationshipError::ProviderError {
            message: format!("gh api list sub-issues failed: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let lower = stderr.to_ascii_lowercase();

        // Classify the error based on the response
        if lower.contains("403") || lower.contains("permission") || lower.contains("denied") {
            return Err(DependencyRelationshipError::PermissionDenied {
                message: format!("GitHub API permission denied for {endpoint}: {stderr}"),
            });
        }
        if lower.contains("404") || lower.contains("not found") {
            // Sub-issues API might not be available on older GitHub instances
            // or the endpoint might not exist - this is not an error, just no relationships
            return Ok(Vec::new());
        }
        if lower.contains("400") || lower.contains("bad request") {
            return Err(DependencyRelationshipError::MalformedResponse {
                message: format!("GitHub API bad request for {endpoint}: {stderr}"),
            });
        }
        if lower.contains("429") || lower.contains("rate limit") {
            return Err(DependencyRelationshipError::PaginationError {
                message: format!("GitHub API rate limited for {endpoint}: {stderr}"),
            });
        }

        return Err(DependencyRelationshipError::ProviderError {
            message: format!("GitHub API error for {endpoint}: {stderr}"),
        });
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        DependencyRelationshipError::MalformedResponse {
            message: format!("Failed to parse GitHub sub-issues response: {e}"),
        }
    })?;

    let issues =
        value
            .as_array()
            .ok_or_else(|| DependencyRelationshipError::MalformedResponse {
                message: format!("Expected array of sub-issues, got: {value}"),
            })?;

    let mut sub_issue_numbers = Vec::new();
    for issue in issues {
        if let Some(number) = issue["number"].as_u64() {
            let number = number.to_string();
            if number != parent_number {
                sub_issue_numbers.push(number);
            }
        }
    }

    Ok(sub_issue_numbers)
}

/// Fetch native GitLab issue link relationships (blocks type) for a given issue.
/// Returns a list of issue numbers that this issue is blocked by.
/// This reads the native GitLab issue links API and resolves incoming blocker
/// edges for dependency-style evaluation.
pub(crate) fn fetch_gitlab_blocks_links(
    profile: &Profile,
    issue_number: &str,
) -> Result<Vec<String>, DependencyRelationshipError> {
    if profile.provider != "gitlab" {
        return Ok(Vec::new());
    }

    let project_id = profile.provider_project_id.as_deref().ok_or_else(|| {
        DependencyRelationshipError::ProviderError {
            message: "profile missing provider_project_id for gitlab".to_string(),
        }
    })?;

    let endpoint = format!("projects/{project_id}/issues/{issue_number}/links");
    let result = gitlab_api(profile, &endpoint, "GET", &[]);

    match result {
        Ok(value) => {
            let links =
                value
                    .as_array()
                    .ok_or_else(|| DependencyRelationshipError::MalformedResponse {
                        message: format!("Expected array of issue links, got: {value}"),
                    })?;

            let mut blocked_numbers = Vec::new();
            for link in links {
                let link_type = link["link_type"].as_str().unwrap_or_default();
                if link_type != "blocks" && link_type != "is_blocked_by" {
                    continue;
                }

                let source_issue = link
                    .pointer("/source_issue/iid")
                    .and_then(|value| value.as_u64())
                    .map(|id| id.to_string())
                    .or_else(|| link["source_issue_iid"].as_u64().map(|id| id.to_string()))
                    .or_else(|| link["source_issue_iid"].as_str().map(ToString::to_string));

                let target_issue = link
                    .pointer("/target_issue/iid")
                    .and_then(|value| value.as_u64())
                    .map(|id| id.to_string())
                    .or_else(|| link["target_issue_iid"].as_u64().map(|id| id.to_string()))
                    .or_else(|| link["target_issue_iid"].as_str().map(ToString::to_string));

                if let (Some(source), Some(target)) = (source_issue, target_issue) {
                    match link_type {
                        "blocks" if target == issue_number => {
                            blocked_numbers.push(source);
                        }
                        "is_blocked_by" if source == issue_number => {
                            blocked_numbers.push(target);
                        }
                        _ => {}
                    }
                }
            }
            Ok(blocked_numbers)
        }
        Err(error) => {
            let error_str = error.to_string().to_ascii_lowercase();

            if error_str.contains("403")
                || error_str.contains("permission")
                || error_str.contains("denied")
            {
                Err(DependencyRelationshipError::PermissionDenied {
                    message: format!("GitLab API permission denied for {endpoint}: {error}"),
                })
            } else if error_str.contains("404") || error_str.contains("not found") {
                // Issue links might not be available - not an error, just no relationships
                Ok(Vec::new())
            } else if error_str.contains("429") || error_str.contains("rate limit") {
                Err(DependencyRelationshipError::PaginationError {
                    message: format!("GitLab API rate limited for {endpoint}: {error}"),
                })
            } else if error_str.contains("version") || error_str.contains("unsupported") {
                Err(DependencyRelationshipError::UnsupportedServerVersion {
                    message: format!("GitLab API unsupported version for {endpoint}: {error}"),
                })
            } else {
                Err(DependencyRelationshipError::ProviderError {
                    message: format!("GitLab API error for {endpoint}: {error}"),
                })
            }
        }
    }
}

/// Fetch native dependencies for an issue from provider APIs.
/// Returns a list of DependencySource representing the native relationships.
pub(crate) fn fetch_native_dependencies(
    profile: &Profile,
    issue_number: &str,
) -> Result<Vec<DependencySource>, DependencyRelationshipError> {
    match profile.provider.as_str() {
        "github" => Ok(Vec::new()),
        "gitlab" => {
            // For GitLab, fetch blocks links which indicate what blocks this issue.
            let blocked = fetch_gitlab_blocks_links(profile, issue_number)?;
            Ok(blocked
                .into_iter()
                .map(|target| DependencySource {
                    source_issue: issue_number.to_string(),
                    target_issue: target,
                    provenance: "gitlab_blocks_link".to_string(),
                })
                .collect())
        }
        _ => Ok(Vec::new()),
    }
}

/// Represents a cross-project dependency with explicit provider/project identity.
/// Cross-project dependencies MUST have explicit provider/project identity;
/// bare numbers are never inferred to be cross-project.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct CrossProjectDependency {
    pub provider: String,
    pub project_id: String,
    pub issue_number: String,
}

impl std::fmt::Display for CrossProjectDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}/#/{}",
            self.provider, self.project_id, self.issue_number
        )
    }
}

#[derive(Debug, Clone)]
struct DependencyFailure {
    code: &'static str,
    message: String,
}

fn parse_dependency_line(body: &str) -> Result<Option<Vec<String>>, DependencyFailure> {
    let candidates: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|line| line.to_ascii_lowercase().starts_with("blocked by"))
        .collect();
    if candidates.is_empty() {
        return Ok(None);
    }
    if candidates.len() != 1 {
        return Err(DependencyFailure {
            code: "dependency_ambiguous",
            message: "multiple 'Blocked by:' compatibility lines are not allowed".into(),
        });
    }

    let line = candidates[0];
    let Some(payload) = line.strip_prefix("Blocked by:") else {
        return Err(DependencyFailure {
            code: "dependency_malformed",
            message: "dependency line must use exact syntax 'Blocked by: #12, #34'".into(),
        });
    };
    if payload.trim().is_empty() {
        return Err(DependencyFailure {
            code: "dependency_malformed",
            message: "dependency line contains no issue references".into(),
        });
    }

    let mut seen = HashSet::new();
    let mut references = Vec::new();
    for raw in payload.split(',') {
        let token = raw.trim();

        // Check if this is a cross-project dependency reference
        // Format: provider:project_id#number (e.g., github:owner/repo#123, gitlab:12345#678)
        if token.contains(':') && token.contains('#') {
            // This is a cross-project dependency - keep project identity
            // in the reference so we do not collapse to a local number.
            let parts: Vec<&str> = token.split('#').collect();
            if parts.len() == 2 {
                let before_hash = parts[0]; // Should be "provider:project_id"
                let number = parts[1].trim();

                // Validate that we have a proper provider:project_id format
                let provider_parts: Vec<&str> = before_hash.split(':').collect();
                if provider_parts.len() != 2
                    || provider_parts[0].is_empty()
                    || provider_parts[1].is_empty()
                {
                    return Err(DependencyFailure {
                        code: "dependency_malformed",
                        message: format!("invalid cross-project dependency reference '{token}'"),
                    });
                }

                // Validate the issue number
                if number.is_empty()
                    || !number.bytes().all(|byte| byte.is_ascii_digit())
                    || number.starts_with('0')
                {
                    return Err(DependencyFailure {
                        code: "dependency_malformed",
                        message: format!("invalid cross-project dependency reference '{token}'"),
                    });
                }
                if !seen.insert(format!("{before_hash}#{number}")) {
                    return Err(DependencyFailure {
                        code: "dependency_duplicate",
                        message: format!("duplicate dependency reference '{token}'"),
                    });
                }
                references.push(format!("{before_hash}#{number}"));
                continue;
            }
        }

        // Handle same-project dependency reference
        let Some(number) = token.strip_prefix('#') else {
            return Err(DependencyFailure {
                code: "dependency_malformed",
                message: format!("invalid dependency reference '{token}'"),
            });
        };
        if number.is_empty()
            || !number.bytes().all(|byte| byte.is_ascii_digit())
            || number.starts_with('0')
        {
            return Err(DependencyFailure {
                code: "dependency_malformed",
                message: format!("invalid dependency reference '{token}'"),
            });
        }
        if !seen.insert(number.to_string()) {
            return Err(DependencyFailure {
                code: "dependency_duplicate",
                message: format!("duplicate dependency reference '#{number}'"),
            });
        }
        references.push(number.to_string());
    }
    Ok(Some(references))
}

fn normalize_state(state: Option<&str>) -> &'static str {
    match state.map(str::to_ascii_lowercase).as_deref() {
        Some("open" | "opened") => "open",
        Some("closed") => "closed",
        _ => "unknown",
    }
}

struct Resolver<'a, F>
where
    F: FnMut(&str) -> Result<DependencyIssue, String>,
{
    provider: &'a str,
    cache: HashMap<String, CachedIssue>,
    fetch: F,
}

impl<'a, F> Resolver<'a, F>
where
    F: FnMut(&str) -> Result<DependencyIssue, String>,
{
    fn resolve(&mut self, number: &str) -> CachedIssue {
        if let Some(cached) = self.cache.get(number) {
            return cached.clone();
        }
        let resolved = match (self.fetch)(number) {
            Ok(issue) => CachedIssue::Found(issue),
            Err(error) => CachedIssue::Error(error),
        };
        self.cache.insert(number.to_string(), resolved.clone());
        resolved
    }

    #[allow(dead_code)]
    fn walk(
        &mut self,
        current: &str,
        references: &[String],
        stack: &mut Vec<String>,
        observations: &mut Vec<DependencyObservation>,
    ) -> Result<bool, DependencyFailure> {
        let mut provenance_map = HashMap::new();
        self.walk_with_provenance(
            current,
            references,
            stack,
            observations,
            &mut provenance_map,
        )
    }

    /// Walk the dependency graph with provenance tracking.
    /// This allows tracking which source (native vs body) each dependency came from.
    fn walk_with_provenance(
        &mut self,
        current: &str,
        references: &[String],
        stack: &mut Vec<String>,
        observations: &mut Vec<DependencyObservation>,
        provenance_map: &mut HashMap<String, String>,
    ) -> Result<bool, DependencyFailure> {
        let mut has_open = false;
        let mut blocked_error = None;
        for number in references {
            if !is_local_issue_reference(number) {
                let provider = dependency_provider(number, self.provider);
                if let Some(provenance) = provenance_map.get(number).cloned() {
                    observations.push(DependencyObservation {
                        identity: dependency_identity(number),
                        provider,
                        provider_state: None,
                        normalized_state: "inaccessible".into(),
                        provenance: Some(provenance),
                    });
                } else {
                    observations.push(DependencyObservation {
                        identity: dependency_identity(number),
                        provider,
                        provider_state: None,
                        normalized_state: "inaccessible".into(),
                        provenance: None,
                    });
                }
                if blocked_error.is_none() {
                    blocked_error = Some(DependencyFailure {
                        code: "dependency_query_failed",
                        message: format!("cannot resolve cross-project dependency '{number}'"),
                    });
                }
                continue;
            }

            if number == current || stack.contains(number) {
                let mut cycle = stack.clone();
                cycle.push(number.clone());
                return Err(DependencyFailure {
                    code: "dependency_cycle",
                    message: format!(
                        "dependency cycle detected: {}",
                        cycle
                            .iter()
                            .map(|part| dependency_identity(part))
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ),
                });
            }

            let issue = match self.resolve(number) {
                CachedIssue::Found(issue) => issue,
                CachedIssue::Error(error) => {
                    let lower = error.to_ascii_lowercase();
                    let (code, label) = if lower.contains("not found") || lower.contains("404") {
                        ("dependency_missing", "missing")
                    } else {
                        ("dependency_query_failed", "inaccessible")
                    };
                    // Store provenance for this observation if available
                    if let Some(provenance) = provenance_map.get(number).cloned() {
                        observations.push(DependencyObservation {
                            identity: dependency_identity(number),
                            provider: self.provider.to_string(),
                            provider_state: None,
                            normalized_state: label.into(),
                            provenance: Some(provenance),
                        });
                    } else {
                        observations.push(DependencyObservation {
                            identity: dependency_identity(number),
                            provider: self.provider.to_string(),
                            provider_state: None,
                            normalized_state: label.into(),
                            provenance: None,
                        });
                    }
                    if blocked_error.is_none() {
                        blocked_error = Some(DependencyFailure {
                            code,
                            message: format!(
                                "could not resolve dependency {}: {error}",
                                dependency_identity(number)
                            ),
                        });
                    }
                    continue;
                }
            };

            let normalized = normalize_state(issue.state.as_deref());
            let identity = format!("#{}", issue.number);

            if !observations.iter().any(|seen| seen.identity == identity) {
                // Apply provenance from the map if available
                let provenance = provenance_map.get(number).cloned();
                let provenance_for_obs = provenance.clone();
                observations.push(DependencyObservation {
                    identity: identity.clone(),
                    provider: self.provider.to_string(),
                    provider_state: issue.state.clone(),
                    normalized_state: normalized.into(),
                    provenance: provenance_for_obs,
                });
                // Store the identity for provenance mapping
                if let Some(provenance) = provenance {
                    provenance_map.insert(identity, provenance);
                }
            }

            match normalized {
                "closed" => continue,
                "unknown" => {
                    if blocked_error.is_none() {
                        blocked_error = Some(DependencyFailure {
                            code: "dependency_unknown_state",
                            message: format!(
                                "dependency {} has unknown provider state",
                                dependency_identity(number)
                            ),
                        });
                    }
                    continue;
                }
                "open" => has_open = true,
                _ => unreachable!(),
            }

            // For nested dependencies, we need to parse the body and continue walking
            // This maintains compatibility with the existing body-based dependency parsing
            let nested = parse_dependency_line(&issue.body)?;
            if let Some(nested) = nested {
                stack.push(number.clone());
                let nested_open = self.walk_with_provenance(
                    number,
                    &nested,
                    stack,
                    observations,
                    provenance_map,
                )?;
                stack.pop();
                has_open |= nested_open;
            }
        }
        if let Some(error) = blocked_error {
            Err(error)
        } else {
            Ok(has_open)
        }
    }
}

#[allow(dead_code)]
fn evaluate_with<F>(provider: &str, issues: &[IssueDetails], mut fetch: F) -> Vec<DependencyBlocker>
where
    F: FnMut(&str) -> Result<DependencyIssue, String>,
{
    let mut cache = HashMap::new();
    for issue in issues {
        cache.insert(
            issue.number.clone(),
            CachedIssue::Found(DependencyIssue {
                number: issue.number.clone(),
                body: issue.body.clone(),
                state: issue.state.clone(),
            }),
        );
    }
    let mut resolver = Resolver {
        provider,
        cache,
        fetch: &mut fetch,
    };
    let mut blockers = Vec::new();

    for issue in issues {
        let parsed = parse_dependency_line(&issue.body);
        let mut observations = Vec::new();
        let outcome = match parsed {
            Ok(None) => continue,
            Ok(Some(references)) => resolver
                .walk(
                    &issue.number,
                    &references,
                    &mut vec![issue.number.clone()],
                    &mut observations,
                )
                .and_then(|has_open| {
                    if has_open {
                        Err(DependencyFailure {
                            code: "dependency_open",
                            message: "one or more declared prerequisites remain open".into(),
                        })
                    } else {
                        Ok(false)
                    }
                }),
            Err(error) => Err(error),
        };
        if let Err(error) = outcome {
            blockers.push(DependencyBlocker {
                ticket_path: issue.number.clone(),
                work_id: format!("#{}", issue.number),
                title: issue.title.clone(),
                reason_code: error.code.into(),
                reason: error.message,
                dependencies: observations,
            });
        }
    }
    blockers
}

/// Enhanced dependency evaluation that merges native provider relationships
/// with body-declared relationships, deduplicating exact matches.
///
/// For GitLab: reads native blocks issue links via the issue links API. GitHub
/// native sub-issue responses are not interpreted as blocker relationships.
///
/// Both sources are normalized into the same dependency identity/state model
/// with provenance metadata indicating the source.
pub(super) fn evaluate_issue_dependencies(
    profile: &Profile,
    issues: &[IssueDetails],
) -> Vec<DependencyBlocker> {
    let mut blockers = Vec::new();

    // Build a set of all known issue numbers for quick lookup
    let _known_issue_numbers: HashSet<String> =
        issues.iter().map(|issue| issue.number.clone()).collect();

    // First, collect all dependency relationships from both sources
    let mut all_dependencies: HashMap<String, Vec<DependencySource>> = HashMap::new();

    // 1. Collect body-declared relationships (canonical fallback)
    for issue in issues {
        if let Ok(Some(references)) = parse_dependency_line(&issue.body) {
            let entry = all_dependencies.entry(issue.number.clone()).or_default();
            for ref_num in references {
                entry.push(DependencySource {
                    source_issue: issue.number.clone(),
                    target_issue: ref_num,
                    provenance: "body".to_string(),
                });
            }
        }
    }

    // 2. Collect native provider relationships
    // For now, we'll attempt to fetch native relationships for each issue
    // In a real implementation, this would be batched for efficiency
    for issue in issues {
        let native_deps = match fetch_native_dependencies(profile, &issue.number) {
            Ok(deps) => deps,
            Err(error) => {
                let reason_code = match &error {
                    DependencyRelationshipError::UnsupportedServerVersion { .. } => None,
                    _ => Some("dependency_query_failed"),
                };

                if let Some(reason_code) = reason_code {
                    blockers.push(DependencyBlocker {
                        ticket_path: issue.number.clone(),
                        work_id: format!("#{}", issue.number),
                        title: issue.title.clone(),
                        reason_code: reason_code.into(),
                        reason: error.to_string(),
                        dependencies: Vec::new(),
                    });
                }
                continue;
            }
        };

        for dep in native_deps {
            let entry = all_dependencies
                .entry(dep.source_issue.clone())
                .or_default();
            entry.push(dep);
        }
    }

    // 3. Deduplicate exact matches - keep only one copy of each unique (source, target) pair
    // For duplicates, prefer native provider relationships over body-declared ones
    let mut deduplicated: HashMap<String, Vec<DependencySource>> = HashMap::new();
    for (source, deps) in all_dependencies {
        let unique_deps = deps;

        // Second pass: prefer native provider relationships over body ones
        let mut final_deps = Vec::new();
        let mut deduped_map: BTreeMap<String, DependencySource> = BTreeMap::new();
        for dep in unique_deps {
            let key = format!("{}:{}", dep.source_issue, dep.target_issue);
            let is_native = matches!(dep.provenance.as_str(), "gitlab_blocks_link");
            match deduped_map.get(&key) {
                Some(existing) if existing.provenance == "body" && is_native => {
                    deduped_map.insert(key, dep);
                }
                Some(_) => {}
                None => {
                    deduped_map.insert(key, dep);
                }
            }
        }
        final_deps.extend(deduped_map.into_values());

        deduplicated.insert(source, final_deps);
    }
    // 4. Now evaluate the merged dependency graph
    let mut cache = HashMap::new();
    for issue in issues {
        cache.insert(
            issue.number.clone(),
            CachedIssue::Found(DependencyIssue {
                number: issue.number.clone(),
                body: issue.body.clone(),
                state: issue.state.clone(),
            }),
        );
    }

    let mut resolver = Resolver {
        provider: &profile.provider,
        cache,
        fetch: |number| {
            fetch_dependency_issue(profile, number).map_err(|error| format!("{error:#}"))
        },
    };

    for issue in issues {
        let deps = deduplicated.get(&issue.number).cloned().unwrap_or_default();

        if deps.is_empty() {
            // No dependencies for this issue
            continue;
        }

        // Extract just the target issue numbers for the walk function
        let target_numbers: Vec<String> = deps.iter().map(|dep| dep.target_issue.clone()).collect();

        // Initialize provenance map with the provenance for each dependency
        // Use the raw issue number (without #) as the key for consistency
        let mut provenance_map: HashMap<String, String> = deps
            .iter()
            .map(|dep| (dep.target_issue.clone(), dep.provenance.clone()))
            .collect();

        let mut observations = Vec::new();

        let outcome = resolver
            .walk_with_provenance(
                &issue.number,
                &target_numbers,
                &mut vec![issue.number.clone()],
                &mut observations,
                &mut provenance_map,
            )
            .and_then(|has_open| {
                if has_open {
                    Err(DependencyFailure {
                        code: "dependency_open",
                        message: "one or more declared prerequisites remain open".into(),
                    })
                } else {
                    Ok(false)
                }
            });

        // Resolve provenance for nested dependencies where a parent provenance
        // was available but the child is discovered from body references.
        for obs in &mut observations {
            if obs.provenance.is_none() {
                let raw_identity = obs.identity.trim_start_matches('#').to_string();
                if let Some(provenance) = provenance_map.get(&obs.identity) {
                    obs.provenance = Some(provenance.clone());
                } else if let Some(provenance) = provenance_map.get(&raw_identity) {
                    obs.provenance = Some(provenance.clone());
                }
            }
        }

        if let Err(error) = outcome {
            blockers.push(DependencyBlocker {
                ticket_path: issue.number.clone(),
                work_id: format!("#{}", issue.number),
                title: issue.title.clone(),
                reason_code: error.code.into(),
                reason: error.message,
                dependencies: observations,
            });
        }
    }
    blockers
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    use crate::test_support::PathGuard;

    fn issue(number: &str, state: Option<&str>, body: &str) -> IssueDetails {
        IssueDetails {
            number: number.into(),
            title: format!("Issue {number}"),
            body: body.into(),
            labels: vec![],
            state: state.map(str::to_string),
        }
    }

    fn write_fake_bin(path: &std::path::Path, name: &str, body: &str) {
        let path = path.join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }

    fn github_test_profile() -> Profile {
        Profile {
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
            delivery_mode: crate::config::DeliveryMode::default(),
            prune_older_than_days: None,
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
            max_open_managed_mrs: None,
            notify_command: None,
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
            review_hard_timeout_seconds: None,
            validation_timeout_seconds: None,
            routing: crate::config::RoutingPolicy::default(),
            publishing: Default::default(),
            external_credential_scopes: HashMap::new(),
            pacing: Default::default(),
        }
    }

    fn gitlab_test_profile() -> Profile {
        let mut profile = github_test_profile();
        profile.provider = "gitlab".into();
        profile.repo = "group/repo".into();
        profile.provider_api_base = Some("https://gitlab.example.com/api/v4".into());
        profile.provider_project_id = Some("42".into());
        profile
    }

    #[test]
    fn strict_parser_rejects_malformed_duplicate_self_and_ambiguous_lines() {
        assert_eq!(parse_dependency_line("no dependencies").unwrap(), None);
        for body in [
            "Blocked by #2",
            "blocked by: #2",
            "Blocked by: #2, 3",
            "Blocked by: #2, #2",
            "Blocked by: #2\nBlocked by: #3",
        ] {
            assert!(parse_dependency_line(body).is_err(), "{body}");
        }

        let roots = vec![issue("2", Some("OPEN"), "Blocked by: #2")];
        let blockers = evaluate_with("github", &roots, |_| unreachable!());
        assert_eq!(blockers[0].reason_code, "dependency_cycle");
    }

    #[test]
    fn github_states_block_open_release_closed_and_cache_shared_references() {
        let roots = vec![
            issue("1", Some("OPEN"), "Blocked by: #9"),
            issue("2", Some("OPEN"), "Blocked by: #9"),
        ];
        let calls = RefCell::new(0);
        let blockers = evaluate_with("github", &roots, |_| {
            *calls.borrow_mut() += 1;
            Ok(DependencyIssue {
                number: "9".into(),
                body: String::new(),
                state: Some("OPEN".into()),
            })
        });
        assert_eq!(blockers.len(), 2);
        assert_eq!(*calls.borrow(), 1, "one controller tick must share lookups");

        let released = evaluate_with("github", &roots, |_| {
            Ok(DependencyIssue {
                number: "9".into(),
                body: String::new(),
                state: Some("CLOSED".into()),
            })
        });
        assert!(released.is_empty());
    }

    #[test]
    fn gitlab_cycle_missing_unknown_and_provider_error_fail_closed() {
        let roots = vec![
            issue("1", Some("opened"), "Blocked by: #2"),
            issue("2", Some("opened"), "Blocked by: #1"),
        ];
        let cycle = evaluate_with("gitlab", &roots, |_| unreachable!());
        assert!(cycle
            .iter()
            .all(|block| block.reason_code == "dependency_cycle"));

        for (error, expected) in [
            ("404 not found", "dependency_missing"),
            ("permission denied", "dependency_query_failed"),
        ] {
            let blocked = evaluate_with(
                "gitlab",
                &[issue("3", Some("opened"), "Blocked by: #99")],
                |_| Err(error.into()),
            );
            assert_eq!(blocked[0].reason_code, expected);
        }

        let unknown = evaluate_with(
            "gitlab",
            &[issue("4", Some("opened"), "Blocked by: #8")],
            |_| {
                Ok(DependencyIssue {
                    number: "8".into(),
                    body: String::new(),
                    state: None,
                })
            },
        );
        assert_eq!(unknown[0].reason_code, "dependency_unknown_state");
    }

    #[test]
    fn cross_project_dependency_parsing_handles_provider_prefix() {
        // Test parsing of cross-project dependency references
        // Format: provider:project_id#number
        assert_eq!(
            parse_dependency_line("Blocked by: github:owner/repo#123").unwrap(),
            Some(vec!["github:owner/repo#123".to_string()])
        );
        assert_eq!(
            parse_dependency_line("Blocked by: gitlab:12345#678").unwrap(),
            Some(vec!["gitlab:12345#678".to_string()])
        );

        // Test mixed same-project and cross-project
        assert_eq!(
            parse_dependency_line("Blocked by: #123, github:owner/repo#456").unwrap(),
            Some(vec!["123".to_string(), "github:owner/repo#456".to_string()])
        );

        // Test malformed cross-project references are rejected
        assert!(parse_dependency_line("Blocked by: github:owner/repo").is_err());
        assert!(parse_dependency_line("Blocked by: github:#123").is_err());
        assert!(parse_dependency_line("Blocked by: :#123").is_err());
    }

    #[test]
    fn gitlab_native_and_body_dependencies_are_deduplicated_with_native_precedence() {
        let profile = gitlab_test_profile();
        let bin_dir = TempDir::new().unwrap();
        let _path = PathGuard::set(bin_dir.path());
        crate::provider::set_test_provider_path(bin_dir.path().to_str().unwrap());

        write_fake_bin(
            bin_dir.path(),
            "glab",
            "#!/bin/sh\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/1/links\" ]; then\n  printf '%s\n' '[{\"link_type\":\"is_blocked_by\",\"source_issue\":{\"iid\":1},\"target_issue\":{\"iid\":2}}]'\n  exit 0\nelif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/2\" ]; then\n  printf '%s\n' '{\"iid\":2,\"description\":\"\",\"state\":\"open\"}'\n  exit 0\nelif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/2/links\" ]; then\n  printf '%s\n' '[]'\n  exit 0\nelse\n  echo \"unexpected glab invocation: $*\" >&2\n  exit 1\nfi\n",
        );

        let issues = vec![
            issue("1", Some("OPEN"), "Blocked by: #2"),
            issue("2", Some("OPEN"), ""),
        ];
        let blockers = evaluate_issue_dependencies(&profile, &issues);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].dependencies.len(), 1);
        assert_eq!(blockers[0].dependencies[0].identity, "#2");
        assert_eq!(
            blockers[0].dependencies[0].provenance.as_deref(),
            Some("gitlab_blocks_link")
        );
    }

    #[test]
    fn evaluate_issue_dependencies_fails_closed_when_native_lookup_fails() {
        let profile = gitlab_test_profile();
        let bin_dir = TempDir::new().unwrap();
        let _path = PathGuard::set(bin_dir.path());
        crate::provider::set_test_provider_path(bin_dir.path().to_str().unwrap());

        write_fake_bin(
            bin_dir.path(),
            "glab",
            "#!/bin/sh\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/1/links\" ]; then\n  echo '403 permission denied' >&2\n  exit 1\nelse\n  echo \"unexpected glab invocation: $*\" >&2\n  exit 1\nfi\n",
        );

        let blockers = evaluate_issue_dependencies(&profile, &[issue("1", Some("OPEN"), "")]);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].reason_code, "dependency_query_failed");
    }

    #[test]
    fn evaluate_issue_dependencies_falls_back_to_body_only_when_native_gitlab_api_unsupported() {
        let profile = gitlab_test_profile();
        let bin_dir = TempDir::new().unwrap();
        let _path = PathGuard::set(bin_dir.path());
        crate::provider::set_test_provider_path(bin_dir.path().to_str().unwrap());

        write_fake_bin(
            bin_dir.path(),
            "glab",
            "#!/bin/sh\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/1/links\" ]; then\n  echo 'old API version unsupported' >&2\n  exit 1\nelif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/2/links\" ]; then\n  printf '%s\n' '[]'\n  exit 0\nelif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/2\" ]; then\n  printf '%s\n' '{\"iid\":2,\"description\":\"\",\"state\":\"closed\"}'\n  exit 0\nelse\n  echo \"unexpected glab invocation: $*\" >&2\n  exit 1\nfi\n",
        );

        let issues = vec![
            issue("1", Some("OPEN"), "Blocked by: #2"),
            issue("2", Some("CLOSED"), ""),
        ];
        let blockers = evaluate_issue_dependencies(&profile, &issues);
        assert!(blockers.is_empty());
    }

    #[test]
    fn walk_collects_all_references_when_cross_project_dependency_is_present() {
        let mut calls = Vec::new();
        let blockers = evaluate_with(
            "github",
            &[issue(
                "1",
                Some("OPEN"),
                "Blocked by: #2, github:owner/repo#99, #3",
            )],
            |number| {
                calls.push(number.to_string());
                Ok(DependencyIssue {
                    number: number.to_string(),
                    body: String::new(),
                    state: Some("CLOSED".into()),
                })
            },
        );

        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].reason_code, "dependency_query_failed");
        let identities: Vec<_> = blockers[0]
            .dependencies
            .iter()
            .map(|obs| obs.identity.as_str())
            .collect();
        assert!(identities.contains(&"#2"));
        assert!(identities.contains(&"github:owner/repo#99"));
        assert!(identities.contains(&"#3"));
        assert_eq!(calls, vec!["2".to_string(), "3".to_string()]);
    }

    #[test]
    fn fetch_gitlab_blocks_links_uses_nested_source_issue_and_rejects_mixed_types() {
        let profile = gitlab_test_profile();
        let bin_dir = TempDir::new().unwrap();
        let _path = PathGuard::set(bin_dir.path());
        crate::provider::set_test_provider_path(bin_dir.path().to_str().unwrap());

        write_fake_bin(
            bin_dir.path(),
            "glab",
            "#!/bin/sh\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/1/links\" ]; then\n  printf '%s\n' '[{\"link_type\":\"is_blocked_by\",\"source_issue\":{\"iid\":1},\"target_issue\":{\"iid\":10},\"target_project_id\":42},{\"link_type\":\"blocks\",\"source_issue\":{\"iid\":1},\"target_issue\":{\"iid\":99},\"target_project_id\":42}]'\n  exit 0\nelif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/issues/10/links\" ]; then\n  printf '%s\n' '[]'\n  exit 0\nelse\n  echo \"unexpected glab invocation: $*\" >&2\n  exit 1\nfi\n",
        );

        let dependencies = fetch_gitlab_blocks_links(&profile, "1").unwrap();
        assert_eq!(dependencies, vec!["10".to_string(),]);
    }

    #[test]
    fn dependency_observation_includes_provenance() {
        // Test that DependencyObservation includes provenance metadata
        let observation = DependencyObservation {
            identity: "#123".to_string(),
            provider: "github".to_string(),
            provider_state: Some("open".to_string()),
            normalized_state: "open".to_string(),
            provenance: Some("body".to_string()),
        };

        assert_eq!(observation.provenance, Some("body".to_string()));
    }

    #[test]
    fn dependency_relationship_error_types_are_send_sync() {
        // Ensure error types can be used across threads (Send + Sync)
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<DependencyRelationshipError>();
    }

    #[test]
    fn dependency_relationship_error_display_formats_correctly() {
        use std::fmt::Write;

        let mut output = String::new();

        let error = DependencyRelationshipError::PermissionDenied {
            message: "access denied".to_string(),
        };
        write!(&mut output, "{}", error).unwrap();
        assert!(output.contains("permission denied"));
        assert!(output.contains("access denied"));

        output.clear();
        let error = DependencyRelationshipError::PaginationError {
            message: "rate limited".to_string(),
        };
        write!(&mut output, "{}", error).unwrap();
        assert!(output.contains("pagination error"));
        assert!(output.contains("rate limited"));

        output.clear();
        let error = DependencyRelationshipError::UnsupportedServerVersion {
            message: "old API".to_string(),
        };
        write!(&mut output, "{}", error).unwrap();
        assert!(output.contains("unsupported server version"));
        assert!(output.contains("old API"));

        output.clear();
        let error = DependencyRelationshipError::MalformedResponse {
            message: "invalid JSON".to_string(),
        };
        write!(&mut output, "{}", error).unwrap();
        assert!(output.contains("malformed response"));
        assert!(output.contains("invalid JSON"));
    }
}
