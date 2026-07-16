use super::issues::{fetch_dependency_issue, DependencyIssue, IssueDetails};
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

    fn walk(
        &mut self,
        current: &str,
        references: &[String],
        stack: &mut Vec<String>,
        observations: &mut Vec<DependencyObservation>,
    ) -> Result<bool, DependencyFailure> {
        let mut has_open = false;
        for number in references {
            if number == current || stack.contains(number) {
                let mut cycle = stack.clone();
                cycle.push(number.clone());
                return Err(DependencyFailure {
                    code: "dependency_cycle",
                    message: format!(
                        "dependency cycle detected: {}",
                        cycle
                            .iter()
                            .map(|part| format!("#{part}"))
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
                    observations.push(DependencyObservation {
                        identity: format!("#{number}"),
                        provider: self.provider.to_string(),
                        provider_state: None,
                        normalized_state: label.into(),
                    });
                    return Err(DependencyFailure {
                        code,
                        message: format!("could not resolve dependency #{number}: {error}"),
                    });
                }
            };

            let normalized = normalize_state(issue.state.as_deref());
            if !observations
                .iter()
                .any(|seen| seen.identity == format!("#{number}"))
            {
                observations.push(DependencyObservation {
                    identity: format!("#{}", issue.number),
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
                        message: format!("dependency #{number} has unknown provider state"),
                    });
                }
                "open" => has_open = true,
                _ => unreachable!(),
            }

            let nested = parse_dependency_line(&issue.body)?;
            if let Some(nested) = nested {
                stack.push(number.clone());
                let nested_open = self.walk(number, &nested, stack, observations)?;
                stack.pop();
                has_open |= nested_open;
            }
        }
        Ok(has_open)
    }
}

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

pub(super) fn evaluate_issue_dependencies(
    profile: &Profile,
    issues: &[IssueDetails],
) -> Vec<DependencyBlocker> {
    evaluate_with(&profile.provider, issues, |number| {
        fetch_dependency_issue(profile, number).map_err(|error| format!("{error:#}"))
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
}
