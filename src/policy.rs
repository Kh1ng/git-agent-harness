use crate::models::PolicyConfig;
use anyhow::Result;
use std::fs;

pub fn run(config: &str, action: &str) -> Result<()> {
    let config: PolicyConfig = toml::from_str(&fs::read_to_string(config)?)?;
    let repo = config.repo;
    let allowed = match repo.trust_mode.as_str() {
        "read_only" => false,
        "draft_pr_allowed" => match action {
            "open-draft-pr" => {
                repo.allow_provider_mutation && repo.allow_push && repo.allow_draft_pr
            }
            "edit-issue" => repo.allow_issue_write,
            _ => false,
        },
        _ => false,
    };
    if allowed {
        println!("allowed");
        Ok(())
    } else {
        println!("blocked");
        std::process::exit(1);
    }
}
