use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

/// Canonical identity selected for one execution attempt.
///
/// This is the typed carrier approved by
/// `docs/EXECUTION_IDENTITY_CONTRACT.md`. During the compatibility phase,
/// account and instance fields are projected from the legacy backend and
/// quota-pool strings. Later migration steps replace those projections with
/// explicit configuration without changing routing policy or candidate order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIdentity {
    /// Agent CLI family GAH invokes. Multiple logical backends may share one
    /// runner kind (for example `agy`, `agy-main`, and `agy-second`).
    pub runner_kind: String,
    /// Resolved executable used for the attempt. Candidate construction does
    /// not resolve executables; the production routing boundary fills this in
    /// with the same resolver dispatch uses.
    #[serde(skip, default)]
    pub executable: Option<PathBuf>,
    /// Optional isolated HOME/state directory used only to launch this
    /// instance. Like the executable, it must never enter durable identity.
    #[serde(skip, default)]
    pub state_root: Option<PathBuf>,
    /// Whether `backend_instance` came from an explicit instance declaration
    /// rather than the legacy backend/quota compatibility projection.
    #[serde(default, skip_serializing_if = "is_false")]
    pub explicit_instance: bool,
    /// Backend requested before routing/fallback.
    pub requested_backend: String,
    /// Effective logical backend selected by routing.
    pub logical_backend: String,
    /// Stable route-instance identity. Until explicit instances land, this is
    /// the documented legacy backend/quota-pool projection.
    pub backend_instance: String,
    /// Secret-safe account label. Legacy configuration only has quota-pool
    /// metadata, so that is retained as the compatibility projection.
    pub account_label: Option<String>,
    /// Secret-safe auth-source label. This is genuinely unknown in legacy
    /// configuration and must not be reconstructed from HOME/path conventions.
    pub auth_source_label: Option<String>,
    /// Capacity/billing pool selected for the route.
    pub quota_pool: Option<String>,
    /// Model requested before routing/fallback.
    pub requested_model: Option<String>,
    /// Effective model selected for the attempt.
    pub effective_model: Option<String>,
}

impl ExecutionIdentity {
    /// Construct the compatibility identity for a configured candidate before
    /// it is attached to a particular route request.
    pub fn legacy_candidate(
        logical_backend: impl Into<String>,
        effective_model: Option<impl Into<String>>,
        quota_pool: Option<impl Into<String>>,
    ) -> Self {
        let logical_backend = logical_backend.into();
        let effective_model = effective_model.map(Into::into);
        let quota_pool = quota_pool.map(Into::into);
        Self::legacy_route(
            logical_backend.clone(),
            None::<String>,
            logical_backend,
            effective_model,
            quota_pool,
        )
    }

    /// Attach a selected legacy candidate to its original request.
    pub fn legacy_route(
        requested_backend: impl Into<String>,
        requested_model: Option<impl Into<String>>,
        logical_backend: impl Into<String>,
        effective_model: Option<impl Into<String>>,
        quota_pool: Option<impl Into<String>>,
    ) -> Self {
        let requested_backend = requested_backend.into();
        let requested_model = requested_model.map(Into::into);
        let logical_backend = logical_backend.into();
        let effective_model = effective_model.map(Into::into);
        let quota_pool = quota_pool.map(Into::into);
        let backend_instance = legacy_backend_instance(&logical_backend, quota_pool.as_deref());
        Self {
            runner_kind: runner_kind_for_backend(&logical_backend).to_string(),
            executable: None,
            state_root: None,
            explicit_instance: false,
            requested_backend,
            logical_backend,
            backend_instance,
            account_label: quota_pool.clone(),
            auth_source_label: None,
            quota_pool,
            requested_model,
            effective_model,
        }
    }

    pub fn with_request(
        &self,
        requested_backend: impl Into<String>,
        requested_model: Option<impl Into<String>>,
    ) -> Self {
        let mut identity = self.clone();
        identity.requested_backend = requested_backend.into();
        identity.requested_model = requested_model.map(Into::into);
        identity
    }

    pub fn with_effective_model(&self, effective_model: Option<impl Into<String>>) -> Self {
        let mut identity = self.clone();
        identity.effective_model = effective_model.map(Into::into);
        identity
    }

    /// Update the compatibility quota projection as one operation. Until
    /// explicit account configuration lands, the quota pool also supplies the
    /// secret-safe account label and contributes to the backend instance key.
    pub fn set_quota_pool(&mut self, quota_pool: Option<impl Into<String>>) {
        self.quota_pool = quota_pool.map(Into::into);
        self.backend_instance =
            legacy_backend_instance(&self.logical_backend, self.quota_pool.as_deref());
        self.account_label = self.quota_pool.clone();
    }

    pub fn set_executable(&mut self, executable: Option<PathBuf>) {
        self.executable = executable;
    }

    pub fn set_state_root(&mut self, state_root: Option<PathBuf>) {
        self.state_root = state_root;
    }

    /// Enforce the durable-label contract at the last boundary before an
    /// attempted route is recorded. Models are intentionally excluded: they
    /// are provider identifiers, while these fields are operator labels.
    pub fn validate_for_persistence(&self) -> anyhow::Result<()> {
        validate_secret_safe_label("runner kind", &self.runner_kind)?;
        validate_secret_safe_label("requested backend", &self.requested_backend)?;
        validate_secret_safe_label("logical backend", &self.logical_backend)?;
        validate_secret_safe_label("backend instance", &self.backend_instance)?;
        if let Some(label) = self.account_label.as_deref() {
            validate_secret_safe_label("account label", label)?;
        }
        if let Some(label) = self.auth_source_label.as_deref() {
            validate_secret_safe_label("auth source label", label)?;
        }
        if let Some(label) = self.quota_pool.as_deref() {
            validate_secret_safe_label("quota pool", label)?;
        }
        Ok(())
    }
}

/// Current compatibility projection. This exactly matches
/// `usage_attribution::normalize_attempt_usage`; keeping it here gives
/// routing and usage one implementation while explicit instances are added.
pub fn legacy_backend_instance(backend: &str, quota_pool: Option<&str>) -> String {
    match quota_pool {
        Some(pool)
            if crate::availability::agy_account(backend).is_some_and(|account| {
                pool.split_once(':')
                    .is_some_and(|(owner, _)| owner == account || owner == backend)
            }) =>
        {
            let (_, family) = pool.split_once(':').expect("qualified AGY pool");
            format!("{backend}:{family}")
        }
        Some(pool) if pool.split(':').next() == Some(backend) => pool.to_string(),
        Some(pool) => format!("{backend}:{pool}"),
        None => backend.to_string(),
    }
}

pub fn runner_kind_for_backend(backend: &str) -> &str {
    match backend {
        "cloud-coder" | "openhands" => "openhands",
        "agy" | "agy-main" | "agy-second" => "agy",
        other => other,
    }
}

/// Validate an operator-facing identity label before it can enter durable
/// state. Labels are logical names, never filesystem/auth material.
pub fn validate_secret_safe_label(field: &str, value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        anyhow::bail!("{field} must be a non-empty label of at most 128 bytes");
    }
    if trimmed.chars().any(char::is_whitespace)
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.chars().any(char::is_control)
    {
        anyhow::bail!("{field} must be a logical label, not a path or credential source");
    }
    if crate::redact::redact(trimmed) != trimmed {
        anyhow::bail!("{field} looks like credential material and cannot be persisted");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_projection_keeps_runner_instance_and_quota_scope_distinct() {
        let identity = ExecutionIdentity::legacy_route(
            "auto",
            Some("requested"),
            "agy-second",
            Some("effective"),
            Some("agy-second:anthropic"),
        );
        assert_eq!(identity.runner_kind, "agy");
        assert_eq!(identity.logical_backend, "agy-second");
        assert_eq!(identity.backend_instance, "agy-second:anthropic");
        assert_eq!(
            identity.account_label.as_deref(),
            Some("agy-second:anthropic")
        );
        assert_eq!(identity.auth_source_label, None);
        assert_eq!(identity.requested_model.as_deref(), Some("requested"));
        assert_eq!(identity.effective_model.as_deref(), Some("effective"));
    }

    #[test]
    fn distinct_backends_in_one_pool_remain_distinct() {
        let first =
            ExecutionIdentity::legacy_candidate("opencode", Some("glm"), Some("shared-pool"));
        let second =
            ExecutionIdentity::legacy_candidate("opencode-alt", Some("glm"), Some("shared-pool"));
        assert_ne!(first.backend_instance, second.backend_instance);
        assert_eq!(first.quota_pool, second.quota_pool);
    }

    #[test]
    fn changing_legacy_quota_refreshes_all_compatibility_projections() {
        let mut identity =
            ExecutionIdentity::legacy_candidate("agy", Some("gemini"), Some("agy:google"));

        identity.set_quota_pool(Some("agy:external"));

        assert_eq!(identity.quota_pool.as_deref(), Some("agy:external"));
        assert_eq!(identity.account_label.as_deref(), Some("agy:external"));
        assert_eq!(identity.backend_instance, "agy:external");
    }

    #[test]
    fn durable_labels_reject_paths_and_token_material() {
        assert!(validate_secret_safe_label("backend instance", "/home/user/codex").is_err());
        assert!(validate_secret_safe_label(
            "backend instance",
            "sk-abcdefghijklmnopqrstuvwxyz123456"
        )
        .is_err());
        assert_eq!(
            validate_secret_safe_label("backend instance", " codex-main ").unwrap(),
            "codex-main"
        );
    }

    #[test]
    fn durable_identity_never_serializes_executable_paths() {
        let mut identity =
            ExecutionIdentity::legacy_candidate("codex", Some("gpt-5"), Some("codex-main"));
        identity.set_executable(Some(PathBuf::from("/secret/home/bin/codex")));

        let json = serde_json::to_string(&identity).unwrap();

        assert!(!json.contains("executable"));
        assert!(!json.contains("/secret"));
        let restored: ExecutionIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.executable, None);
        assert_eq!(restored.backend_instance, "codex:codex-main");
    }
}
