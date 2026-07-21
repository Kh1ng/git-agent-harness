use super::{CandidateConfig, Defaults, Profile, RoutingPolicy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Provider-neutral declaration of one concrete runner/account binding.
/// Map keys are stable backend-instance identifiers. Credentials are never
/// stored here; executable/state paths remain runtime-only identity inputs.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct BackendInstanceConfig {
    pub runner_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_source_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    /// Empty preserves unrestricted legacy model behavior.
    #[serde(default)]
    pub supported_models: Vec<String>,
}

impl Profile {
    /// Whether a backend has an explicit profile setup marker. Readiness and
    /// executable resolution remain separate facts.
    pub fn is_backend_configured(&self, backend: &str) -> bool {
        if self.routing.backend_instances.values().any(|instance| {
            instance
                .logical_backend
                .as_deref()
                .unwrap_or(instance.runner_kind.as_str())
                == backend
                && instance.executable.is_some()
        }) {
            return true;
        }
        if backend == "openhands" {
            return self.oh_profile.is_some();
        }
        self.configured_backend_path(backend).is_some()
    }

    pub fn is_backend_configured_with_defaults(&self, defaults: &Defaults, backend: &str) -> bool {
        self.effective_routing(defaults)
            .backend_instances
            .values()
            .any(|instance| {
                instance
                    .logical_backend
                    .as_deref()
                    .unwrap_or(instance.runner_kind.as_str())
                    == backend
                    && instance.executable.is_some()
            })
            || self.is_backend_configured(backend)
    }
}

impl RoutingPolicy {
    /// Resolve one routing candidate through the effective global/profile
    /// registry. Explicit instance references are authoritative; an absent or
    /// incomplete declaration carries a fail-closed runtime sentinel.
    pub fn execution_identity_for_candidate(
        &self,
        candidate: &CandidateConfig,
    ) -> crate::execution_identity::ExecutionIdentity {
        let configured_pool = candidate.quota_pool.clone().or_else(|| {
            candidate
                .instance
                .as_ref()
                .and_then(|name| self.backend_instances.get(name))
                .and_then(|instance| instance.quota_pool.clone())
        });
        let quota_pool = crate::availability::resolve_candidate_quota_pool(
            &candidate.backend,
            candidate.model.as_deref(),
            configured_pool.as_deref(),
        );
        let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            candidate.backend.clone(),
            candidate.model.clone(),
            quota_pool,
        );
        let Some(instance_name) = candidate.instance.as_deref() else {
            return identity;
        };

        identity.backend_instance = instance_name.to_string();
        identity.explicit_instance = true;
        identity.runner_kind = "unknown_instance".to_string();
        identity.executable = Some(PathBuf::new());
        if let Some(instance) = self.backend_instances.get(instance_name) {
            identity.runner_kind = instance.runner_kind.clone();
            identity.logical_backend = instance
                .logical_backend
                .clone()
                .unwrap_or_else(|| candidate.backend.clone());
            identity.executable = Some(
                instance
                    .executable
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_default(),
            );
            identity.state_root = instance.state_root.as_deref().map(PathBuf::from);
            identity.account_label = instance.account_label.clone();
            identity.auth_source_label = instance.auth_source_label.clone();
            identity.quota_pool = candidate
                .quota_pool
                .clone()
                .or_else(|| instance.quota_pool.clone())
                .or(identity.quota_pool);
        }
        identity
    }
}

pub fn check_profile_backend_instances(
    defaults: &Defaults,
    profile: &Profile,
) -> Result<(), Vec<String>> {
    let routing = profile.effective_routing(defaults);
    let mut errors = Vec::new();
    let mut state_roots: HashMap<String, String> = HashMap::new();

    for (name, instance) in &routing.backend_instances {
        validate_instance(name, instance, &mut state_roots, &mut errors);
    }
    for candidate in all_candidates(&routing) {
        validate_candidate(&routing, candidate, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_instance(
    name: &str,
    instance: &BackendInstanceConfig,
    state_roots: &mut HashMap<String, String>,
    errors: &mut Vec<String>,
) {
    for (field, value) in [
        ("backend instance", Some(name)),
        ("runner kind", Some(instance.runner_kind.as_str())),
        ("account label", instance.account_label.as_deref()),
        ("auth source label", instance.auth_source_label.as_deref()),
        ("quota pool", instance.quota_pool.as_deref()),
    ] {
        if let Some(value) = value {
            if let Err(error) = crate::execution_identity::validate_secret_safe_label(field, value)
            {
                errors.push(format!("instance '{name}': {error:#}"));
            }
        }
    }
    if !matches!(
        instance.runner_kind.as_str(),
        "agy" | "claude" | "codex" | "openhands" | "opencode" | "vibe"
    ) {
        errors.push(format!(
            "instance '{name}': unsupported runner kind '{}'",
            instance.runner_kind
        ));
    }
    match instance
        .executable
        .as_deref()
        .filter(|path| !path.trim().is_empty())
    {
        Some(path) if crate::runner::is_executable_path(Path::new(path)) => {}
        Some(path) => errors.push(format!(
            "instance '{name}': executable binding '{path}' is missing or not executable"
        )),
        None => errors.push(format!(
            "instance '{name}': missing explicit executable binding"
        )),
    }
    if let Some(root) = instance
        .state_root
        .as_deref()
        .filter(|root| !root.trim().is_empty())
    {
        let normalized = PathBuf::from(root).to_string_lossy().into_owned();
        if let Some(other) = state_roots.insert(normalized.clone(), name.to_string()) {
            errors.push(format!(
                "instances '{other}' and '{name}' share state_root '{normalized}'; declare isolated state roots"
            ));
        }
    }
}

fn all_candidates(routing: &RoutingPolicy) -> impl Iterator<Item = &CandidateConfig> {
    routing
        .pm_candidates
        .iter()
        .flatten()
        .chain(routing.improve_candidates.iter().flatten())
        .chain(routing.review_candidates.iter().flatten())
        .chain(routing.routine_reviewer.iter())
        .chain(routing.escalatory_reviewers.iter())
        .chain(
            routing
                .task_routing_rules
                .iter()
                .flat_map(|rule| rule.candidates.iter()),
        )
}

fn validate_candidate(
    routing: &RoutingPolicy,
    candidate: &CandidateConfig,
    errors: &mut Vec<String>,
) {
    let Some(instance_name) = candidate.instance.as_deref() else {
        return;
    };
    let Some(instance) = routing.backend_instances.get(instance_name) else {
        errors.push(format!(
            "candidate {}/{} references unknown instance '{}'",
            candidate.backend,
            candidate.model.as_deref().unwrap_or("<default>"),
            instance_name
        ));
        return;
    };
    if instance
        .logical_backend
        .as_deref()
        .is_some_and(|logical| logical != candidate.backend)
    {
        errors.push(format!(
            "candidate backend '{}' disagrees with instance '{}' logical_backend '{}'",
            candidate.backend,
            instance_name,
            instance.logical_backend.as_deref().unwrap_or_default()
        ));
    }
    if !instance.supported_models.is_empty()
        && !candidate.model.as_deref().is_some_and(|model| {
            instance
                .supported_models
                .iter()
                .any(|supported| supported == model)
        })
    {
        errors.push(format!(
            "candidate {}/{} is not in instance '{}' supported_models",
            candidate.backend,
            candidate.model.as_deref().unwrap_or("<default>"),
            instance_name
        ));
    }
    validate_cost(candidate, instance_name, instance, errors);
}

fn validate_cost(
    candidate: &CandidateConfig,
    instance_name: &str,
    instance: &BackendInstanceConfig,
    errors: &mut Vec<String>,
) {
    let label = format!(
        "{}/{}",
        candidate.backend,
        candidate.model.as_deref().unwrap_or("<default>")
    );
    let is_local = candidate
        .model
        .as_deref()
        .is_some_and(|model| model.contains("ollama") || model.contains("local/"));
    if candidate.included_in_quota && candidate.marginal_cost_usd.is_some() {
        errors.push(format!(
            "candidate {label} cannot be included_in_quota and declare marginal_cost_usd"
        ));
    }
    if candidate.included_in_quota && candidate.requires_approval {
        errors.push(format!(
            "candidate {label} cannot require paid-route approval while included_in_quota"
        ));
    }
    if is_local
        && (candidate.included_in_quota
            || candidate.marginal_cost_usd.is_some()
            || candidate.requires_approval)
    {
        errors.push(format!(
            "local candidate {label} must be unmetered, outside quota, and approval-free"
        ));
    }
    if (candidate.marginal_cost_usd.is_some() || candidate.requires_approval)
        && instance.auth_source_label.is_none()
    {
        errors.push(format!(
            "paid candidate {label} requires instance '{instance_name}' auth_source_label"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::test_profile_for_notifications;

    #[test]
    fn profile_registry_overrides_global_by_instance_key() {
        let mut profile = test_profile_for_notifications();
        let mut defaults = Defaults::default();
        for (name, executable) in [
            ("opencode-main", "/global/opencode"),
            ("claude-main", "/global/claude"),
        ] {
            defaults.routing.backend_instances.insert(
                name.into(),
                BackendInstanceConfig {
                    runner_kind: name.split('-').next().unwrap().into(),
                    executable: Some(executable.into()),
                    ..Default::default()
                },
            );
        }
        profile.routing.backend_instances.insert(
            "opencode-main".into(),
            BackendInstanceConfig {
                runner_kind: "opencode".into(),
                executable: Some("/project/opencode-wrapper".into()),
                ..Default::default()
            },
        );

        let effective = profile.effective_routing(&defaults);

        assert_eq!(effective.backend_instances.len(), 2);
        assert_eq!(
            effective.backend_instances["opencode-main"]
                .executable
                .as_deref(),
            Some("/project/opencode-wrapper")
        );
        assert_eq!(
            effective.backend_instances["claude-main"]
                .executable
                .as_deref(),
            Some("/global/claude")
        );
    }

    #[test]
    fn declaration_resolves_safe_and_runtime_identity_fields() {
        let mut routing = RoutingPolicy::default();
        routing.backend_instances.insert(
            "opencode-api".into(),
            BackendInstanceConfig {
                runner_kind: "opencode".into(),
                logical_backend: Some("opencode".into()),
                executable: Some("/opt/wrappers/opencode-api".into()),
                state_root: Some("/var/lib/gah/opencode-api".into()),
                account_label: Some("team-api".into()),
                auth_source_label: Some("env-openai-key".into()),
                quota_pool: Some("openai-api".into()),
                supported_models: vec!["openai/gpt-5".into()],
            },
        );
        let identity = routing.execution_identity_for_candidate(&CandidateConfig {
            backend: "opencode".into(),
            instance: Some("opencode-api".into()),
            model: Some("openai/gpt-5".into()),
            ..Default::default()
        });

        assert_eq!(identity.backend_instance, "opencode-api");
        assert_eq!(identity.account_label.as_deref(), Some("team-api"));
        assert_eq!(
            identity.auth_source_label.as_deref(),
            Some("env-openai-key")
        );
        assert_eq!(identity.quota_pool.as_deref(), Some("openai-api"));
        assert_eq!(
            identity.executable.as_deref(),
            Some(Path::new("/opt/wrappers/opencode-api"))
        );
        assert_eq!(
            identity.state_root.as_deref(),
            Some(Path::new("/var/lib/gah/opencode-api"))
        );
    }

    #[test]
    fn doctor_rejects_state_collision_and_invalid_paid_route() {
        let mut profile = test_profile_for_notifications();
        for name in ["opencode-subscription", "opencode-api"] {
            profile.routing.backend_instances.insert(
                name.into(),
                BackendInstanceConfig {
                    runner_kind: "opencode".into(),
                    executable: Some("/bin/sh".into()),
                    state_root: Some("/tmp/shared-opencode-home".into()),
                    ..Default::default()
                },
            );
        }
        profile.routing.improve_candidates = Some(vec![CandidateConfig {
            backend: "opencode".into(),
            instance: Some("opencode-api".into()),
            model: Some("openai/gpt-5".into()),
            marginal_cost_usd: Some(1.0),
            ..Default::default()
        }]);

        let errors = check_profile_backend_instances(&Defaults::default(), &profile)
            .unwrap_err()
            .join("\n");
        assert!(errors.contains("share state_root"));
        assert!(errors.contains("requires instance 'opencode-api' auth_source_label"));
    }
}
