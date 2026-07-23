use super::issues::{
    fetch_dependency_issue, DependencyIssue, DependencyLookupTarget, IssueDetails,
};
use crate::config::Profile;
use crate::models::{DependencyBlocker, DependencyObservation};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
enum CachedIssue {
    Found(DependencyIssue),
    Error(String),
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

fn dependency_targets_from_body(
    body: &str,
) -> Result<Option<Vec<DependencyLookupTarget>>, DependencyFailure> {
    parse_dependency_line(body).map(|parsed| {
        parsed.map(|numbers| {
            numbers
                .into_iter()
                .map(DependencyLookupTarget::same_project)
                .collect::<Vec<_>>()
        })
    })
}

fn dedupe_dependency_targets(targets: Vec<DependencyLookupTarget>) -> Vec<DependencyLookupTarget> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for target in targets {
        if seen.insert(target.cache_key("dedupe")) {
            deduped.push(target);
        }
    }
    deduped
}

fn dependency_targets_for_issue(
    issue: &DependencyIssue,
) -> Result<Vec<DependencyLookupTarget>, DependencyFailure> {
    Ok(dependency_targets_from_body(&issue.body)?
        .map(dedupe_dependency_targets)
        .unwrap_or_default())
}

fn dependency_identity_list(targets: &[DependencyLookupTarget]) -> String {
    targets
        .iter()
        .map(DependencyLookupTarget::display_identity)
        .collect::<Vec<_>>()
        .join(", ")
}

struct Resolver<'a, F>
where
    F: FnMut(&DependencyLookupTarget) -> Result<DependencyIssue, String>,
{
    provider: &'a str,
    cache: HashMap<String, CachedIssue>,
    fetch: F,
}

impl<'a, F> Resolver<'a, F>
where
    F: FnMut(&DependencyLookupTarget) -> Result<DependencyIssue, String>,
{
    fn resolve(&mut self, target: &DependencyLookupTarget) -> CachedIssue {
        let cache_key = target.cache_key(self.provider);
        if let Some(cached) = self.cache.get(&cache_key) {
            return cached.clone();
        }
        let resolved = match (self.fetch)(target) {
            Ok(issue) => CachedIssue::Found(issue),
            Err(error) => CachedIssue::Error(error),
        };
        self.cache.insert(cache_key, resolved.clone());
        resolved
    }

    fn walk(
        &mut self,
        current: &DependencyLookupTarget,
        references: &[DependencyLookupTarget],
        stack: &mut Vec<DependencyLookupTarget>,
        observations: &mut Vec<DependencyObservation>,
    ) -> Result<bool, DependencyFailure> {
        let current_key = current.cache_key(self.provider);
        let mut has_open = false;
        for target in references {
            let target_key = target.cache_key(self.provider);
            if target_key == current_key
                || stack
                    .iter()
                    .any(|seen| seen.cache_key(self.provider) == target_key)
            {
                let mut cycle = stack
                    .iter()
                    .map(DependencyLookupTarget::display_identity)
                    .collect::<Vec<_>>();
                cycle.push(target.display_identity());
                return Err(DependencyFailure {
                    code: "dependency_cycle",
                    message: format!("dependency cycle detected: {}", cycle.join(" -> ")),
                });
            }

            let issue = match self.resolve(target) {
                CachedIssue::Found(issue) => issue,
                CachedIssue::Error(error) => {
                    let lower = error.to_ascii_lowercase();
                    let (code, label) = if lower.contains("not found") || lower.contains("404") {
                        ("dependency_missing", "missing")
                    } else {
                        ("dependency_query_failed", "inaccessible")
                    };
                    observations.push(DependencyObservation {
                        identity: target.display_identity(),
                        provider: self.provider.to_string(),
                        provider_state: None,
                        normalized_state: label.into(),
                    });
                    return Err(DependencyFailure {
                        code,
                        message: format!(
                            "could not resolve dependency {}: {error}",
                            target.display_identity()
                        ),
                    });
                }
            };

            let normalized = normalize_state(issue.state.as_deref());
            if !observations
                .iter()
                .any(|seen| seen.identity == target.display_identity())
            {
                observations.push(DependencyObservation {
                    identity: target.display_identity(),
                    provider: self.provider.to_string(),
                    provider_state: issue.state.clone(),
                    normalized_state: normalized.into(),
                });
            }
            match normalized {
                "closed" => continue,
                "unknown" => {
                    return Err(DependencyFailure {
                        code: "dependency_unknown_state",
                        message: format!(
                            "dependency {} has unknown provider state",
                            target.display_identity()
                        ),
                    });
                }
                "open" => has_open = true,
                _ => unreachable!(),
            }

            if normalized == "open" {
                let nested = dependency_targets_for_issue(&issue)?;
                if !nested.is_empty() {
                    stack.push(target.clone());
                    let nested_open = self.walk(target, &nested, stack, observations)?;
                    stack.pop();
                    has_open |= nested_open;
                }
            }
        }
        Ok(has_open)
    }
}

fn evaluate_with<F>(provider: &str, issues: &[IssueDetails], mut fetch: F) -> Vec<DependencyBlocker>
where
    F: FnMut(&DependencyLookupTarget) -> Result<DependencyIssue, String>,
{
    let cache = HashMap::new();
    let mut resolver = Resolver {
        provider,
        cache,
        fetch: &mut fetch,
    };
    let mut blockers = Vec::new();

    for issue in issues {
        let mut observations = Vec::new();
        let current_target = DependencyLookupTarget::same_project(issue.number.clone());
        let references = match dependency_targets_from_body(&issue.body) {
            Ok(Some(body_refs)) => body_refs,
            Ok(None) => continue,
            Err(error) => {
                blockers.push(DependencyBlocker {
                    ticket_path: issue.number.clone(),
                    work_id: format!("#{}", issue.number),
                    title: issue.title.clone(),
                    reason_code: error.code.into(),
                    reason: error.message,
                    dependencies: observations,
                    eligible_when: "eligible when all prerequisites are closed".into(),
                });
                continue;
            }
        };
        let outcome = resolver
            .walk(
                &current_target,
                &references,
                &mut vec![current_target.clone()],
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
            });
        if let Err(error) = outcome {
            let eligible_when = if observations.is_empty() {
                dependency_targets_from_body(&issue.body)
                    .ok()
                    .and_then(|parsed| parsed)
                    .map(|body_refs| {
                        format!(
                            "eligible when {} close",
                            dependency_identity_list(&body_refs)
                        )
                    })
            } else {
                Some(format!(
                    "eligible when {} close",
                    observations
                        .iter()
                        .map(|dependency| dependency.identity.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            };
            blockers.push(DependencyBlocker {
                ticket_path: issue.number.clone(),
                work_id: format!("#{}", issue.number),
                title: issue.title.clone(),
                reason_code: error.code.into(),
                reason: error.message,
                dependencies: observations,
                eligible_when: eligible_when
                    .unwrap_or_else(|| "eligible when all prerequisites are closed".into()),
            });
        }
    }
    blockers
}

pub(super) fn evaluate_issue_dependencies(
    profile: &Profile,
    issues: &[IssueDetails],
) -> Vec<DependencyBlocker> {
    evaluate_with(&profile.provider, issues, |target| {
        fetch_dependency_issue(profile, target).map_err(|error| format!("{error:#}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn issue(number: &str, state: Option<&str>, body: &str) -> IssueDetails {
        IssueDetails {
            number: number.into(),
            title: format!("Issue {number}"),
            body: body.into(),
            labels: vec![],
            state: state.map(str::to_string),
        }
    }

    fn dependency_issue(number: &str, state: Option<&str>, body: &str) -> DependencyIssue {
        DependencyIssue {
            number: number.into(),
            target: DependencyLookupTarget::same_project(number),
            body: body.into(),
            state: state.map(str::to_string),
            native_dependencies: vec![],
        }
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
        let blockers = evaluate_with("github", &roots, |target| {
            Ok(dependency_issue(
                target.number(),
                Some("OPEN"),
                "Blocked by: #2",
            ))
        });
        assert_eq!(blockers[0].reason_code, "dependency_cycle");
    }

    #[test]
    fn github_states_block_open_release_closed_and_cache_shared_references() {
        let roots = vec![
            issue("1", Some("OPEN"), "Blocked by: #9"),
            issue("2", Some("OPEN"), "Blocked by: #9"),
        ];
        let calls = RefCell::new(0);
        let blockers = evaluate_with("github", &roots, |target| {
            *calls.borrow_mut() += 1;
            match target.number() {
                "1" | "2" => Ok(dependency_issue(
                    target.number(),
                    Some("OPEN"),
                    "Blocked by: #9",
                )),
                "9" => Ok(dependency_issue("9", Some("OPEN"), "")),
                other => unreachable!("unexpected target {other}"),
            }
        });
        assert_eq!(blockers.len(), 2);
        assert_eq!(*calls.borrow(), 1, "one controller tick must share lookups");

        let released = evaluate_with("github", &roots, |target| match target.number() {
            "1" | "2" => Ok(dependency_issue(
                target.number(),
                Some("OPEN"),
                "Blocked by: #9",
            )),
            "9" => Ok(dependency_issue("9", Some("CLOSED"), "")),
            other => unreachable!("unexpected target {other}"),
        });
        assert!(released.is_empty());
    }

    #[test]
    fn gitlab_cycle_missing_unknown_and_provider_error_fail_closed() {
        let roots = vec![
            issue("1", Some("opened"), "Blocked by: #2"),
            issue("2", Some("opened"), "Blocked by: #1"),
        ];
        let cycle = evaluate_with("gitlab", &roots, |target| match target.number() {
            "1" => Ok(dependency_issue("1", Some("opened"), "Blocked by: #2")),
            "2" => Ok(dependency_issue("2", Some("opened"), "Blocked by: #1")),
            other => unreachable!("unexpected target {other}"),
        });
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
                |target| match target.number() {
                    "3" => Ok(dependency_issue("3", Some("opened"), "Blocked by: #99")),
                    _ => Err(error.into()),
                },
            );
            assert_eq!(blocked[0].reason_code, expected);
        }

        let unknown = evaluate_with(
            "gitlab",
            &[issue("4", Some("opened"), "Blocked by: #8")],
            |target| match target.number() {
                "4" => Ok(dependency_issue("4", Some("opened"), "Blocked by: #8")),
                "8" => Ok(dependency_issue("8", None, "")),
                other => unreachable!("unexpected target {other}"),
            },
        );
        assert_eq!(unknown[0].reason_code, "dependency_unknown_state");
    }
}
