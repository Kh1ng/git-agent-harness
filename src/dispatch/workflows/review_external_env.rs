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
    let declared_external_env_vars: HashSet<String> = profile
        .external_credential_scopes
        .values()
        .flat_map(|scope| scope.env_vars.iter().cloned())
        .collect();
    // A review without a resolvable source work item has no approval scope.
    // Preserve ordinary task configuration, but fail closed for every env var
    // declared as an external credential.
    let allowed_external_env_vars = work_id
        .and_then(|work_id| {
            ledger::read_entries(cfg).ok().map(|entries| {
                ledger::active_external_approval_env_vars_from_entries(
                    &entries,
                    profile_name,
                    &profile.repo_id,
                    work_id,
                )
            })
        })
        .unwrap_or_default();
    attempt_env_vars.retain(|(key, _)| {
        !declared_external_env_vars.contains(key) || allowed_external_env_vars.contains(key)
    });
    attempt_env_vars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ExternalCredentialScope;
    use crate::config::RoutingPolicy;
    use crate::dispatch::test_util::{gah_config_with_ledger, profile};

    #[test]
    fn missing_work_id_preserves_configuration_but_removes_external_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.external_credential_scopes.insert(
            "odds".to_string(),
            ExternalCredentialScope {
                env_vars: vec!["ODDS_API_KEY".to_string()],
            },
        );
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());

        let filtered = filter_attempt_env_vars(
            &cfg,
            &prof,
            "test",
            None,
            vec![
                ("PUBLIC_SETTING".to_string(), "keep".to_string()),
                ("ODDS_API_KEY".to_string(), "secret".to_string()),
            ],
        );

        assert_eq!(
            filtered,
            vec![("PUBLIC_SETTING".to_string(), "keep".to_string())]
        );
    }
}
