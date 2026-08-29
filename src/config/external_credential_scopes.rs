use super::Profile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
pub struct ExternalCredentialScope {
    #[serde(default)]
    pub env_vars: Vec<String>,
}

impl Profile {
    pub fn external_credential_scope(&self, label: &str) -> Option<&ExternalCredentialScope> {
        self.external_credential_scopes.get(label)
    }

    pub fn effective_prune_older_than_days(&self) -> u64 {
        self.prune_older_than_days.unwrap_or(30)
    }

    pub fn effective_chat_session_idle_days(&self) -> u64 {
        self.chat_session_idle_days.unwrap_or(14)
    }
}

#[cfg(test)]
mod tests {
    use super::Profile;
    use crate::config::load;

    #[test]
    fn prune_older_than_days_defaults_to_30() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_config_path = tmp.path().join("gah-config.toml");
        std::fs::write(
            &repo_config_path,
            "[defaults]\nartifact_root = \"\"\nworktree_base = \"\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n[profiles.repo]\ndisplay_name = \"repo\"\nrepo_id = \"real\"\nrepo = \"real\"\nprovider = \"github\"\nlocal_path = \"/tmp\"\nartifact_root = \"/tmp\"\ndefault_target_branch = \"main\"\n",
        )
        .unwrap();
        let cfg = load(Some(repo_config_path.to_str().unwrap())).unwrap();
        let profile = cfg.profiles.get("repo").unwrap();
        assert_eq!(profile.prune_older_than_days, None);
        assert_eq!(profile.effective_prune_older_than_days(), 30);
    }

    #[test]
    fn prune_older_than_days_respects_profile_override() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_config_path = tmp.path().join("gah-config.toml");
        std::fs::write(
            &repo_config_path,
            "[defaults]\nartifact_root = \"\"\nworktree_base = \"\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n[profiles.repo]\ndisplay_name = \"repo\"\nrepo_id = \"real\"\nrepo = \"real\"\nprovider = \"github\"\nlocal_path = \"/tmp\"\nartifact_root = \"/tmp\"\ndefault_target_branch = \"main\"\nprune_older_than_days = 7\n",
        )
        .unwrap();
        let cfg = load(Some(repo_config_path.to_str().unwrap())).unwrap();
        let profile = cfg.profiles.get("repo").unwrap();
        assert_eq!(profile.prune_older_than_days, Some(7));
        assert_eq!(profile.effective_prune_older_than_days(), 7);
    }

    #[test]
    fn chat_session_idle_days_defaults_to_14_and_respects_profile_override() {
        let default_profile: Profile = toml::from_str(
            "display_name = \"repo\"\nrepo_id = \"real\"\nrepo = \"real\"\nprovider = \"github\"\nlocal_path = \"/tmp\"\nartifact_root = \"/tmp\"\ndefault_target_branch = \"main\"\n",
        )
        .unwrap();
        assert_eq!(default_profile.chat_session_idle_days, None);
        assert_eq!(default_profile.effective_chat_session_idle_days(), 14);

        let configured: Profile = toml::from_str(
            "display_name = \"repo\"\nrepo_id = \"real\"\nrepo = \"real\"\nprovider = \"github\"\nlocal_path = \"/tmp\"\nartifact_root = \"/tmp\"\ndefault_target_branch = \"main\"\nchat_session_idle_days = 7\n",
        )
        .unwrap();
        assert_eq!(configured.chat_session_idle_days, Some(7));
        assert_eq!(configured.effective_chat_session_idle_days(), 7);
    }
}
