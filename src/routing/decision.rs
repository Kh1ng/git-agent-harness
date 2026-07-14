//! Route evaluation, eligibility checks, and candidate selection.

use super::diagnostics::build_routing_diagnostics;
use super::policy::{
    any_available_backend, append_reorder_reason, auto_candidates, builtin_backend,
    explicit_candidates, is_genuine_agent_failure, order_candidates, policy_backend_model,
    policy_candidates, review_fallback_backend, review_fallback_model, task_rule_candidates,
    RouteCandidate,
};
use super::reservation::max_concurrent_skip;
use super::types::{
    render_skips, CandidateIdentity, RouteDecision, RouteError, RouteRequest, RoutingRuntimeState,
    SkippedBackend, TaskRoutingContext,
};
use crate::availability::{self, AvailabilityDecision, BlockScope};
use crate::config::{Defaults, Profile, RoutingPolicy};
use crate::ledger::BackendUsageSummary;
use crate::runner;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
pub(super) struct RouteEvaluation<'a> {
    pub(super) state_path: &'a Path,
    pub(super) now: OffsetDateTime,
}

pub fn decide_with_state(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
    runtime: &RoutingRuntimeState,
) -> Result<RouteDecision> {
    decide_with_runtime(
        defaults,
        profile,
        req,
        runtime,
        &availability::resolve_state_path(),
        OffsetDateTime::now_utc(),
        |backend| runner::backend_available_for_profile(profile, backend),
    )
}

/// Route an implementation request with deterministic task-class rules. A
/// matching rule wins over the generic implementation candidate list, but
/// unavailable or saturated entries still fall through to the next candidate
/// in that same rule.
pub fn decide_for_task_with_state(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
    task: TaskRoutingContext<'_>,
    runtime: &RoutingRuntimeState,
) -> Result<RouteDecision> {
    decide_with_task_runtime(
        defaults,
        profile,
        req,
        Some(task),
        runtime,
        RouteEvaluation {
            state_path: &availability::resolve_state_path(),
            now: OffsetDateTime::now_utc(),
        },
        |backend| runner::backend_available_for_profile(profile, backend),
    )
}

#[cfg(test)]
pub(super) fn decide_with<F>(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
    state_path: &Path,
    now: OffsetDateTime,
    backend_available: F,
) -> Result<RouteDecision>
where
    F: Fn(&str) -> bool + Copy,
{
    decide_with_runtime(
        defaults,
        profile,
        req,
        &RoutingRuntimeState::default(),
        state_path,
        now,
        backend_available,
    )
}

pub(super) fn decide_with_runtime<F>(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
    runtime: &RoutingRuntimeState,
    state_path: &Path,
    now: OffsetDateTime,
    backend_available: F,
) -> Result<RouteDecision>
where
    F: Fn(&str) -> bool + Copy,
{
    decide_with_task_runtime(
        defaults,
        profile,
        req,
        None,
        runtime,
        RouteEvaluation { state_path, now },
        backend_available,
    )
}

#[cfg(test)]
pub(super) fn decide_with_task<F>(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
    task: Option<TaskRoutingContext<'_>>,
    state_path: &Path,
    now: OffsetDateTime,
    backend_available: F,
) -> Result<RouteDecision>
where
    F: Fn(&str) -> bool + Copy,
{
    decide_with_task_runtime(
        defaults,
        profile,
        req,
        task,
        &RoutingRuntimeState::default(),
        RouteEvaluation { state_path, now },
        backend_available,
    )
}

pub(super) fn decide_with_task_runtime<F>(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
    task: Option<TaskRoutingContext<'_>>,
    runtime: &RoutingRuntimeState,
    evaluation: RouteEvaluation<'_>,
    backend_available: F,
) -> Result<RouteDecision>
where
    F: Fn(&str) -> bool + Copy,
{
    let auto_model_override = (req.requested_backend == "auto")
        .then(|| req.requested_model.map(str::to_string))
        .flatten();
    let mode = req.mode.to_string();
    let last_failure_class = req.last_failure_class;
    let (mut decision, candidates) = decide_with_inner(
        defaults,
        profile,
        req,
        task,
        runtime,
        evaluation,
        backend_available,
    )?;
    if let Some(model) = auto_model_override {
        // The requested model may not match any configured candidate at all
        // (an ad-hoc override unrelated to the routing config) -- that's
        // fine, nothing gates it. But if it DOES match a configured
        // candidate for the selected backend, that candidate must pass the
        // same eligibility checks (availability, already-attempted,
        // requires_approval) every other candidate had to pass, or an
        // explicit --model flag would be a standing bypass of the approval
        // gate this routing layer exists to enforce.
        if decision.effective_model.as_deref() != Some(model.as_str()) {
            if let Some(candidate) = candidates.iter().find(|c| {
                c.backend == decision.effective_backend
                    && c.model.as_deref() == Some(model.as_str())
            }) {
                let escalate =
                    is_genuine_agent_failure(last_failure_class) || !runtime.attempted.is_empty();
                if let Some(skip) = skip_reason_for_candidate(
                    evaluation.state_path,
                    &candidate.backend,
                    candidate.model.as_deref(),
                    candidate.quota_pool.as_deref(),
                    &profile.max_concurrent_per_model,
                    evaluation.now,
                    backend_available,
                    runtime,
                    escalate,
                    candidate.requires_approval,
                )? {
                    if skip.reason == "operator_approval_required" {
                        return Err(RouteError::ApprovalRequired {
                            backend: skip.backend.clone(),
                            model: skip.model.clone(),
                            skipped: vec![skip],
                        }
                        .into());
                    }
                    return Err(RouteError::NoEligibleBackend {
                        preferred_backend: candidate.backend.clone(),
                        preferred_model: candidate.model.clone(),
                        skipped: vec![skip],
                        earliest_reset: None,
                    }
                    .into());
                }
            }
        }
        let previous_model = decision.effective_model.replace(model.clone());
        decision.effective_quota_pool = profile.effective_routing(defaults).find_quota_pool(
            &mode,
            &decision.effective_backend,
            Some(&model),
        );
        if let Some(diagnostics) = decision.routing_diagnostics.as_mut() {
            diagnostics.selected_model = Some(model.clone());
            diagnostics.selected_quota_pool = decision.effective_quota_pool.clone();
            if let Some(candidate) = diagnostics.candidates.iter_mut().find(|candidate| {
                candidate.backend == decision.effective_backend
                    && candidate.model == previous_model
                    && candidate.skip_reason.is_none()
            }) {
                candidate.model = Some(model.clone());
                candidate.quota_pool = decision.effective_quota_pool.clone();
            }
            let tail = diagnostics
                .human_summary
                .as_deref()
                .and_then(|summary| summary.split_once("; ").map(|(_, tail)| tail));
            diagnostics.human_summary = Some(match tail {
                Some(tail) => format!(
                    "selected {}/{} (explicit CLI model override); {tail}",
                    decision.effective_backend, model
                ),
                None => format!(
                    "selected {}/{} (explicit CLI model override)",
                    decision.effective_backend, model
                ),
            });
        }
    }
    if decision.effective_backend == "codex" && decision.effective_model.is_none() {
        if let Some(model) = runner::extract_model_from_args(&profile.codex_args) {
            decision.effective_model = Some(model);
        }
    }
    Ok(decision)
}

/// Returns the decision alongside the exact candidate list it was chosen
/// from, so a caller applying a post-hoc override (an explicit CLI model
/// with `requested_backend == "auto"`) can validate the override target
/// against the same eligibility gates every other candidate had to pass --
/// including `requires_approval` -- rather than swapping it in unchecked.
fn decide_with_inner<F>(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
    task: Option<TaskRoutingContext<'_>>,
    runtime: &RoutingRuntimeState,
    evaluation: RouteEvaluation<'_>,
    backend_available: F,
) -> Result<(RouteDecision, Vec<RouteCandidate>)>
where
    F: Fn(&str) -> bool + Copy,
{
    let state_path = evaluation.state_path;
    let now = evaluation.now;
    let requested_backend = req.requested_backend.to_string();
    let requested_model = req.requested_model.map(str::to_string);
    let effective_routing = profile.effective_routing(defaults);

    if req.requested_backend != "auto" {
        // No auto_model_override is ever applied on this path (it only
        // triggers for requested_backend == "auto"), so the candidate list
        // is never consulted here.
        return route_explicit(
            defaults,
            profile,
            &effective_routing,
            req,
            requested_backend,
            requested_model,
            state_path,
            now,
            backend_available,
            runtime,
        )
        .map(|decision| (decision, Vec::new()));
    }

    if let Some((rule_index, candidates)) = task_rule_candidates(&effective_routing, req.mode, task)
        .filter(|(_, list)| !list.is_empty())
    {
        let escalate =
            is_genuine_agent_failure(req.last_failure_class) || !runtime.attempted.is_empty();
        let (candidates, reorder) =
            order_candidates(profile, candidates, escalate, runtime, req.mode);
        let preferred = candidates
            .first()
            .cloned()
            .expect("non-empty task routing rule");
        let candidates_for_diagnostics = candidates.clone();
        let (selected, skipped) = pick_route_candidate(
            candidates,
            state_path,
            &profile.max_concurrent_per_model,
            now,
            backend_available,
            runtime,
            escalate,
        )?;
        let fallback_used =
            selected.backend != preferred.backend || selected.model != preferred.model;
        let mut reason = format!("task routing rule #{}", rule_index + 1);
        if let Some(reorder) = reorder
            .as_ref()
            .filter(|reorder| !reorder.selected_over.is_empty())
        {
            reason = append_reorder_reason(reason, &selected, reorder, &profile.pacing);
        }
        if fallback_used {
            reason = append_availability_reason(reason, &skipped, &selected.backend, false);
        }
        return Ok((
            RouteDecision {
                requested_backend,
                effective_backend: selected.backend.clone(),
                requested_model,
                effective_model: selected.model.clone(),
                effective_quota_pool: selected.quota_pool.clone(),
                routing_reason: reason,
                fallback_used,
                confidence_impact: None,
                human_required: false,
                routing_diagnostics: Some(build_routing_diagnostics(
                    &candidates_for_diagnostics,
                    &selected,
                    &skipped,
                    reorder
                        .as_ref()
                        .map(|decision| decision.selected_over.as_slice()),
                    &profile.pacing,
                )),
            },
            candidates_for_diagnostics,
        ));
    }

    let mut is_profile_policy = false;

    let candidates =
        if let Some(c) = policy_candidates(&profile.routing, req.mode).filter(|l| !l.is_empty()) {
            is_profile_policy = true;
            Some(c)
        } else if policy_candidates(&defaults.routing, req.mode)
            .filter(|l| !l.is_empty())
            .is_some()
        {
            policy_candidates(&effective_routing, req.mode).filter(|l| !l.is_empty())
        } else {
            None
        };

    if let Some(candidates) = candidates {
        let escalate =
            is_genuine_agent_failure(req.last_failure_class) || !runtime.attempted.is_empty();
        let (candidates, reorder) =
            order_candidates(profile, candidates, escalate, runtime, req.mode);
        let preferred = candidates.first().cloned().expect("non-empty list");
        let candidates_for_diagnostics = candidates.clone();
        let (selected, skipped) = pick_route_candidate(
            candidates,
            state_path,
            &profile.max_concurrent_per_model,
            now,
            backend_available,
            runtime,
            escalate,
        )?;

        let mut fallback_used = false;
        let mut confidence_impact = None;
        let mut human_required = false;
        let mut reason = if is_profile_policy {
            "profile routing policy".to_string()
        } else {
            "global routing policy".to_string()
        };

        if let Some(reorder) = reorder.as_ref().filter(|r| !r.selected_over.is_empty()) {
            reason = append_reorder_reason(reason, &selected, reorder, &profile.pacing);
        }

        if selected.backend != preferred.backend || selected.model != preferred.model {
            fallback_used = true;
            reason = append_availability_reason(
                reason,
                &skipped,
                &selected.backend,
                req.mode == "review",
            );
            if req.mode == "review" {
                confidence_impact = Some("low".into());
                human_required = true;
            }
        }

        let routing_diagnostics = Some(build_routing_diagnostics(
            &candidates_for_diagnostics,
            &selected,
            &skipped,
            reorder
                .as_ref()
                .map(|decision| decision.selected_over.as_slice()),
            &profile.pacing,
        ));
        return Ok((
            RouteDecision {
                requested_backend,
                effective_backend: selected.backend.clone(),
                requested_model,
                effective_model: selected.model.clone(),
                effective_quota_pool: selected.quota_pool.clone(),
                routing_reason: reason,
                fallback_used,
                confidence_impact,
                human_required,
                routing_diagnostics,
            },
            candidates_for_diagnostics,
        ));
    }

    let profile_mode = policy_backend_model(&profile.routing, req.mode);
    let default_mode = policy_backend_model(&defaults.routing, req.mode);
    let effective_mode = policy_backend_model(&effective_routing, req.mode);
    let review_fallback_allowed = effective_routing.allow_review_fallback;
    let allow_impl_fallback = effective_routing.allow_implementation_fallback;

    let mut backend = effective_mode
        .0
        .or(req.recommended_backend)
        .map(str::to_string)
        .unwrap_or_else(|| builtin_backend(req.mode, backend_available));
    let mut model = effective_mode
        .1
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
            &effective_routing,
            &RoutingPolicy::default(),
            &backend,
            req.session_id,
            summary,
        ) {
            if req.mode == "review" && review_fallback_allowed {
                let fallback = review_fallback_backend(defaults, profile, backend_available)
                    .unwrap_or_else(|| backend.clone());
                if fallback != backend {
                    reason = format!("{}; {}", reason, cap_reason);
                    fallback_used = true;
                    confidence_impact = Some("low".into());
                    human_required = true;
                    backend = fallback;
                    if model.is_none() {
                        model = review_fallback_model(&effective_routing).map(str::to_string);
                    }
                }
            } else if req.mode != "review" && allow_impl_fallback {
                let fallback = any_available_backend(req.mode, backend_available)
                    .unwrap_or_else(|| backend.clone());
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

    let primary = RouteCandidate {
        backend: backend.clone(),
        model: model.clone(),
        quota_pool: effective_routing.find_quota_pool(req.mode, &backend, model.as_deref()),
        priority: 0,
        included_in_quota: false,
        marginal_cost_usd: None,
        quota_usage_percent: None,
        quota_days_remaining: None,
        requires_approval: false,
        original_order: 0,
    };
    let candidates = auto_candidates(&effective_routing, req.mode, &primary);
    let candidates_for_diagnostics = candidates.clone();
    let (selected, skipped) = pick_route_candidate(
        candidates,
        state_path,
        &profile.max_concurrent_per_model,
        now,
        backend_available,
        runtime,
        false,
    )?;

    if selected.backend != primary.backend || selected.model != primary.model {
        fallback_used = true;
        reason =
            append_availability_reason(reason, &skipped, &selected.backend, req.mode == "review");
        if req.mode == "review" {
            confidence_impact.get_or_insert_with(|| "low".into());
            human_required = true;
        }
    }

    let routing_diagnostics = Some(build_routing_diagnostics(
        &candidates_for_diagnostics,
        &selected,
        &skipped,
        None,
        &profile.pacing,
    ));
    Ok((
        RouteDecision {
            requested_backend,
            effective_backend: selected.backend.clone(),
            requested_model,
            effective_model: selected.model.clone(),
            effective_quota_pool: selected.quota_pool.clone(),
            routing_reason: reason,
            fallback_used,
            confidence_impact,
            human_required,
            routing_diagnostics,
        },
        candidates_for_diagnostics,
    ))
}

#[allow(clippy::too_many_arguments)]
fn route_explicit<F>(
    defaults: &Defaults,
    profile: &Profile,
    effective_routing: &RoutingPolicy,
    req: RouteRequest<'_>,
    requested_backend: String,
    requested_model: Option<String>,
    state_path: &Path,
    now: OffsetDateTime,
    backend_available: F,
    runtime: &RoutingRuntimeState,
) -> Result<RouteDecision>
where
    F: Fn(&str) -> bool + Copy,
{
    let allow_impl_fallback = effective_routing.allow_implementation_fallback;
    let allow_review_fallback = effective_routing.allow_review_fallback;
    let review_fallback = if req.mode == "review" && allow_review_fallback {
        review_fallback_backend(defaults, profile, backend_available)
    } else {
        None
    };

    let primary = RouteCandidate {
        backend: requested_backend.clone(),
        model: requested_model.clone(),
        quota_pool: effective_routing.find_quota_pool(
            req.mode,
            &requested_backend,
            requested_model.as_deref(),
        ),
        priority: 0,
        included_in_quota: false,
        marginal_cost_usd: None,
        quota_usage_percent: None,
        quota_days_remaining: None,
        requires_approval: false,
        original_order: 0,
    };
    let candidates = explicit_candidates(
        effective_routing,
        req.mode,
        &primary,
        review_fallback,
        allow_review_fallback,
        allow_impl_fallback,
    );
    let candidates_for_diagnostics = candidates.clone();
    let (selected, skipped) = pick_route_candidate(
        candidates,
        state_path,
        &profile.max_concurrent_per_model,
        now,
        backend_available,
        runtime,
        false,
    )?;

    if selected.backend == primary.backend && selected.model == primary.model {
        let routing_diagnostics = Some(build_routing_diagnostics(
            &candidates_for_diagnostics,
            &selected,
            &skipped,
            None,
            &profile.pacing,
        ));
        return Ok(RouteDecision {
            requested_backend: requested_backend.clone(),
            effective_backend: requested_backend,
            requested_model: requested_model.clone(),
            effective_model: requested_model,
            effective_quota_pool: primary.quota_pool,
            routing_reason: "explicit CLI override".into(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics,
        });
    }

    let mut routing_reason = if req.mode == "review" {
        "explicit CLI override unavailable; review fallback".to_string()
    } else {
        "explicit CLI override unavailable; implementation fallback".to_string()
    };
    routing_reason = append_availability_reason(
        routing_reason,
        &skipped,
        &selected.backend,
        req.mode == "review",
    );

    let routing_diagnostics = Some(build_routing_diagnostics(
        &candidates_for_diagnostics,
        &selected,
        &skipped,
        None,
        &profile.pacing,
    ));
    Ok(RouteDecision {
        requested_backend,
        effective_backend: selected.backend.clone(),
        requested_model: requested_model.clone(),
        effective_model: selected.model.clone(),
        effective_quota_pool: selected.quota_pool.clone(),
        routing_reason,
        fallback_used: true,
        confidence_impact: if req.mode == "review" {
            Some("low".into())
        } else {
            Some("medium".into())
        },
        human_required: req.mode == "review",
        routing_diagnostics,
    })
}

fn append_availability_reason(
    mut base: String,
    skipped: &[SkippedBackend],
    selected_backend: &str,
    mention_human_review: bool,
) -> String {
    if !skipped.is_empty() {
        base.push_str("; ");
        base.push_str(&render_skips(skipped));
    }
    base.push_str("; availability fallback to ");
    base.push_str(selected_backend);
    if mention_human_review {
        base.push_str(" (human review required)");
    }
    base
}

fn pick_route_candidate<F>(
    candidates: Vec<RouteCandidate>,
    state_path: &Path,
    max_concurrent: &HashMap<String, u32>,
    now: OffsetDateTime,
    backend_available: F,
    runtime: &RoutingRuntimeState,
    exclude_attempted: bool,
) -> Result<(RouteCandidate, Vec<SkippedBackend>)>
where
    F: Fn(&str) -> bool + Copy,
{
    let preferred = candidates
        .first()
        .cloned()
        .expect("candidate list must never be empty");
    let mut skipped = Vec::new();
    for candidate in candidates {
        if let Some(reason) = skip_reason_for_candidate(
            state_path,
            &candidate.backend,
            candidate.model.as_deref(),
            candidate.quota_pool.as_deref(),
            max_concurrent,
            now,
            backend_available,
            runtime,
            exclude_attempted,
            candidate.requires_approval,
        )? {
            skipped.push(reason);
            continue;
        }
        return Ok((candidate, skipped));
    }
    if let Some(candidate) = skipped
        .iter()
        .find(|candidate| candidate.reason == "operator_approval_required")
    {
        return Err(RouteError::ApprovalRequired {
            backend: candidate.backend.clone(),
            model: candidate.model.clone(),
            skipped,
        }
        .into());
    }
    Err(RouteError::NoEligibleBackend {
        preferred_backend: preferred.backend,
        preferred_model: preferred.model,
        earliest_reset: earliest_reset(&skipped),
        skipped,
    }
    .into())
}

#[allow(clippy::too_many_arguments)]
fn skip_reason_for_candidate<F>(
    state_path: &Path,
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
    max_concurrent: &HashMap<String, u32>,
    now: OffsetDateTime,
    backend_available: F,
    runtime: &RoutingRuntimeState,
    exclude_attempted: bool,
    requires_approval: bool,
) -> Result<Option<SkippedBackend>>
where
    F: Fn(&str) -> bool + Copy,
{
    if !backend_available(backend) {
        return Ok(Some(SkippedBackend {
            backend: backend.to_string(),
            model: model.map(str::to_string),
            reason: "backend CLI not installed".into(),
            unavailable_until: None,
        }));
    }

    if let Some(skip) = max_concurrent_skip(max_concurrent, backend, model) {
        return Ok(Some(skip));
    }

    let decision = availability::availability_for(state_path, backend, model, quota_pool, now)?;
    if decision.eligible {
        let identity = CandidateIdentity::new(backend, model);
        if exclude_attempted && runtime.attempted.contains(&identity) {
            return Ok(Some(SkippedBackend {
                backend: backend.to_string(),
                model: model.map(str::to_string),
                reason: "already_attempted_after_capability_failure".into(),
                unavailable_until: None,
            }));
        }
        if requires_approval && !runtime.approved.contains(&identity) {
            return Ok(Some(SkippedBackend {
                backend: backend.to_string(),
                model: model.map(str::to_string),
                reason: "operator_approval_required".into(),
                unavailable_until: None,
            }));
        }
        return Ok(None);
    }

    Ok(Some(SkippedBackend {
        backend: backend.to_string(),
        model: model.map(str::to_string),
        reason: availability_reason(&decision),
        unavailable_until: decision.unavailable_until,
    }))
}

fn availability_reason(decision: &AvailabilityDecision) -> String {
    let scope = match decision.scope {
        Some(BlockScope::BackendWide) => "backend-wide",
        Some(BlockScope::ModelSpecific) => "model-specific",
        Some(BlockScope::QuotaPool) => "quota-pool",
        None => "availability",
    };
    let reason = decision
        .reason
        .map(|r| r.as_str().to_string())
        .unwrap_or_else(|| "unknown".into());
    format!("{scope} {reason}")
}

fn earliest_reset(skipped: &[SkippedBackend]) -> Option<String> {
    skipped
        .iter()
        .filter_map(|skip| skip.unavailable_until.as_deref())
        .filter_map(|ts| OffsetDateTime::parse(ts, &Rfc3339).ok().map(|dt| (dt, ts)))
        .min_by_key(|(dt, _)| *dt)
        .map(|(_, ts)| ts.to_string())
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
