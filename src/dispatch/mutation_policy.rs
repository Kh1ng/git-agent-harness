use crate::config::Profile;
use anyhow::Result;

/// Check profile policy before provisioning any worktree.
/// If a policy_path is set, the requested action must be allowed or dispatch
/// hard-fails before any mutations occur.
pub(in crate::dispatch) fn enforce_policy(profile: &Profile, action: &str) -> Result<()> {
    let Some(policy_path) = &profile.policy_path else {
        return Ok(()); // no policy file = trust the user
    };
    if crate::policy::config_allows_action(std::path::Path::new(policy_path), action)? {
        Ok(())
    } else {
        anyhow::bail!(
            "POLICY BLOCKED: action={action:?} is not allowed by {policy_path}. \
             Set the matching permission in that policy or pass --override-policy if you know what you're doing."
        )
    }
}
