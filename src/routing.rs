use crate::config::{Defaults, Profile, RoutingPolicy};
use crate::ledger::BackendUsageSummary;
use crate::runner;
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct RouteRequest<'a> {
    pub mode: &'a str,
    pub requested_backend: &'a str,
    pub requested_model: Option<&'a str>,
    pub recommended_backend: Option<&'a str>,
    pub recommended_model: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub usage_summary: Option<BackendUsageSummary>,
}

#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub requested_backend: String,
    pub effective_backend: String,
    pub requested_model: Option<String>,
    pub effective_model: Option<String>,
    pub routing_reason: String,
    pub fallback_used: bool,
    pub confidence_impact: Option<String>,
    pub human_required: bool,
}

pub fn decide(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
) -> Result<RouteDecision> {
    let requested_backend = req.requested_backend.to_string();
    let requested_model = req.requested_model.map(str::to_string);

    if req.requested_backend != "auto" {
        return route_explicit(defaults, profile, req, requested_backend, requested_model);
    }

    let profile_mode = policy_backend_model(&profile.routing, req.mode);
    let default_mode = policy_backend_model(&defaults.routing, req.mode);
    let review_fallback_allowed =
        profile.routing.allow_review_fallback || defaults.routing.allow_review_fallback;
    let allow_impl_fallback = profile.routing.allow_implementation_fallback
        || defaults.routing.allow_implementation_fallback;

    let mut backend = profile_mode
        .0
        .or(default_mode.0)
        .or(req.recommended_backend)
        .map(str::to_string)
        .unwrap_or_else(|| builtin_backend(req.mode));
    let mut model = profile_mode
        .1
        .or(default_mode.1)
        .or(req.recommended_model)
        .map(str::to_string);
    let mut reason = if profile_mode.0.is_some() || profile_mode.1.is_some() {
        "profile routing policy".to_string()
    } else if default_mode.0.is_some() || default_mode.1.is_some() {
        "global routing policy".to_string()
    } else if req.recommended_backend.is_some() || req.recommended_model.is_some() {
        "PM recommendation".to_string()
    } else {
        "built-in default".to_string()
    };
    let mut fallback_used = false;
    let mut confidence_impact = None;
    let mut human_required = false;
    if let Some(summary) = &req.usage_summary {
        if let Some(cap_reason) = over_cap_reason(
            &profile.routing,
            &defaults.routing,
            &backend,
            req.session_id,
            summary,
        ) {
            if req.mode == "review" && review_fallback_allowed {
                let fallback = review_fallback_backend(defaults, profile)
                    .or_else(any_available_backend)
                    .unwrap_or_else(|| backend.clone());
                if fallback != backend {
                    reason = format!("{}; {}", reason, cap_reason);
                    fallback_used = true;
                    confidence_impact = Some("low".into());
                    human_required = true;
                    backend = fallback;
                }
            } else if req.mode != "review" && allow_impl_fallback {
                let fallback = any_available_backend().unwrap_or_else(|| backend.clone());
                if fallback != backend {
                    fallback_used = true;
                    confidence_impact = Some("medium".into());
                    backend = fallback;
                    reason = format!(
                        "{}; {}; implementation fallback due to routing caps",
                        reason, cap_reason
                    );
                }
            } else {
                anyhow::bail!("{}", cap_reason);
            }
        }
    }

    if !runner::backend_available(&backend) {
        if req.mode == "review" && review_fallback_allowed {
            let fallback = review_fallback_backend(defaults, profile)
                .or_else(any_available_backend)
                .unwrap_or_else(|| backend.clone());
            if fallback != backend {
                reason = format!("{}; review fallback to available backend", reason);
                backend = fallback;
                if model.is_none() {
                    model = review_fallback_model(defaults, profile).map(str::to_string);
                }
                fallback_used = true;
                confidence_impact = Some("low".into());
                human_required = true;
            }
        } else {
            anyhow::bail!("routed backend '{}' is not available", backend);
        }
    }

    Ok(RouteDecision {
        requested_backend,
        effective_backend: backend,
        requested_model,
        effective_model: model,
        routing_reason: reason,
        fallback_used,
        confidence_impact,
        human_required,
    })
}

fn route_explicit(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
    requested_backend: String,
    requested_model: Option<String>,
) -> Result<RouteDecision> {
    let allow_impl_fallback = profile.routing.allow_implementation_fallback
        || defaults.routing.allow_implementation_fallback;
    let allow_review_fallback =
        profile.routing.allow_review_fallback || defaults.routing.allow_review_fallback;

    if runner::backend_available(req.requested_backend) {
        return Ok(RouteDecision {
            requested_backend: requested_backend.clone(),
            effective_backend: requested_backend,
            requested_model: requested_model.clone(),
            effective_model: requested_model,
            routing_reason: "explicit CLI override".into(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
        });
    }

    if req.mode == "review" && allow_review_fallback {
        let fallback = review_fallback_backend(defaults, profile)
            .or_else(any_available_backend)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "explicit review backend '{}' is unavailable",
                    req.requested_backend
                )
            })?;
        return Ok(RouteDecision {
            requested_backend,
            effective_backend: fallback,
            requested_model: requested_model.clone(),
            effective_model: requested_model,
            routing_reason: "explicit CLI override unavailable; review fallback".into(),
            fallback_used: true,
            confidence_impact: Some("low".into()),
            human_required: true,
        });
    }

    if req.mode != "review" && allow_impl_fallback {
        let fallback = any_available_backend().ok_or_else(|| {
            anyhow::anyhow!(
                "explicit backend '{}' is unavailable",
                req.requested_backend
            )
        })?;
        return Ok(RouteDecision {
            requested_backend,
            effective_backend: fallback,
            requested_model: requested_model.clone(),
            effective_model: requested_model,
            routing_reason: "explicit CLI override unavailable; implementation fallback".into(),
            fallback_used: true,
            confidence_impact: Some("medium".into()),
            human_required: false,
        });
    }

    anyhow::bail!(
        "explicit backend '{}' is unavailable",
        req.requested_backend
    )
}

fn policy_backend_model<'a>(
    policy: &'a RoutingPolicy,
    mode: &str,
) -> (Option<&'a str>, Option<&'a str>) {
    match mode {
        "pm" => (
            policy
                .pm_backend
                .as_deref()
                .or(policy.default_backend.as_deref()),
            policy
                .pm_model
                .as_deref()
                .or(policy.default_model.as_deref()),
        ),
        "review" => (
            policy
                .strong_review_backend
                .as_deref()
                .or(policy.review_backend.as_deref())
                .or(policy.default_backend.as_deref()),
            policy
                .strong_review_model
                .as_deref()
                .or(policy.review_model.as_deref())
                .or(policy.default_model.as_deref()),
        ),
        "improve" | "fix" | "experiment" => (
            policy
                .improve_backend
                .as_deref()
                .or(policy.default_backend.as_deref()),
            policy
                .improve_model
                .as_deref()
                .or(policy.default_model.as_deref()),
        ),
        _ => (
            policy.default_backend.as_deref(),
            policy.default_model.as_deref(),
        ),
    }
}

fn review_fallback_backend(defaults: &Defaults, profile: &Profile) -> Option<String> {
    profile
        .routing
        .weak_review_backend
        .clone()
        .or(defaults.routing.weak_review_backend.clone())
        .or_else(any_available_backend)
}

fn review_fallback_model<'a>(defaults: &'a Defaults, profile: &'a Profile) -> Option<&'a str> {
    profile
        .routing
        .weak_review_model
        .as_deref()
        .or(defaults.routing.weak_review_model.as_deref())
}

fn builtin_backend(mode: &str) -> String {
    let preferred = match mode {
        "pm" | "review" => ["claude", "codex", "openhands"],
        _ => ["openhands", "codex", "claude"],
    };
    preferred
        .into_iter()
        .find(|backend| runner::backend_available(backend))
        .unwrap_or("openhands")
        .to_string()
}

fn any_available_backend() -> Option<String> {
    ["claude", "codex", "openhands"]
        .into_iter()
        .find(|backend| runner::backend_available(backend))
        .map(str::to_string)
}

fn over_cap_reason(
    profile: &RoutingPolicy,
    defaults: &RoutingPolicy,
    backend: &str,
    session_id: Option<&str>,
    summary: &BackendUsageSummary,
) -> Option<String> {
    let max_runs_week = profile
        .max_runs_per_backend_per_week
        .or(defaults.max_runs_per_backend_per_week);
    if let Some(max) = max_runs_week {
        if summary.runs_this_week >= max {
            return Some(format!(
                "backend '{}' exceeded weekly run cap ({}/{})",
                backend, summary.runs_this_week, max
            ));
        }
    }
    if session_id.is_some() {
        let max_runs_session = profile
            .max_runs_per_backend_per_session
            .or(defaults.max_runs_per_backend_per_session);
        if let Some(max) = max_runs_session {
            if summary.runs_this_session >= max {
                return Some(format!(
                    "backend '{}' exceeded session run cap ({}/{})",
                    backend, summary.runs_this_session, max
                ));
            }
        }
        let max_strong_session = profile
            .max_total_strong_model_runs_per_session
            .or(defaults.max_total_strong_model_runs_per_session);
        if let Some(max) = max_strong_session {
            if summary.strong_runs_this_session >= max {
                return Some(format!(
                    "strong-model session cap reached ({}/{})",
                    summary.strong_runs_this_session, max
                ));
            }
        }
    }
    let max_strong_week = profile
        .max_total_strong_model_runs_per_week
        .or(defaults.max_total_strong_model_runs_per_week);
    if let Some(max) = max_strong_week {
        if summary.strong_runs_this_week >= max {
            return Some(format!(
                "strong-model weekly cap reached ({}/{})",
                summary.strong_runs_this_week, max
            ));
        }
    }
    let max_estimated = profile
        .max_known_estimated_cost_per_week
        .or(defaults.max_known_estimated_cost_per_week);
    if let Some(max) = max_estimated {
        if summary.estimated_cost_this_week >= max {
            return Some(format!(
                "estimated weekly cost cap reached (${:.4}/${:.4})",
                summary.estimated_cost_this_week, max
            ));
        }
    }
    let max_actual = profile
        .max_known_actual_cost_per_week
        .or(defaults.max_known_actual_cost_per_week);
    if let Some(max) = max_actual {
        if summary.actual_cost_this_week >= max {
            return Some(format!(
                "actual weekly cost cap reached (${:.4}/${:.4})",
                summary.actual_cost_this_week, max
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{decide, RouteRequest};
    use crate::config::{Defaults, Profile, RoutingPolicy};

    fn defaults() -> Defaults {
        Defaults {
            artifact_root: String::new(),
            worktree_base: String::new(),
            llm_base_url: String::new(),
            llm_model_local: String::new(),
            llm_model_cloud: String::new(),
            routing: RoutingPolicy {
                default_backend: Some("codex".into()),
                weak_review_backend: Some("codex".into()),
                allow_review_fallback: true,
                ..RoutingPolicy::default()
            },
        }
    }

    fn profile() -> Profile {
        Profile {
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: "/tmp/repo".into(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: None,
            provider_project_id: None,
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            claude_args: vec![],
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            test_file_patterns: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            routing: RoutingPolicy {
                pm_backend: Some("claude".into()),
                ..RoutingPolicy::default()
            },
        }
    }

    #[test]
    fn profile_routing_beats_global_policy() {
        let decision = decide(
            &defaults(),
            &profile(),
            RouteRequest {
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: Some("openhands"),
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
        )
        .unwrap();
        assert_eq!(decision.effective_backend, "claude");
        assert_eq!(decision.routing_reason, "profile routing policy");
    }
}
