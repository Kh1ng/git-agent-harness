use crate::{
    config,
    context::{ContextConfig, ContextOverride},
};
use anyhow::Result;

#[derive(serde::Serialize)]
pub struct RoutingCandidateSummary {
    pub backend: String,
    pub model: Option<String>,
    pub quota_pool: Option<String>,
    pub priority: i32,
    pub included_in_quota: bool,
    pub marginal_cost_usd: Option<f64>,
    pub quota_usage_percent: Option<f64>,
    pub quota_days_remaining: Option<f64>,
    pub requires_approval: bool,
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
pub struct ConfigProfileSummary {
    pub profile: String,
    pub merge_policy: String,
    pub max_fix_attempts_per_mr: u32,
    pub max_implementation_failures_per_ticket: u32,
    pub max_review_cycles_per_ticket: u32,
    pub max_paid_reviews_per_ticket: u32,
    pub pm_candidates: Vec<RoutingCandidateSummary>,
    pub improve_candidates: Vec<RoutingCandidateSummary>,
    pub review_candidates: Vec<RoutingCandidateSummary>,
    pub routine_reviewer: Option<RoutingCandidateSummary>,
    pub escalatory_reviewers: Vec<RoutingCandidateSummary>,
    pub context: ConfigProfileContextSummary,
}

#[derive(serde::Serialize)]
pub struct ConfigShow {
    pub current_manager: Option<String>,
    pub profile: Option<ConfigProfileSummary>,
}

fn to_summary(candidate: &config::CandidateConfig) -> RoutingCandidateSummary {
    RoutingCandidateSummary {
        backend: candidate.backend.clone(),
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

fn build_profile_summary(
    cfg: &config::GahConfig,
    profile_name: &str,
) -> Result<ConfigProfileSummary> {
    let profile = config::get_profile(cfg, profile_name)?;
    let routing = profile.effective_routing(&cfg.defaults);
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

    let mut routed_backends: Vec<&str> = routing
        .pm_candidates
        .iter()
        .flatten()
        .chain(routing.improve_candidates.iter().flatten())
        .chain(routing.review_candidates.iter().flatten())
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
        merge_policy: routing
            .merge_policy
            .unwrap_or_default()
            .as_str()
            .to_string(),
        max_fix_attempts_per_mr: routing.max_fix_attempts_per_mr(),
        max_implementation_failures_per_ticket: routing.max_implementation_failures_per_ticket(),
        max_review_cycles_per_ticket: routing.max_review_cycles_per_ticket(),
        max_paid_reviews_per_ticket: routing.max_paid_reviews_per_ticket(),
        pm_candidates,
        improve_candidates,
        review_candidates,
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
    })
}

pub fn config_show(cfg: &config::GahConfig, profile: Option<&str>) -> Result<ConfigShow> {
    let profile = profile
        .map(|profile_name| build_profile_summary(cfg, profile_name))
        .transpose()?;

    Ok(ConfigShow {
        current_manager: cfg.defaults.current_manager.clone(),
        profile,
    })
}

pub fn config_show_json(cfg: &config::GahConfig, profile: Option<&str>) -> Result<String> {
    let show = config_show(cfg, profile)?;
    Ok(serde_json::to_string(&show)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        self, tests::test_profile_for_notifications, CandidateConfig, MergePolicy,
    };
    use crate::context::{ContextConfig, ContextOverride};
    use serde_json::Value;
    use std::collections::HashMap;

    fn sample_profile() -> crate::config::Profile {
        let mut profile = test_profile_for_notifications();
        profile.routing.pm_candidates = Some(vec![CandidateConfig {
            backend: "claude".into(),
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
            model: None,
            quota_pool: None,
            priority: 15,
            included_in_quota: true,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        }];
        profile.routing.merge_policy = Some(MergePolicy::StopForHuman);
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

        assert_eq!(summary.pm_candidates.len(), 1);
        assert_eq!(summary.pm_candidates[0].backend, "claude");
        assert_eq!(summary.pm_candidates[0].model.as_deref(), Some("sonnet"));

        assert_eq!(summary.improve_candidates.len(), 1);
        assert_eq!(summary.improve_candidates[0].backend, "vibe");

        assert_eq!(summary.review_candidates.len(), 1);
        assert_eq!(summary.review_candidates[0].backend, "openhands");

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
            vec!["claude", "codex", "ogy", "openhands", "vibe"]
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
    fn config_show_json_includes_expected_shape_when_profile_set() {
        let cfg = sample_config();
        let raw = config_show_json(&cfg, Some("repo")).expect("json encoding should work");
        let payload: Value = serde_json::from_str(&raw).expect("json should parse");

        assert_eq!(payload["current_manager"], "manager");
        assert_eq!(payload["profile"]["profile"], "repo");
    }

    #[test]
    fn config_show_json_omits_profile_with_missing_argument() {
        let cfg = sample_config();
        let raw = config_show_json(&cfg, None).expect("json encoding should work");
        let payload: Value = serde_json::from_str(&raw).expect("json should parse");

        assert_eq!(payload["profile"], Value::Null);
    }
}
