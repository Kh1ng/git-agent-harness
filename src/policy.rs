use crate::models::{PolicyConfig, RepoPolicy};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Read a policy file and answer whether it permits one repository mutation.
/// CLI, dispatch, and reconciliation checks share this rule table so they
/// cannot disagree about the same action.
pub(crate) fn config_allows_action(config: &Path, action: &str) -> Result<bool> {
    let text = fs::read_to_string(config)
        .with_context(|| format!("reading policy file: {}", config.display()))?;
    let config: PolicyConfig = toml::from_str(&text)
        .with_context(|| format!("parsing policy file: {}", config.display()))?;
    Ok(repo_allows_action(&config.repo, action))
}

fn repo_allows_action(repo: &RepoPolicy, action: &str) -> bool {
    match repo.trust_mode.as_str() {
        "read_only" => false,
        "draft_pr_allowed" => match action {
            "open-draft-pr" => {
                repo.allow_provider_mutation && repo.allow_push && repo.allow_draft_pr
            }
            "edit-issue" => repo.allow_issue_write,
            "git-push" => repo.allow_push,
            "git-push-prod" => repo.allow_project_write,
            _ => false,
        },
        _ => false,
    }
}

pub fn run(config: &str, action: &str) -> Result<()> {
    if config_allows_action(Path::new(config), action)? {
        println!("allowed");
        Ok(())
    } else {
        println!("blocked");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RepoPolicy {
        RepoPolicy {
            trust_mode: "draft_pr_allowed".into(),
            allow_provider_mutation: true,
            allow_push: true,
            allow_draft_pr: true,
            allow_issue_write: false,
            allow_project_write: false,
        }
    }

    #[test]
    fn one_rule_table_covers_every_mutation_action() {
        let policy = policy();
        assert!(repo_allows_action(&policy, "open-draft-pr"));
        assert!(repo_allows_action(&policy, "git-push"));
        assert!(!repo_allows_action(&policy, "edit-issue"));
        assert!(!repo_allows_action(&policy, "git-push-prod"));
        assert!(!repo_allows_action(&policy, "unknown"));
    }
}
