//! Resolve the host role from persistent defaults and an explicit process override.
//! A worker's central URL is shared by status, claims, and remote memory; it is
//! never inferred from this machine's own network address.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    #[default]
    Central,
    Worker,
}

impl NodeRole {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "central" => Ok(Self::Central),
            "worker" => Ok(Self::Worker),
            other => bail!("invalid node role '{other}' (expected 'central' or 'worker')"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct NodeRoleStatus {
    pub role: NodeRole,
    pub central_url: Option<String>,
}

impl NodeRoleStatus {
    pub fn resolve(defaults: &crate::config::Defaults) -> Result<Self> {
        Self::with_override(defaults, std::env::var("GAH_NODE_ROLE").ok().as_deref())
    }

    pub fn with_override(
        defaults: &crate::config::Defaults,
        role_override: Option<&str>,
    ) -> Result<Self> {
        let role = role_override
            .map(NodeRole::parse)
            .transpose()?
            .unwrap_or(defaults.node_role);
        let central_url = defaults.registry_central_url.clone();
        if role == NodeRole::Worker && central_url.is_none() {
            bail!("worker role requires [defaults].registry_central_url; use gah config set --node-role worker --registry-central-url URL");
        }
        if let Some(value) = central_url.as_deref() {
            let url = url::Url::parse(value)?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/"
            {
                bail!("registry_central_url must be an HTTP(S) origin without credentials, query, or path");
            }
        }
        Ok(Self { role, central_url })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_roles_require_a_central_origin_and_respect_explicit_override() {
        let mut defaults = crate::config::Defaults::default();
        assert_eq!(
            NodeRoleStatus::with_override(&defaults, None).unwrap().role,
            NodeRole::Central
        );
        defaults.registry_central_url = Some("https://user:secret@central.test".into());
        assert!(NodeRoleStatus::with_override(&defaults, None).is_err());
        defaults.registry_central_url = None;
        defaults.node_role = NodeRole::Worker;
        assert!(NodeRoleStatus::with_override(&defaults, None).is_err());
        for value in [
            "file:///tmp",
            "https://user:secret@central.test",
            "https://central.test/path",
        ] {
            defaults.registry_central_url = Some(value.into());
            assert!(NodeRoleStatus::with_override(&defaults, None).is_err());
        }
        defaults.registry_central_url = Some("https://central.test:3773".into());
        let worker = NodeRoleStatus::with_override(&defaults, None).unwrap();
        assert_eq!(worker.role, NodeRole::Worker);
        assert_eq!(worker.central_url, defaults.registry_central_url);
        assert_eq!(
            NodeRoleStatus::with_override(&defaults, Some("central"))
                .unwrap()
                .role,
            NodeRole::Central
        );
        assert!(NodeRoleStatus::with_override(&defaults, Some("typo")).is_err());
        let saved = toml::to_string(&defaults).unwrap();
        let restored: crate::config::Defaults = toml::from_str(&saved).unwrap();
        assert_eq!(restored.node_role, NodeRole::Worker);
        assert_eq!(restored.registry_central_url, defaults.registry_central_url);
    }
}
