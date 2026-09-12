use super::{CandidateConfig, Defaults, Profile, RoutingPolicy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Provider-neutral declaration of one concrete runner/account binding.
/// Map keys are stable backend-instance identifiers. Credentials are never
/// stored here; executable/state paths remain runtime-only identity inputs.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BackendInstanceConfig {
    #[serde(default)]
    pub runner_kind: String,
    /// Issue #822: a disabled instance stays fully declared but routing
    /// must skip it with a typed reason. Defaults to enabled so legacy
    /// config lines (written before this field existed) deserialize
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Resolve `runner_kind` on the service PATH when no explicit executable
    /// is bound. Opt-in keeps absent explicit bindings fail-closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve_from_path: Option<bool>,
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

impl BackendInstanceConfig {
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    pub fn resolves_from_path(&self) -> bool {
        self.resolve_from_path.unwrap_or(false)
    }
}

pub(crate) fn merge_instance_maps(
    mut canonical: HashMap<String, BackendInstanceConfig>,
    project: HashMap<String, BackendInstanceConfig>,
) -> HashMap<String, BackendInstanceConfig> {
    for (name, mut project) in project {
        if let Some(canonical) = canonical.remove(&name) {
            if project.runner_kind.is_empty() {
                project.runner_kind = canonical.runner_kind;
            }
            project.enabled = project.enabled.or(canonical.enabled);
            project.logical_backend = project.logical_backend.or(canonical.logical_backend);
            project.executable = project.executable.or(canonical.executable);
            project.resolve_from_path = project.resolve_from_path.or(canonical.resolve_from_path);
            project.state_root = project.state_root.or(canonical.state_root);
            project.account_label = project.account_label.or(canonical.account_label);
            project.auth_source_label = project.auth_source_label.or(canonical.auth_source_label);
            project.quota_pool = project.quota_pool.or(canonical.quota_pool);
            if project.supported_models.is_empty() {
                project.supported_models = canonical.supported_models;
            }
        }
        canonical.insert(name, project);
    }
    canonical
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
            identity.executable = match crate::runner::resolve_backend_instance_executable(instance)
            {
                crate::runner::ExecutableResolution::Found(path)
                | crate::runner::ExecutableResolution::MissingExplicitPath(path) => Some(path),
                crate::runner::ExecutableResolution::MissingFromPath(_)
                | crate::runner::ExecutableResolution::UnknownBackend(_) => Some(PathBuf::new()),
            };
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
            if let Err(error) = crate::execution_identity::validate_operator_label(field, value) {
                errors.push(format!("instance '{name}': {error:#}"));
            }
        }
    }
    // Note: BackendKind::parse also accepts "hermes" (not just the six
    // kinds this validation originally allowed) -- a deliberate, safe
    // widening, since Hermes is a real coding backend already dispatched
    // by manager chat even though it has no runner::backends module yet;
    // this never rejects anything that was previously valid.
    if crate::backend_kind::BackendKind::parse(instance.runner_kind.as_str()).is_err() {
        errors.push(format!(
            "instance '{name}': unsupported runner kind '{}'",
            instance.runner_kind
        ));
    }
    // Issue #822: a disabled instance must stay saveable even while its
    // executable is broken or removed -- taking a misbehaving backend out
    // of rotation without deleting its declaration is the point of the
    // toggle. Its identity fields are still validated above.
    if !instance.enabled() {
        return;
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
        None if instance.resolves_from_path() => {
            if !matches!(
                crate::runner::resolve_backend_instance_executable(instance),
                crate::runner::ExecutableResolution::Found(_)
            ) {
                errors.push(format!(
                    "instance '{name}': runner '{}' was not found on PATH",
                    instance.runner_kind
                ));
            }
        }
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
            effective.backend_instances["opencode-main"].runner_kind,
            "opencode"
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
                enabled: Some(true),
                resolve_from_path: Some(false),
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

/// `enabled` defaults to true: a hand-built instance (tests, programmatic
/// config) must never silently mean "disabled".
impl Default for BackendInstanceConfig {
    fn default() -> Self {
        BackendInstanceConfig {
            runner_kind: String::new(),
            enabled: None,
            logical_backend: None,
            executable: None,
            resolve_from_path: None,
            state_root: None,
            account_label: None,
            auth_source_label: None,
            quota_pool: None,
            supported_models: Vec::new(),
        }
    }
}

#[cfg(test)]
mod enabled_flag_tests {
    use super::*;

    /// Issue #822: legacy config lines written before `enabled` existed must
    /// deserialize as enabled (never as disabled).
    #[test]
    fn legacy_instance_line_without_enabled_deserializes_as_enabled() {
        let legacy: BackendInstanceConfig = toml::from_str(
            "runner_kind = \"codex\"\n\
             logical_backend = \"codex\"\n\
             executable = \"/bin/ls\"\n",
        )
        .unwrap();
        assert!(legacy.enabled(), "absence of the field must mean enabled");

        let explicit_false: BackendInstanceConfig = toml::from_str(
            "runner_kind = \"codex\"\n\
             logical_backend = \"codex\"\n\
             executable = \"/bin/ls\"\n\
             enabled = false\n",
        )
        .unwrap();
        assert!(!explicit_false.enabled());

        // Default-constructed instances (tests, programmatic config) are
        // enabled: disabled must never be an implicit default.
        assert!(BackendInstanceConfig::default().enabled());
    }

    /// Issue #822: a disabled instance must stay saveable even while its
    /// executable is missing -- taking a misbehaving backend out of rotation
    /// without deleting its declaration is the point of the toggle. Re-enabling
    /// it without a valid executable must fail validation.
    #[test]
    fn disabled_instance_survives_validation_with_missing_executable() {
        let mut profile = crate::ledger::test_util::profile();
        profile.routing.backend_instances.insert(
            "broken".into(),
            BackendInstanceConfig {
                runner_kind: "codex".into(),
                logical_backend: Some("codex".into()),
                executable: Some("/nonexistent/gah-test-wrapper".into()),
                enabled: Some(false),
                ..Default::default()
            },
        );
        let defaults = crate::config::Defaults::default();
        assert!(check_profile_backend_instances(&defaults, &profile).is_ok());

        if let Some(instance) = profile.routing.backend_instances.get_mut("broken") {
            instance.enabled = Some(true);
        }
        let errors = check_profile_backend_instances(&defaults, &profile)
            .expect_err("enabled instance with a missing executable must fail validation");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("missing or not executable")),
            "expected the executable check, got: {errors:?}"
        );
    }
}
