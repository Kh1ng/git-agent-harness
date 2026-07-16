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
pub struct ConfigProfileContextSummary {
    pub global: ContextBudgetSummary,
    pub effective: ContextBudgetSummary,
    pub profile_override: Option<ContextOverrideBudgetSummary>,
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

fn build_profile_summary(cfg: &config::GahConfig, profile_name: &str) -> ConfigProfileSummary {
    let profile =
        config::get_profile(cfg, profile_name).expect("profile missing from prevalidated config");
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

    ConfigProfileSummary {
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
            effective: to_context(&cfg.context.effective(profile_name, "")),
            profile_override: cfg
                .context
                .profiles
                .get(profile_name)
                .map(to_context_override),
        },
    }
}

pub fn config_show(cfg: &config::GahConfig, profile: Option<&str>) -> ConfigShow {
    let profile = profile.map(|profile_name| build_profile_summary(cfg, profile_name));

    ConfigShow {
        current_manager: cfg.defaults.current_manager.clone(),
        profile,
    }
}

pub fn config_show_json(cfg: &config::GahConfig, profile: Option<&str>) -> Result<String> {
    Ok(serde_json::to_string(&config_show(cfg, profile))?)
}
