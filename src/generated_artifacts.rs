use anyhow::{Context, Result};
use regex::Regex;
use std::path::Path;

pub const POLICY_SOURCE: &str = "profile.publishing.generated_artifact_deny_patterns";

pub fn default_deny_patterns() -> Vec<String> {
    [
        "**/node_modules/**",
        "**/.vite/**",
        "**/.vitest/**",
        "**/coverage/**",
        "**/.nyc_output/**",
        "**/.cache/**",
        "**/.next/**",
        "**/test-results/**",
        "**/playwright-report/**",
        "**/target/**",
        "**/__pycache__/**",
        "**/.pytest_cache/**",
        "**/*.tsbuildinfo",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeniedArtifact {
    pub path: String,
    pub pattern: String,
}

/// Inspect the complete index relative to the target branch. Callers stage
/// uncommitted changes first; backend-authored commits are already represented
/// by the index, so one comparison covers additions, force-adds, and rename
/// destinations without deleting or rewriting the worker's files.
pub fn denied_index_additions(
    worktree: &Path,
    target_branch: &str,
    patterns: &[String],
) -> Result<Vec<DeniedArtifact>> {
    if patterns.is_empty() {
        return Ok(Vec::new());
    }
    let compiled = patterns
        .iter()
        .map(|pattern| {
            glob_regex(pattern)
                .with_context(|| format!("invalid generated-artifact deny pattern '{pattern}'"))
                .map(|regex| (pattern, regex))
        })
        .collect::<Result<Vec<_>>>()?;
    let target = format!("origin/{target_branch}");
    let output = crate::worktree::git_raw(
        &[
            "diff",
            "--cached",
            "--name-status",
            "-z",
            "--diff-filter=AR",
            &target,
        ],
        worktree,
    )?;
    let fields = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let mut index = 0;
    let mut denied = Vec::new();
    while index < fields.len() {
        let status = String::from_utf8_lossy(fields[index]);
        index += 1;
        let path = if status.starts_with('R') {
            // Rename records contain source then destination. Only the new
            // tracked destination is subject to this publication guard.
            index += 1;
            let Some(destination) = fields.get(index) else {
                anyhow::bail!("malformed git rename record while checking generated artifacts");
            };
            index += 1;
            destination
        } else {
            let Some(path) = fields.get(index) else {
                anyhow::bail!("malformed git add record while checking generated artifacts");
            };
            index += 1;
            path
        };
        let path = String::from_utf8_lossy(path).replace('\\', "/");
        if let Some((pattern, _)) = compiled.iter().find(|(_, regex)| regex.is_match(&path)) {
            denied.push(DeniedArtifact {
                path,
                pattern: (*pattern).clone(),
            });
        }
    }
    Ok(denied)
}

pub fn enforce_index_policy(
    worktree: &Path,
    target_branch: &str,
    patterns: &[String],
) -> Result<()> {
    let denied = denied_index_additions(worktree, target_branch, patterns)?;
    if denied.is_empty() {
        return Ok(());
    }
    let details = denied
        .iter()
        .map(|entry| format!("'{}' (pattern '{}')", entry.path, entry.pattern))
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::bail!(
        "generated_artifact_policy: refusing to publish newly tracked generated artifact(s): {details}; policy source: {POLICY_SOURCE}"
    )
}

pub fn validate_patterns(patterns: &[String]) -> Result<()> {
    for pattern in patterns {
        glob_regex(pattern)
            .with_context(|| format!("invalid generated-artifact deny pattern '{pattern}'"))?;
    }
    Ok(())
}

fn glob_regex(pattern: &str) -> Result<Regex> {
    let normalized = pattern.trim().replace('\\', "/");
    if normalized.is_empty() {
        anyhow::bail!("pattern is empty");
    }
    let chars = normalized.chars().collect::<Vec<_>>();
    let mut expression = String::from("^");
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '*' if chars.get(index + 1) == Some(&'*') => {
                index += 2;
                if chars.get(index) == Some(&'/') {
                    expression.push_str("(?:.*/)?");
                    index += 1;
                } else {
                    expression.push_str(".*");
                }
            }
            '*' => {
                expression.push_str("[^/]*");
                index += 1;
            }
            '?' => {
                expression.push_str("[^/]");
                index += 1;
            }
            character => {
                expression.push_str(&regex::escape(&character.to_string()));
                index += 1;
            }
        }
    }
    expression.push('$');
    Regex::new(&expression).context("compiling generated-artifact glob")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        crate::worktree::git(&["init", "-q"], temp.path()).unwrap();
        crate::worktree::git(
            &["config", "user.email", "gah@example.invalid"],
            temp.path(),
        )
        .unwrap();
        crate::worktree::git(&["config", "user.name", "GAH Test"], temp.path()).unwrap();
        fs::write(temp.path().join("README.md"), "base\n").unwrap();
        crate::worktree::git(&["add", "."], temp.path()).unwrap();
        crate::worktree::git(&["commit", "-q", "-m", "base"], temp.path()).unwrap();
        crate::worktree::git(&["branch", "-M", "main"], temp.path()).unwrap();
        crate::worktree::git(&["remote", "add", "origin", "."], temp.path()).unwrap();
        crate::worktree::git(
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
            temp.path(),
        )
        .unwrap();
        temp
    }

    #[test]
    fn default_policy_blocks_nested_vite_cache_even_when_force_added() {
        let temp = repo();
        let path = temp
            .path()
            .join("apps/server/node_modules/.vite/vitest/results.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{}\n").unwrap();
        crate::worktree::git(
            &[
                "add",
                "-f",
                "apps/server/node_modules/.vite/vitest/results.json",
            ],
            temp.path(),
        )
        .unwrap();

        let denied = denied_index_additions(temp.path(), "main", &default_deny_patterns()).unwrap();
        assert_eq!(denied.len(), 1);
        assert_eq!(
            denied[0].path,
            "apps/server/node_modules/.vite/vitest/results.json"
        );
        assert_eq!(denied[0].pattern, "**/node_modules/**");
    }

    #[test]
    fn similar_fixture_name_is_not_a_generated_path_segment() {
        let temp = repo();
        let path = temp.path().join("tests/fixtures/node_modules-sample.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{}\n").unwrap();
        crate::worktree::git(&["add", "."], temp.path()).unwrap();

        assert!(
            denied_index_additions(temp.path(), "main", &default_deny_patterns())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rename_destination_is_checked() {
        let temp = repo();
        fs::write(temp.path().join("tracked.json"), "{}\n").unwrap();
        crate::worktree::git(&["add", "."], temp.path()).unwrap();
        crate::worktree::git(&["commit", "-q", "-m", "fixture"], temp.path()).unwrap();
        crate::worktree::git(
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
            temp.path(),
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("coverage")).unwrap();
        crate::worktree::git(
            &["mv", "tracked.json", "coverage/tracked.json"],
            temp.path(),
        )
        .unwrap();

        let denied = denied_index_additions(temp.path(), "main", &default_deny_patterns()).unwrap();
        assert_eq!(denied[0].path, "coverage/tracked.json");
    }

    #[test]
    fn explicit_empty_policy_disables_the_guard() {
        let temp = repo();
        fs::create_dir_all(temp.path().join("node_modules")).unwrap();
        fs::write(temp.path().join("node_modules/result.json"), "{}\n").unwrap();
        crate::worktree::git(&["add", "-f", "node_modules/result.json"], temp.path()).unwrap();

        assert!(denied_index_additions(temp.path(), "main", &[])
            .unwrap()
            .is_empty());
    }
}
