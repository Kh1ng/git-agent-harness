use crate::{
    config,
    context::{ContextConfig, ContextOverride},
};
use anyhow::Result;
use std::{collections::BTreeMap, path::Path};

pub const CONFIG_SHOW_SCHEMA_VERSION: u32 = 3;

#[derive(serde::Serialize)]
pub struct RoutingCandidateSummary {
    pub backend: String,
    pub instance: Option<String>,
    pub model: Option<String>,
    pub quota_pool: Option<String>,
    pub priority: i32,
    pub included_in_quota: bool,
    pub marginal_cost_usd: Option<f64>,
    pub quota_usage_percent: Option<f64>,
    pub quota_days_remaining: Option<f64>,
    pub requires_approval: bool,
}

#[derive(serde::Serialize, Clone)]
pub struct BackendInstanceSummary {
    pub backend_instance: String,
    pub runner_kind: String,
    pub logical_backend: String,
    pub account_label: Option<String>,
    pub auth_source_label: Option<String>,
    pub quota_pool: Option<String>,
    pub supported_models: Vec<String>,
    pub executable_configured: bool,
    pub isolated_state_configured: bool,
}

pub(crate) fn backend_instance_summaries(
    routing: &config::RoutingPolicy,
) -> Vec<BackendInstanceSummary> {
    let mut summaries = routing
        .backend_instances
        .iter()
        .map(|(name, instance)| BackendInstanceSummary {
            backend_instance: name.clone(),
            runner_kind: instance.runner_kind.clone(),
            logical_backend: instance
                .logical_backend
                .clone()
                .unwrap_or_else(|| instance.runner_kind.clone()),
            account_label: instance.account_label.clone(),
            auth_source_label: instance.auth_source_label.clone(),
            quota_pool: instance.quota_pool.clone(),
            supported_models: instance.supported_models.clone(),
            executable_configured: instance.executable.is_some(),
            isolated_state_configured: instance.state_root.is_some(),
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| left.backend_instance.cmp(&right.backend_instance));
    summaries
}

#[derive(serde::Serialize)]
pub struct ContextBudgetSummary {
    pub enabled: bool,
    pub soft_limit_tokens: u64,
    pub hard_limit_tokens: u64,
    pub compact_after_tool_calls: u64,
    pub fresh_context_on_review: bool,
    pub fresh_context_on_fix: bool,
    pub include_full_git_history: bool,
    pub include_full_worker_transcript_in_review: bool,
    pub recent_history_tokens: u64,
}

#[derive(serde::Serialize)]
pub struct ContextOverrideBudgetSummary {
    pub enabled: Option<bool>,
    pub soft_limit_tokens: Option<u64>,
    pub hard_limit_tokens: Option<u64>,
    pub compact_after_tool_calls: Option<u64>,
    pub fresh_context_on_review: Option<bool>,
    pub fresh_context_on_fix: Option<bool>,
    pub include_full_git_history: Option<bool>,
    pub include_full_worker_transcript_in_review: Option<bool>,
    pub recent_history_tokens: Option<u64>,
}

#[derive(serde::Serialize)]
pub struct ConfigBackendContextSummary {
    pub backend: String,
    pub effective: ContextBudgetSummary,
    pub backend_override: Option<ContextOverrideBudgetSummary>,
}

#[derive(serde::Serialize)]
pub struct ConfigProfileContextSummary {
    pub global: ContextBudgetSummary,
    pub profile_override: Option<ContextOverrideBudgetSummary>,
    /// Effective context budget for every backend the profile actually
    /// routes to (pm/improve/review candidates, routine reviewer, and
    /// escalatory reviewers), since `context.backends.<name>` overrides are
    /// merged in per-backend and can diverge from one routed backend to the
    /// next. This is what dispatch actually applies -- see
    /// `dispatch::prompts::enforce_context_budget`.
    pub effective_by_backend: Vec<ConfigBackendContextSummary>,
}

#[derive(serde::Serialize)]
pub struct TaskRoutingRuleSummary {
    pub modes: Vec<String>,
    pub task_classes: Vec<String>,
    pub difficulties: Vec<String>,
    pub risks: Vec<String>,
    pub candidates: Vec<RoutingCandidateSummary>,
}

#[derive(serde::Serialize)]
pub struct NotificationSummary {
    pub configured: bool,
    pub transport: Option<String>,
    pub manager_wake_autonomy: String,
    pub env_file: Option<String>,
    pub env_file_prod: Option<String>,
}

#[derive(serde::Serialize)]
pub struct PmOrchestrationSummary {
    pub decomposition_labels: Vec<String>,
    pub max_children: u32,
    pub max_depth: u32,
    pub max_attempts: u32,
    pub timeout_seconds: u64,
    pub difficulty_labels: BTreeMap<String, String>,
    pub risk_labels: BTreeMap<String, String>,
    pub execution_labels: BTreeMap<String, String>,
}

#[derive(serde::Serialize)]
pub struct ConfigProfileSummary {
    pub profile: String,
    pub delivery_mode: String,
    pub merge_policy: String,
    pub max_fix_attempts_per_mr: u32,
    pub max_implementation_failures_per_ticket: u32,
    pub max_review_cycles_per_ticket: u32,
    pub max_paid_reviews_per_ticket: u32,
    pub backend_instances: Vec<BackendInstanceSummary>,
    pub pm_candidates: Vec<RoutingCandidateSummary>,
    pub improve_candidates: Vec<RoutingCandidateSummary>,
    pub review_candidates: Vec<RoutingCandidateSummary>,
    pub task_routing_rules: Vec<TaskRoutingRuleSummary>,
    pub routine_reviewer: Option<RoutingCandidateSummary>,
    pub escalatory_reviewers: Vec<RoutingCandidateSummary>,
    pub context: ConfigProfileContextSummary,
    pub notifications: NotificationSummary,
    pub pm_orchestration: PmOrchestrationSummary,
}

#[derive(serde::Serialize)]
pub struct ConfigShowSummary {
    pub current_manager: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ConfigShowFull {
    pub schema_version: u32,
    pub config_path: String,
    pub current_manager: Option<String>,
    pub profiles: BTreeMap<String, ConfigProfileSummary>,
}

fn to_summary(candidate: &config::CandidateConfig) -> RoutingCandidateSummary {
    RoutingCandidateSummary {
        backend: candidate.backend.clone(),
        instance: candidate.instance.clone(),
        model: candidate.model.clone(),
        quota_pool: candidate.quota_pool.clone(),
        priority: candidate.priority,
        included_in_quota: candidate.included_in_quota,
        marginal_cost_usd: candidate.marginal_cost_usd,
        quota_usage_percent: candidate.quota_usage_percent,
        quota_days_remaining: candidate.quota_days_remaining,
        requires_approval: candidate.requires_approval,
    }
}

fn to_context(context: &ContextConfig) -> ContextBudgetSummary {
    ContextBudgetSummary {
        enabled: context.enabled,
        soft_limit_tokens: context.soft_limit_tokens,
        hard_limit_tokens: context.hard_limit_tokens,
        compact_after_tool_calls: context.compact_after_tool_calls,
        fresh_context_on_review: context.fresh_context_on_review,
        fresh_context_on_fix: context.fresh_context_on_fix,
        include_full_git_history: context.include_full_git_history,
        include_full_worker_transcript_in_review: context.include_full_worker_transcript_in_review,
        recent_history_tokens: context.recent_history_tokens,
    }
}

fn to_context_override(override_cfg: &ContextOverride) -> ContextOverrideBudgetSummary {
    ContextOverrideBudgetSummary {
        enabled: override_cfg.enabled,
        soft_limit_tokens: override_cfg.soft_limit_tokens,
        hard_limit_tokens: override_cfg.hard_limit_tokens,
        compact_after_tool_calls: override_cfg.compact_after_tool_calls,
        fresh_context_on_review: override_cfg.fresh_context_on_review,
        fresh_context_on_fix: override_cfg.fresh_context_on_fix,
        include_full_git_history: override_cfg.include_full_git_history,
        include_full_worker_transcript_in_review: override_cfg
            .include_full_worker_transcript_in_review,
        recent_history_tokens: override_cfg.recent_history_tokens,
    }
}

fn notification_transport(command: Option<&str>) -> Option<String> {
    command.map(|command| {
        let command = command.to_ascii_lowercase();
        if command.contains("telegram") {
            "telegram".to_string()
        } else {
            "custom_command".to_string()
        }
    })
}

fn wake_autonomy(value: config::WakeAutonomy) -> String {
    match value {
        config::WakeAutonomy::Off => "off",
        config::WakeAutonomy::ReviewOnly => "review_only",
        config::WakeAutonomy::Full => "full",
    }
    .to_string()
}

fn build_profile_summary(
    cfg: &config::GahConfig,
    profile_name: &str,
) -> Result<ConfigProfileSummary> {
    let profile = config::get_profile(cfg, profile_name)?;
    let routing = profile.effective_routing(&cfg.defaults);
    let backend_instances = backend_instance_summaries(&routing);
    let routine_reviewer = routing.effective_routine_reviewer();
    let escalatory_reviewers = routing.effective_escalatory_reviewers();

    let pm_candidates = routing
        .pm_candidates
        .as_ref()
        .map(|candidates| candidates.iter().map(to_summary).collect())
        .unwrap_or_default();
    let improve_candidates = routing
        .improve_candidates
        .as_ref()
        .map(|candidates| candidates.iter().map(to_summary).collect())
        .unwrap_or_default();
    let review_candidates = routing
        .review_candidates
        .as_ref()
        .map(|candidates| candidates.iter().map(to_summary).collect())
        .unwrap_or_default();
    let task_routing_rules = routing
        .task_routing_rules
        .iter()
        .map(|rule| TaskRoutingRuleSummary {
            modes: rule.modes.clone(),
            task_classes: rule.task_classes.clone(),
            difficulties: rule.difficulties.clone(),
            risks: rule.risks.clone(),
            candidates: rule.candidates.iter().map(to_summary).collect(),
        })
        .collect();

    let mut routed_backends: Vec<&str> = routing
        .pm_candidates
        .iter()
        .flatten()
        .chain(routing.improve_candidates.iter().flatten())
        .chain(routing.review_candidates.iter().flatten())
        .chain(
            routing
                .task_routing_rules
                .iter()
                .flat_map(|rule| rule.candidates.iter()),
        )
        .map(|candidate| candidate.backend.as_str())
        .chain(routine_reviewer.iter().map(|c| c.backend.as_str()))
        .chain(escalatory_reviewers.iter().map(|c| c.backend.as_str()))
        .collect();
    routed_backends.sort_unstable();
    routed_backends.dedup();
    let effective_by_backend = routed_backends
        .into_iter()
        .map(|backend| ConfigBackendContextSummary {
            backend: backend.to_string(),
            effective: to_context(&cfg.context.effective(profile_name, backend)),
            backend_override: cfg.context.backends.get(backend).map(to_context_override),
        })
        .collect();

    Ok(ConfigProfileSummary {
        profile: profile_name.to_string(),
        delivery_mode: profile.delivery_mode.as_str().to_string(),
        merge_policy: routing
            .merge_policy
            .unwrap_or_default()
            .as_str()
            .to_string(),
        max_fix_attempts_per_mr: routing.max_fix_attempts_per_mr(),
        max_implementation_failures_per_ticket: routing.max_implementation_failures_per_ticket(),
        max_review_cycles_per_ticket: routing.max_review_cycles_per_ticket(),
        max_paid_reviews_per_ticket: routing.max_paid_reviews_per_ticket(),
        backend_instances,
        pm_candidates,
        improve_candidates,
        review_candidates,
        task_routing_rules,
        routine_reviewer: routine_reviewer.as_ref().map(to_summary),
        escalatory_reviewers: escalatory_reviewers
            .iter()
            .map(to_summary)
            .collect::<Vec<_>>(),
        context: ConfigProfileContextSummary {
            global: to_context(&cfg.context),
            profile_override: cfg
                .context
                .profiles
                .get(profile_name)
                .map(to_context_override),
            effective_by_backend,
        },
        notifications: NotificationSummary {
            configured: profile.notify_command.is_some(),
            transport: notification_transport(profile.notify_command.as_deref()),
            manager_wake_autonomy: wake_autonomy(profile.manager_wake_autonomy),
            env_file: profile.env_file.clone(),
            env_file_prod: profile.env_file_prod.clone(),
        },
        pm_orchestration: PmOrchestrationSummary {
            decomposition_labels: profile.publishing.pm_decomposition_labels(),
            max_children: profile.publishing.pm_max_children() as u32,
            max_depth: profile.publishing.pm_max_depth(),
            max_attempts: profile.publishing.pm_max_attempts() as u32,
            timeout_seconds: profile.publishing.pm_timeout_seconds(),
            difficulty_labels: profile.publishing.pm_difficulty_labels.clone(),
            risk_labels: profile.publishing.pm_risk_labels.clone(),
            execution_labels: profile.publishing.pm_execution_labels.clone(),
        },
    })
}

pub fn config_show(cfg: &config::GahConfig) -> ConfigShowSummary {
    ConfigShowSummary {
        current_manager: cfg.defaults.current_manager.clone(),
    }
}

pub fn config_show_json(cfg: &config::GahConfig) -> Result<String> {
    let show = config_show(cfg);
    Ok(serde_json::to_string(&show)?)
}

pub fn config_show_full(
    cfg: &config::GahConfig,
    config_path: &Path,
    profile: Option<&str>,
) -> Result<ConfigShowFull> {
    let mut profiles = BTreeMap::new();
    if let Some(profile_name) = profile {
        profiles.insert(
            profile_name.to_string(),
            build_profile_summary(cfg, profile_name)?,
        );
    } else {
        for profile_name in cfg.profiles.keys() {
            profiles.insert(
                profile_name.clone(),
                build_profile_summary(cfg, profile_name)?,
            );
        }
    }

    Ok(ConfigShowFull {
        schema_version: CONFIG_SHOW_SCHEMA_VERSION,
        config_path: config_path.to_string_lossy().into_owned(),
        current_manager: cfg.defaults.current_manager.clone(),
        profiles,
    })
}

pub fn config_show_full_json(
    cfg: &config::GahConfig,
    config_path: &Path,
    profile: Option<&str>,
) -> Result<String> {
    let mut value = serde_json::to_value(config_show_full(cfg, config_path, profile)?)?;
    crate::redact::redact_json_value(&mut value);
    Ok(serde_json::to_string(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        self, tests::test_profile_for_notifications, CandidateConfig, MergePolicy, TaskRoutingRule,
        WakeAutonomy,
    };
    use crate::context::{ContextConfig, ContextOverride};
    use serde_json::Value;
    use std::collections::HashMap;

    fn sample_profile() -> crate::config::Profile {
        let mut profile = test_profile_for_notifications();
        profile.routing.pm_candidates = Some(vec![CandidateConfig {
            backend: "claude".into(),
            instance: None,
            model: Some("sonnet".into()),
            quota_pool: Some("default".into()),
            priority: 10,
            included_in_quota: true,
            marginal_cost_usd: Some(0.5),
            quota_usage_percent: Some(7.5),
            quota_days_remaining: Some(9.25),
            requires_approval: true,
        }]);
        profile.routing.improve_candidates = Some(vec![CandidateConfig {
            backend: "vibe".into(),
            instance: None,
            model: None,
            quota_pool: None,
            priority: 20,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        }]);
        profile.routing.review_candidates = Some(vec![CandidateConfig {
            backend: "openhands".into(),
            instance: None,
            model: Some("opus".into()),
            quota_pool: Some("review".into()),
            priority: 30,
            included_in_quota: true,
            marginal_cost_usd: Some(1.25),
            quota_usage_percent: Some(11.0),
            quota_days_remaining: Some(2.0),
            requires_approval: true,
        }]);
        profile.routing.routine_reviewer = Some(CandidateConfig {
            backend: "codex".into(),
            instance: None,
            model: Some("gpt-4.1".into()),
            quota_pool: None,
            priority: 5,
            included_in_quota: true,
            marginal_cost_usd: Some(0.12),
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        });
        profile.routing.escalatory_reviewers = vec![CandidateConfig {
            backend: "ogy".into(),
            instance: None,
            model: None,
            quota_pool: None,
            priority: 15,
            included_in_quota: true,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        }];
        profile.routing.task_routing_rules = vec![TaskRoutingRule {
            difficulties: vec!["easy".into()],
            candidates: vec![CandidateConfig {
                backend: "opencode".into(),
                model: Some("hy3".into()),
                priority: 1,
                ..CandidateConfig::default()
            }],
            ..TaskRoutingRule::default()
        }];
        profile.routing.merge_policy = Some(MergePolicy::StopForHuman);
        profile.notify_command = Some("send-telegram-notification".into());
        profile.manager_wake_autonomy = WakeAutonomy::ReviewOnly;
        profile.env_file = Some("/config/dev.env".into());
        profile
    }

    fn sample_config() -> config::GahConfig {
        let mut context = ContextConfig::default();
        context.profiles.insert(
            "repo".into(),
            ContextOverride {
                enabled: Some(false),
                soft_limit_tokens: Some(90_000),
                hard_limit_tokens: Some(180_000),
                compact_after_tool_calls: Some(22),
                fresh_context_on_review: Some(false),
                fresh_context_on_fix: Some(false),
                include_full_git_history: Some(true),
                include_full_worker_transcript_in_review: Some(true),
                recent_history_tokens: Some(12_500),
            },
        );
        context.backends.insert(
            "claude".into(),
            ContextOverride {
                soft_limit_tokens: Some(50_000),
                ..Default::default()
            },
        );

        let mut cfg = config::GahConfig {
            defaults: config::Defaults::default(),
            profiles: HashMap::new(),
            context,
        };
        cfg.defaults.current_manager = Some("manager".into());
        cfg.profiles.insert("repo".into(), sample_profile());
        cfg
    }

    #[test]
    fn to_summary_copies_all_candidate_fields() {
        let candidate = CandidateConfig {
            backend: "claude".into(),
            instance: None,
            model: Some("opus".into()),
            quota_pool: Some("main".into()),
            priority: 12,
            included_in_quota: true,
            marginal_cost_usd: Some(1.5),
            quota_usage_percent: Some(8.25),
            quota_days_remaining: Some(3.75),
            requires_approval: true,
        };

        let summary = to_summary(&candidate);

        assert_eq!(summary.backend, "claude");
        assert_eq!(summary.model.as_deref(), Some("opus"));
        assert_eq!(summary.quota_pool.as_deref(), Some("main"));
        assert_eq!(summary.priority, 12);
        assert!(summary.included_in_quota);
        assert_eq!(summary.marginal_cost_usd, Some(1.5));
        assert_eq!(summary.quota_usage_percent, Some(8.25));
        assert_eq!(summary.quota_days_remaining, Some(3.75));
        assert!(summary.requires_approval);
    }

    #[test]
    fn to_context_maps_base_context_fields() {
        let cfg = ContextConfig {
            enabled: false,
            soft_limit_tokens: 5_000,
            hard_limit_tokens: 10_000,
            compact_after_tool_calls: 17,
            fresh_context_on_review: false,
            fresh_context_on_fix: false,
            include_full_git_history: true,
            include_full_worker_transcript_in_review: true,
            recent_history_tokens: 9_999,
            profiles: HashMap::new(),
            backends: HashMap::new(),
        };

        let summary = to_context(&cfg);

        assert!(!summary.enabled);
        assert_eq!(summary.soft_limit_tokens, 5_000);
        assert_eq!(summary.hard_limit_tokens, 10_000);
        assert_eq!(summary.compact_after_tool_calls, 17);
        assert!(!summary.fresh_context_on_review);
        assert!(!summary.fresh_context_on_fix);
        assert!(summary.include_full_git_history);
        assert!(summary.include_full_worker_transcript_in_review);
        assert_eq!(summary.recent_history_tokens, 9_999);
    }

    #[test]
    fn to_context_override_maps_optional_context_fields() {
        let override_cfg = ContextOverride {
            enabled: Some(false),
            soft_limit_tokens: Some(1_000),
            hard_limit_tokens: Some(2_000),
            compact_after_tool_calls: Some(4),
            fresh_context_on_review: Some(true),
            fresh_context_on_fix: Some(false),
            include_full_git_history: Some(true),
            include_full_worker_transcript_in_review: Some(false),
            recent_history_tokens: Some(300),
        };

        let summary = to_context_override(&override_cfg);

        assert_eq!(summary.enabled, Some(false));
        assert_eq!(summary.soft_limit_tokens, Some(1_000));
        assert_eq!(summary.hard_limit_tokens, Some(2_000));
        assert_eq!(summary.compact_after_tool_calls, Some(4));
        assert_eq!(summary.fresh_context_on_review, Some(true));
        assert_eq!(summary.fresh_context_on_fix, Some(false));
        assert_eq!(summary.include_full_git_history, Some(true));
        assert_eq!(
            summary.include_full_worker_transcript_in_review,
            Some(false)
        );
        assert_eq!(summary.recent_history_tokens, Some(300));
    }

    #[test]
    fn build_profile_summary_projects_full_projection() {
        let cfg = sample_config();

        let summary = build_profile_summary(&cfg, "repo").expect("profile should resolve");

        assert_eq!(summary.profile, "repo");
        assert_eq!(summary.merge_policy, MergePolicy::StopForHuman.as_str());
        assert_eq!(summary.max_fix_attempts_per_mr, 2);
        assert_eq!(summary.max_implementation_failures_per_ticket, 8);
        assert_eq!(summary.max_review_cycles_per_ticket, 3);
        assert_eq!(summary.max_paid_reviews_per_ticket, 3);
        assert_eq!(
            summary.pm_orchestration.decomposition_labels,
            ["planning", "plan"]
        );
        assert_eq!(summary.pm_orchestration.max_children, 12);
        assert_eq!(summary.pm_orchestration.max_depth, 1);
        assert_eq!(summary.pm_orchestration.max_attempts, 2);
        assert_eq!(summary.pm_orchestration.timeout_seconds, 900);

        assert_eq!(summary.pm_candidates.len(), 1);
        assert_eq!(summary.pm_candidates[0].backend, "claude");
        assert_eq!(summary.pm_candidates[0].model.as_deref(), Some("sonnet"));

        assert_eq!(summary.improve_candidates.len(), 1);
        assert_eq!(summary.improve_candidates[0].backend, "vibe");

        assert_eq!(summary.review_candidates.len(), 1);
        assert_eq!(summary.review_candidates[0].backend, "openhands");
        assert_eq!(summary.task_routing_rules.len(), 1);
        assert_eq!(
            summary.task_routing_rules[0].candidates[0].backend,
            "opencode"
        );

        assert!(summary.routine_reviewer.is_some());
        assert_eq!(summary.routine_reviewer.as_ref().unwrap().backend, "codex");
        assert_eq!(summary.escalatory_reviewers.len(), 1);

        assert!(summary.context.profile_override.is_some());
        assert!(summary.context.global.enabled);
        assert_eq!(
            summary.context.profile_override.unwrap().enabled,
            Some(false)
        );

        // pm=claude, improve=vibe, review=openhands, routine_reviewer=codex,
        // escalatory=ogy.
        assert_eq!(
            summary
                .context
                .effective_by_backend
                .iter()
                .map(|entry| entry.backend.as_str())
                .collect::<Vec<_>>(),
            vec!["claude", "codex", "ogy", "opencode", "openhands", "vibe"]
        );
        let claude = summary
            .context
            .effective_by_backend
            .iter()
            .find(|entry| entry.backend == "claude")
            .unwrap();
        // Profile override sets soft_limit_tokens to 90_000, but the
        // claude-specific backend override further narrows it to 50_000.
        assert_eq!(claude.effective.soft_limit_tokens, 50_000);
        assert_eq!(
            claude.backend_override.as_ref().unwrap().soft_limit_tokens,
            Some(50_000)
        );
        let vibe = summary
            .context
            .effective_by_backend
            .iter()
            .find(|entry| entry.backend == "vibe")
            .unwrap();
        assert!(vibe.backend_override.is_none());
        assert_eq!(vibe.effective.soft_limit_tokens, 90_000);
        assert!(summary.notifications.configured);
        assert_eq!(summary.notifications.transport.as_deref(), Some("telegram"));
        assert_eq!(summary.notifications.manager_wake_autonomy, "review_only");
        assert_eq!(
            summary.notifications.env_file.as_deref(),
            Some("/config/dev.env")
        );
    }

    #[test]
    fn build_profile_summary_unknown_profile_returns_error() {
        let cfg = sample_config();
        let err = match build_profile_summary(&cfg, "missing") {
            Ok(_) => panic!("missing profile should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("profile 'missing' not found"));
    }

    #[test]
    fn bare_config_show_json_retains_legacy_shape() {
        let cfg = sample_config();
        let raw = config_show_json(&cfg).expect("json encoding should work");
        let payload: Value = serde_json::from_str(&raw).expect("json should parse");

        assert_eq!(raw, r#"{"current_manager":"manager"}"#);
        assert_eq!(payload["current_manager"], "manager");
    }

    #[test]
    fn full_config_show_is_versioned_keyed_and_redacted() {
        let mut cfg = sample_config();
        cfg.defaults.current_manager = Some("sk-abcdefghijklmnopqrstuvwxyz123456".into());
        cfg.profiles.get_mut("repo").unwrap().notify_command =
            Some("curl -H 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz' telegram".into());

        let raw = config_show_full_json(&cfg, Path::new("/tmp/config.toml"), Some("repo"))
            .expect("full JSON encoding should work");
        let payload: Value = serde_json::from_str(&raw).expect("json should parse");

        assert_eq!(payload["schema_version"], CONFIG_SHOW_SCHEMA_VERSION);
        assert_eq!(payload["config_path"], "/tmp/config.toml");
        assert_eq!(payload["current_manager"], "[REDACTED:API_KEY]");
        assert_eq!(payload["profiles"]["repo"]["profile"], "repo");
        assert_eq!(
            payload["profiles"]["repo"]["notifications"]["transport"],
            "telegram"
        );
        assert!(!raw.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!raw.contains("Authorization"));
    }

    #[test]
    fn full_config_show_without_filter_contains_all_profiles() {
        let cfg = sample_config();
        let summary = config_show_full(&cfg, Path::new("/tmp/config.toml"), None)
            .expect("full projection should build");

        assert_eq!(summary.profiles.len(), 1);
        assert!(summary.profiles.contains_key("repo"));
    }
}
