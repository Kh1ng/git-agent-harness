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
}
