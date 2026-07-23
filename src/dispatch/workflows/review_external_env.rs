use crate::config::{GahConfig, Profile};
use crate::ledger;
use std::collections::HashSet;

pub(super) fn filter_attempt_env_vars(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    work_id: Option<&str>,
    mut attempt_env_vars: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let Some(work_id) = work_id else {
        return attempt_env_vars;
    };
    let allowed_external_env_vars = ledger::read_entries(cfg)
        .ok()
        .map(|entries| {
            ledger::active_external_approval_env_vars_from_entries(
                &entries,
                profile_name,
                &profile.repo_id,
                work_id,
            )
        })
        .unwrap_or_default();
    let declared_external_env_vars: HashSet<String> = profile
        .external_credential_scopes
        .values()
        .flat_map(|scope| scope.env_vars.iter().cloned())
        .collect();
    attempt_env_vars.retain(|(key, _)| {
        !declared_external_env_vars.contains(key) || allowed_external_env_vars.contains(key)
    });
    attempt_env_vars
}
