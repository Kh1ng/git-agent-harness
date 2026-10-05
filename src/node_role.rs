//! Resolve the host role from persistent defaults and an explicit process override.
//! A worker's central URL is shared by status, claims, and remote memory; it is
//! never inferred from this machine's own network address.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Host role union. Contract compatibility policy (CONTRIBUTING.md "Cross-surface
/// changes"): role values are append-only, and every consumer fails closed on a
/// value it does not know -- an unknown role must never silently deserialize to
/// a default, because a misread role changes what this host is allowed to serve.
/// Consequences of adding a value (for example `Standalone` in #1318):
/// - An older binary that reads a newer role from the shared config TOML fails
///   deserialization with serde's unknown-variant error. That failure is the
///   designed boundary, not a regression: upgrade a host's `gah` before
///   persisting a newer `node_role` there (`gah update` upgrades the CLI and
///   the config reader together, so a host cannot outrun itself).
/// - `StatusSnapshot.node` is optional ("absent on older CLIs"), so consumers
///   already tolerate absence; a `role` they do not recognize must be rejected
///   with an upgrade instruction, not defaulted.
/// - The role is host-local configuration: it never crosses fleet wire formats
///   (registry observations carry no role), so mixed-version fleets are
///   unaffected by a new value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    #[default]
    Central,
    Standalone,
    Worker,
}

impl NodeRole {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "central" => Ok(Self::Central),
            "standalone" => Ok(Self::Standalone),
            "worker" => Ok(Self::Worker),
            other => {
                bail!("invalid node role '{other}' (expected 'central', 'standalone', or 'worker')")
            }
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

    #[test]
    fn standalone_resolves_locally_and_unknown_roles_fail_closed() {
        let defaults = crate::config::Defaults {
            node_role: NodeRole::Standalone,
            ..crate::config::Defaults::default()
        };
        // Standalone is loopback-only: it neither requires nor consumes a central origin.
        let standalone = NodeRoleStatus::with_override(&defaults, None).unwrap();
        assert_eq!(standalone.role, NodeRole::Standalone);
        assert_eq!(standalone.central_url, None);
        // An explicit override still wins over the persisted default.
        assert_eq!(
            NodeRoleStatus::with_override(&defaults, Some("central"))
                .unwrap()
                .role,
            NodeRole::Central
        );
        assert_eq!(NodeRole::parse("standalone").unwrap(), NodeRole::Standalone);
        // The persisted role round-trips through the shared config TOML.
        let saved = toml::to_string(&defaults).unwrap();
        let restored: crate::config::Defaults = toml::from_str(&saved).unwrap();
        assert_eq!(restored.node_role, NodeRole::Standalone);
        // Compatibility boundary: a value this binary does not know fails closed
        // instead of silently defaulting -- the same guarantee an older gah
        // applies when it reads `standalone` from a newer config.
        let unknown =
            toml::from_str::<crate::config::GahConfig>("[defaults]\nnode_role = \"edge\"\n");
        assert!(unknown.is_err());
    }
}
