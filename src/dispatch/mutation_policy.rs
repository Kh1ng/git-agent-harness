use crate::config::Profile;
use anyhow::{Context, Result};

/// Check profile policy before provisioning any worktree.
/// If a policy_path is set, the requested action must be allowed or dispatch
/// hard-fails before any mutations occur.
pub(in crate::dispatch) fn enforce_policy(profile: &Profile, action: &str) -> Result<()> {
    let Some(policy_path) = &profile.policy_path else {
        return Ok(()); // no policy file = trust the user
    };
    let text = std::fs::read_to_string(policy_path)
        .with_context(|| format!("reading policy file: {}", policy_path))?;
    let cfg: crate::models::PolicyConfig =
        toml::from_str(&text).with_context(|| format!("parsing policy file: {}", policy_path))?;
    let repo = cfg.repo;
    let allowed = match repo.trust_mode.as_str() {
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
    };
    if allowed {
        Ok(())
    } else {
        anyhow::bail!(
            "POLICY BLOCKED: trust_mode={:?} does not allow action={:?}.              Set allow_push/allow_draft_pr/allow_project_write in {} or              pass --override-policy if you know what you're doing.",
            repo.trust_mode, action, policy_path
        )
    }
}
