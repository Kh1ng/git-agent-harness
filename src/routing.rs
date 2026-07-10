use crate::availability::{self, AvailabilityDecision, BlockScope};
use crate::config::{Defaults, Profile, RoutingPolicy};
use crate::ledger::{BackendUsageSummary, RoutingCandidateDiagnostic, RoutingDiagnostics};
use crate::quota::{self, PaceBand};
use crate::runner;
use anyhow::Result;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct RouteRequest<'a> {
    pub mode: &'a str,
    pub requested_backend: &'a str,
    pub requested_model: Option<&'a str>,
    pub recommended_backend: Option<&'a str>,
    pub recommended_model: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub usage_summary: Option<BackendUsageSummary>,
    /// TICKET-089 AC7/8: the `FailureClass::as_str()` of the immediately
    /// preceding attempt, when this route decision is a same-invocation
    /// retry. Only `agent_failure`/`agent_no_progress`/`validation_failure`
    /// (genuine agent-capability failures) may escalate candidate ordering
    /// toward a stronger model; harness/environment/backend (auth/quota)
    /// failures must not, since a stronger model doesn't fix those.
    pub last_failure_class: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub requested_backend: String,
    pub effective_backend: String,
    pub requested_model: Option<String>,
    pub effective_model: Option<String>,
    pub effective_quota_pool: Option<String>,
    pub routing_reason: String,
    pub fallback_used: bool,
    pub confidence_impact: Option<String>,
    pub human_required: bool,
    pub routing_diagnostics: Option<RoutingDiagnostics>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedBackend {
    pub backend: String,
    pub model: Option<String>,
    pub reason: String,
    pub unavailable_until: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    NoEligibleBackend {
        preferred_backend: String,
        preferred_model: Option<String>,
        skipped: Vec<SkippedBackend>,
        earliest_reset: Option<String>,
    },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::NoEligibleBackend {
                preferred_backend,
                preferred_model,
                skipped,
                earliest_reset,
            } => {
                write!(
                    f,
                    "no eligible backend available for preferred {}",
                    candidate_label(preferred_backend, preferred_model.as_deref())
                )?;
                if !skipped.is_empty() {
                    write!(f, "; skipped: {}", render_skips(skipped))?;
                }
                if let Some(reset) = earliest_reset {
                    write!(f, "; earliest reset: {}", reset)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for RouteError {}

#[derive(Debug, Clone)]
struct RouteCandidate {
    backend: String,
    model: Option<String>,
    quota_pool: Option<String>,
    priority: i32,
    included_in_quota: bool,
    marginal_cost_usd: Option<f64>,
    quota_usage_percent: Option<f64>,
    quota_days_remaining: Option<f64>,
    original_order: usize,
}

#[derive(Debug, Clone)]
struct ReorderDecision {
    selected_over: Vec<String>,
    escalated: bool,
}

pub fn decide(
    defaults: &Defaults,
    profile: &Profile,
    req: RouteRequest<'_>,
) -> Result<RouteDecision> {
    decide_with(
        defaults,
        profile,
        req,
        &availability::resolve_state_path(),
        OffsetDateTime::now_utc(),
        |backend| runner::backend_available_for_profile(profile, backend),
    )
}

fn decide_with<F>(
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
    let mut decision =
        decide_with_inner(defaults, profile, req, state_path, now, backend_available)?;
    if decision.effective_backend == "codex" && decision.effective_model.is_none() {
        if let Some(model) = runner::extract_model_from_args(&profile.codex_args) {
            decision.effective_model = Some(model);
        }
    }
    Ok(decision)
}

fn decide_with_inner<F>(
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
    let requested_backend = req.requested_backend.to_string();
    let requested_model = req.requested_model.map(str::to_string);
    let effective_routing = profile.effective_routing(defaults);

    if req.requested_backend != "auto" {
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
        );
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
        let escalate = is_genuine_agent_failure(req.last_failure_class);
        let (candidates, reorder) = order_candidates(profile, candidates, escalate);
        let preferred = candidates.first().cloned().expect("non-empty list");
        let candidates_for_diagnostics = candidates.clone();
        let (selected, skipped) =
            pick_route_candidate(candidates, state_path, now, backend_available)?;

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
            reorder.as_ref(),
            &profile.pacing,
        ));
        return Ok(RouteDecision {
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
        });
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
        original_order: 0,
    };
    let candidates = auto_candidates(&effective_routing, req.mode, &primary);
    let candidates_for_diagnostics = candidates.clone();
    let (selected, skipped) = pick_route_candidate(candidates, state_path, now, backend_available)?;

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
    Ok(RouteDecision {
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
    })
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
    let (selected, skipped) = pick_route_candidate(candidates, state_path, now, backend_available)?;

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

fn append_reorder_reason(
    mut base: String,
    selected: &RouteCandidate,
    reorder: &ReorderDecision,
    pacing: &crate::quota::PacingConfig,
) -> String {
    if reorder.escalated {
        base.push_str("; escalated to stronger model after genuine agent failure, selected ");
    } else {
        base.push_str("; cost-aware reorder selected ");
    }
    base.push_str(&describe_candidate(selected, pacing));
    base.push_str(" over ");
    base.push_str(&reorder.selected_over.join(", "));
    base
}

fn render_skips(skipped: &[SkippedBackend]) -> String {
    skipped
        .iter()
        .map(|skip| {
            let mut summary = format!(
                "{}: {}",
                candidate_label(&skip.backend, skip.model.as_deref()),
                skip.reason
            );
            if let Some(until) = &skip.unavailable_until {
                summary.push_str(" until ");
                summary.push_str(until);
            }
            summary
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn pick_route_candidate<F>(
    candidates: Vec<RouteCandidate>,
    state_path: &Path,
    now: OffsetDateTime,
    backend_available: F,
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
            now,
            backend_available,
        )? {
            skipped.push(reason);
            continue;
        }
        return Ok((candidate, skipped));
    }
    Err(RouteError::NoEligibleBackend {
        preferred_backend: preferred.backend,
        preferred_model: preferred.model,
        earliest_reset: earliest_reset(&skipped),
        skipped,
    }
    .into())
}

fn skip_reason_for_candidate<F>(
    state_path: &Path,
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
    now: OffsetDateTime,
    backend_available: F,
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

    let decision = availability::availability_for(state_path, backend, model, quota_pool, now)?;
    if decision.eligible {
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

fn auto_candidates(
    routing: &RoutingPolicy,
    mode: &str,
    primary: &RouteCandidate,
) -> Vec<RouteCandidate> {
    let mut candidates = vec![primary.clone()];
    if mode == "review" {
        if let Some(weak_backend) = review_fallback_backend_name(routing) {
            let weak_model = review_fallback_model(routing)
                .map(str::to_string)
                .or_else(|| primary.model.clone());
            let quota_pool = routing.find_quota_pool(mode, weak_backend, weak_model.as_deref());
            candidates.push(RouteCandidate {
                backend: weak_backend.to_string(),
                model: weak_model,
                quota_pool,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
                original_order: candidates.len(),
            });
        }
    }
    extend_remaining_candidates(routing, &mut candidates, mode, primary.model.clone());
    dedupe_candidates(candidates)
}

fn explicit_candidates(
    routing: &RoutingPolicy,
    mode: &str,
    primary: &RouteCandidate,
    review_fallback_backend: Option<String>,
    allow_review_fallback: bool,
    allow_impl_fallback: bool,
) -> Vec<RouteCandidate> {
    let mut candidates = vec![primary.clone()];
    if mode == "review" && allow_review_fallback {
        if let Some(weak_backend) = review_fallback_backend {
            let weak_model = review_fallback_model(routing)
                .map(str::to_string)
                .or_else(|| primary.model.clone());
            let quota_pool = routing.find_quota_pool(mode, &weak_backend, weak_model.as_deref());
            candidates.push(RouteCandidate {
                backend: weak_backend,
                model: weak_model,
                quota_pool,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
                original_order: candidates.len(),
            });
        }
        extend_remaining_candidates(routing, &mut candidates, mode, primary.model.clone());
    } else if mode != "review" && allow_impl_fallback {
        extend_remaining_candidates(routing, &mut candidates, mode, primary.model.clone());
    }
    dedupe_candidates(candidates)
}

fn extend_remaining_candidates(
    routing: &RoutingPolicy,
    candidates: &mut Vec<RouteCandidate>,
    mode: &str,
    model: Option<String>,
) {
    for backend in mode_backend_preference(mode) {
        let quota_pool = routing.find_quota_pool(mode, backend, model.as_deref());
        candidates.push(RouteCandidate {
            backend: backend.to_string(),
            model: model.clone(),
            quota_pool,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            original_order: candidates.len(),
        });
    }
}

fn dedupe_candidates(candidates: Vec<RouteCandidate>) -> Vec<RouteCandidate> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let key = format!(
            "{}\u{0}{}",
            candidate.backend,
            candidate.model.as_deref().unwrap_or("")
        );
        if seen.insert(key) {
            out.push(candidate);
        }
    }
    out
}

/// TICKET-089 AC7/8: only a genuine agent-capability failure justifies
/// escalating toward a stronger (likely costlier) model. Harness/environment
/// failures, backend errors (which cover the AGY/Codex/Claude quota and auth
/// classifications from TICKET-102/107), and unknown/human-blocked states
/// must not -- a stronger model doesn't fix a broken auth token.
fn is_genuine_agent_failure(last_failure_class: Option<&str>) -> bool {
    matches!(
        last_failure_class,
        Some("agent_failure") | Some("agent_no_progress") | Some("validation_failure")
    )
}

fn order_candidates(
    profile: &Profile,
    candidates: Vec<RouteCandidate>,
    escalate: bool,
) -> (Vec<RouteCandidate>, Option<ReorderDecision>) {
    let mut candidates = with_original_order(candidates);
    if !escalate && !candidates.iter().any(RouteCandidate::has_cost_policy) {
        return (candidates, None);
    }

    let original = candidates.clone();
    candidates.sort_by(|left, right| compare_candidates(left, right, &profile.pacing, escalate));

    let Some(selected) = candidates.first() else {
        return (candidates, None);
    };
    let selected_over = original
        .iter()
        .take_while(|candidate| {
            candidate.backend != selected.backend || candidate.model != selected.model
        })
        .filter(|candidate| {
            compare_candidates(selected, candidate, &profile.pacing, escalate) == Ordering::Less
        })
        .map(|candidate| describe_candidate(candidate, &profile.pacing))
        .collect::<Vec<_>>();

    let reorder = if selected_over.is_empty() {
        None
    } else {
        Some(ReorderDecision {
            selected_over,
            escalated: escalate,
        })
    };
    (candidates, reorder)
}

fn is_strong_candidate(candidate: &RouteCandidate) -> bool {
    candidate
        .model
        .as_deref()
        .map(crate::ledger::is_strong_model)
        .unwrap_or(true)
}

fn compare_candidates(
    left: &RouteCandidate,
    right: &RouteCandidate,
    pacing: &crate::quota::PacingConfig,
    escalate: bool,
) -> Ordering {
    right
        .priority
        .cmp(&left.priority)
        .then_with(|| {
            if escalate {
                is_strong_candidate(right).cmp(&is_strong_candidate(left))
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| economic_rank(left, pacing).cmp(&economic_rank(right, pacing)))
        .then_with(|| compare_optional_f64(left.marginal_cost_usd, right.marginal_cost_usd))
        .then_with(|| left.original_order.cmp(&right.original_order))
}

fn economic_rank(candidate: &RouteCandidate, pacing: &crate::quota::PacingConfig) -> u8 {
    if !candidate.included_in_quota {
        return 1;
    }

    match quota::quota_pace(
        candidate.quota_usage_percent,
        candidate.quota_days_remaining,
        pacing,
    )
    .unwrap_or(PaceBand::HardConserve)
    {
        PaceBand::AggressiveBurn | PaceBand::MildBurn | PaceBand::Normal => 0,
        PaceBand::Conserve => 2,
        PaceBand::HardConserve => 3,
    }
}

fn compare_optional_f64(left: Option<f64>, right: Option<f64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn describe_candidate(candidate: &RouteCandidate, pacing: &crate::quota::PacingConfig) -> String {
    let mut details = Vec::new();
    if candidate.included_in_quota {
        details.push(
            match quota::quota_pace(
                candidate.quota_usage_percent,
                candidate.quota_days_remaining,
                pacing,
            )
            .unwrap_or(PaceBand::Normal)
            {
                PaceBand::AggressiveBurn => "included quota aggressive-burn".into(),
                PaceBand::MildBurn => "included quota mild-burn".into(),
                PaceBand::Normal => "included quota normal".into(),
                PaceBand::Conserve => "included quota conserve".into(),
                PaceBand::HardConserve => "included quota hard-conserve".into(),
            },
        );
    } else if let Some(cost) = candidate.marginal_cost_usd {
        details.push(format!("paid ${cost:.4}"));
    }
    if candidate.priority != 0 {
        details.push(format!("priority {}", candidate.priority));
    }
    let label = candidate_label(&candidate.backend, candidate.model.as_deref());
    if details.is_empty() {
        label
    } else {
        format!("{label} ({})", details.join(", "))
    }
}

fn build_routing_diagnostics(
    candidates: &[RouteCandidate],
    selected: &RouteCandidate,
    skipped: &[SkippedBackend],
    reorder: Option<&ReorderDecision>,
    pacing: &crate::quota::PacingConfig,
) -> RoutingDiagnostics {
    let candidates = candidates
        .iter()
        .enumerate()
        .map(|(consideration_order, candidate)| {
            let skipped = skipped
                .iter()
                .find(|skip| skip.backend == candidate.backend && skip.model == candidate.model);
            RoutingCandidateDiagnostic {
                backend: candidate.backend.clone(),
                model: candidate.model.clone(),
                quota_pool: candidate.quota_pool.clone(),
                default_order: Some(candidate.original_order),
                consideration_order: Some(consideration_order),
                pace_band: candidate_pace_band(candidate, pacing),
                cost_class: Some(candidate_cost_class(candidate)),
                skip_reason: skipped.map(|skip| skip.reason.clone()),
                unavailable_until: skipped.and_then(|skip| skip.unavailable_until.clone()),
            }
        })
        .collect();

    RoutingDiagnostics {
        policy_reordered_candidates: reorder.is_some(),
        selected_backend: Some(selected.backend.clone()),
        selected_model: selected.model.clone(),
        selected_quota_pool: selected.quota_pool.clone(),
        selected_pace_band: candidate_pace_band(selected, pacing),
        selected_cost_class: Some(candidate_cost_class(selected)),
        selected_over: reorder.map(|r| r.selected_over.clone()).unwrap_or_default(),
        human_summary: Some(render_routing_diagnostics_human(
            selected, skipped, reorder, pacing,
        )),
        candidates,
    }
}

fn render_routing_diagnostics_human(
    selected: &RouteCandidate,
    skipped: &[SkippedBackend],
    reorder: Option<&ReorderDecision>,
    pacing: &crate::quota::PacingConfig,
) -> String {
    let mut parts = vec![format!("selected {}", describe_candidate(selected, pacing))];
    if let Some(pool) = &selected.quota_pool {
        parts.push(format!("quota pool {}", pool));
    }
    if let Some(band) = candidate_pace_band(selected, pacing) {
        parts.push(format!("pace {}", band));
    }
    parts.push(format!("cost {}", candidate_cost_class(selected)));
    if let Some(reorder) = reorder.filter(|r| !r.selected_over.is_empty()) {
        parts.push(format!(
            "policy reordered defaults over {}",
            reorder.selected_over.join(", ")
        ));
    }
    if !skipped.is_empty() {
        parts.push(format!("skipped {}", render_skips(skipped)));
    }
    parts.join("; ")
}

fn candidate_pace_band(
    candidate: &RouteCandidate,
    pacing: &crate::quota::PacingConfig,
) -> Option<String> {
    if !candidate.included_in_quota {
        return None;
    }
    Some(
        match quota::quota_pace(
            candidate.quota_usage_percent,
            candidate.quota_days_remaining,
            pacing,
        )
        .unwrap_or(PaceBand::Normal)
        {
            PaceBand::AggressiveBurn => "aggressive_burn",
            PaceBand::MildBurn => "mild_burn",
            PaceBand::Normal => "normal",
            PaceBand::Conserve => "conserve",
            PaceBand::HardConserve => "hard_conserve",
        }
        .to_string(),
    )
}

fn candidate_cost_class(candidate: &RouteCandidate) -> String {
    if candidate.included_in_quota {
        "included_quota".into()
    } else if candidate.marginal_cost_usd.unwrap_or(0.0) > 0.0 {
        "paid".into()
    } else {
        "standard".into()
    }
}

fn with_original_order(candidates: Vec<RouteCandidate>) -> Vec<RouteCandidate> {
    candidates
        .into_iter()
        .enumerate()
        .map(|(idx, mut candidate)| {
            candidate.original_order = idx;
            candidate
        })
        .collect()
}

fn candidate_label(backend: &str, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{backend}/{model}"),
        None => backend.to_string(),
    }
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

fn policy_candidates(policy: &RoutingPolicy, mode: &str) -> Option<Vec<RouteCandidate>> {
    let raw = match mode {
        "pm" => policy.pm_candidates.as_ref(),
        "review" => policy.review_candidates.as_ref(),
        "improve" | "fix" | "experiment" => policy.improve_candidates.as_ref(),
        _ => None,
    };
    raw.map(|list| {
        list.iter()
            .enumerate()
            .map(|(idx, c)| RouteCandidate {
                backend: c.backend.clone(),
                model: c.model.clone(),
                quota_pool: c.quota_pool.clone(),
                priority: c.priority,
                included_in_quota: c.included_in_quota,
                marginal_cost_usd: c.marginal_cost_usd,
                quota_usage_percent: c.quota_usage_percent,
                quota_days_remaining: c.quota_days_remaining,
                original_order: idx,
            })
            .collect()
    })
}

impl RouteCandidate {
    fn has_cost_policy(&self) -> bool {
        self.priority != 0
            || self.included_in_quota
            || self.marginal_cost_usd.is_some()
            || self.quota_usage_percent.is_some()
            || self.quota_days_remaining.is_some()
    }
}

fn review_fallback_backend_name(routing: &RoutingPolicy) -> Option<&str> {
    routing.weak_review_backend.as_deref()
}

fn review_fallback_backend<F>(
    defaults: &Defaults,
    profile: &Profile,
    backend_available: F,
) -> Option<String>
where
    F: Fn(&str) -> bool + Copy,
{
    review_fallback_backend_name(&profile.effective_routing(defaults))
        .map(str::to_string)
        .or_else(|| any_available_backend("review", backend_available))
}

fn review_fallback_model(routing: &RoutingPolicy) -> Option<&str> {
    routing.weak_review_model.as_deref()
}

fn builtin_backend<F>(mode: &str, backend_available: F) -> String
where
    F: Fn(&str) -> bool + Copy,
{
    mode_backend_preference(mode)
        .into_iter()
        .find(|backend| backend_available(backend))
        .unwrap_or("openhands")
        .to_string()
}

fn any_available_backend<F>(mode: &str, backend_available: F) -> Option<String>
where
    F: Fn(&str) -> bool + Copy,
{
    mode_backend_preference(mode)
        .into_iter()
        .find(|backend| backend_available(backend))
        .map(str::to_string)
}

fn mode_backend_preference(mode: &str) -> [&'static str; 3] {
    match mode {
        "pm" | "review" => ["claude", "codex", "openhands"],
        _ => ["openhands", "codex", "claude"],
    }
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
    use super::{decide_with, is_genuine_agent_failure, RouteError, RouteRequest};
    use crate::availability::{Reason, Source};
    use crate::config::{Defaults, Profile, RoutingPolicy};

    #[allow(clippy::too_many_arguments)]
    fn record_unavailable(
        state_path: &std::path::Path,
        backend: &str,
        model: Option<&str>,
        reason: Reason,
        source: Source,
        unavailable_until: Option<OffsetDateTime>,
        last_error_summary: Option<String>,
        now: OffsetDateTime,
    ) -> anyhow::Result<()> {
        crate::availability::record_unavailable(
            state_path,
            backend,
            model,
            None,
            reason,
            source,
            unavailable_until,
            last_error_summary,
            now,
        )
    }

    fn record_available(
        state_path: &std::path::Path,
        backend: &str,
        model: Option<&str>,
        source: Source,
        now: OffsetDateTime,
    ) -> anyhow::Result<()> {
        crate::availability::record_available(state_path, backend, model, None, source, now)
    }
    use tempfile::TempDir;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    fn defaults() -> Defaults {
        Defaults {
            current_manager: None,
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
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
            prune_older_than_days: None,
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
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            vibe_args: vec![],
            vibe_path: None,
            opencode_args: vec![],
            opencode_path: None,
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds: None,
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            auto_fix_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            notify_command: None,
            routing: RoutingPolicy {
                pm_backend: Some("claude".into()),
                ..RoutingPolicy::default()
            },
            pacing: Default::default(),
            publishing: Default::default(),
        }
    }

    fn path(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join("availability.json")
    }

    fn backend_available(name: &str) -> bool {
        matches!(
            name,
            "claude" | "codex" | "openhands" | "agy" | "agy-main" | "agy-second"
        )
    }

    fn candidate_config(
        backend: &str,
        model: Option<&str>,
        quota_pool: Option<&str>,
    ) -> crate::config::CandidateConfig {
        crate::config::CandidateConfig {
            backend: backend.into(),
            model: model.map(str::to_string),
            quota_pool: quota_pool.map(str::to_string),
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
        }
    }

    #[test]
    fn profile_routing_beats_global_policy() {
        let tmp = TempDir::new().unwrap();
        let decision = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: Some("openhands"),
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();
        assert_eq!(decision.effective_backend, "claude");
        assert_eq!(decision.routing_reason, "profile routing policy");
    }

    #[test]
    fn profile_routing_can_select_agy() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.default_backend = Some("agy".into());
        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "improve",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "agy");
        assert_eq!(decision.routing_reason, "profile routing policy");
    }

    #[test]
    fn default_candidate_list_is_inherited_when_profile_only_overrides_other_fields() {
        let tmp = TempDir::new().unwrap();
        let mut defaults = defaults();
        defaults.routing.pm_candidates = Some(vec![
            candidate_config("codex", Some("gpt-5"), None),
            candidate_config("claude", Some("sonnet"), None),
        ]);
        let mut profile = profile();
        profile.routing.improve_backend = Some("agy".into());

        let decision = decide_with(
            &defaults,
            &profile,
            RouteRequest {
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
                last_failure_class: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-5"));
        assert_eq!(decision.routing_reason, "global routing policy");
    }

    #[test]
    fn codex_fallback_model_extracted_from_profile_codex_args() {
        let tmp = TempDir::new().unwrap();
        let defaults = defaults();
        let mut profile = profile();
        profile.codex_args = vec!["-m".to_string(), "gpt-5.4-mini".to_string()];

        let decision = decide_with(
            &defaults,
            &profile,
            RouteRequest {
                mode: "improve",
                requested_backend: "codex",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
                last_failure_class: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4-mini"));
    }

    #[test]
    fn codex_stale_args_do_not_override_resolved_model() {
        let tmp = TempDir::new().unwrap();
        let defaults = defaults();
        let mut profile = profile();
        profile.codex_args = vec!["-m".to_string(), "gpt-5.4-mini".to_string()];

        let decision = decide_with(
            &defaults,
            &profile,
            RouteRequest {
                mode: "improve",
                requested_backend: "codex",
                requested_model: Some("gpt-5.4"),
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
                last_failure_class: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn profile_scalar_override_preserves_inherited_default_model() {
        let tmp = TempDir::new().unwrap();
        let mut defaults = defaults();
        defaults.routing.improve_model = Some("gpt-5.4".into());
        let mut profile = profile();
        profile.routing.improve_backend = Some("agy".into());

        let decision = decide_with(
            &defaults,
            &profile,
            RouteRequest {
                mode: "improve",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
                last_failure_class: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "agy");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
        assert_eq!(decision.routing_reason, "profile routing policy");
    }

    #[test]
    fn preferred_backend_unavailable_falls_back() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &path(&tmp),
            "claude",
            None,
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::hours(1)),
            None,
            now,
        )
        .unwrap();

        let decision = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert!(decision.fallback_used);
        assert!(decision.routing_reason.contains("quota_exhausted"));
    }

    #[test]
    fn preferred_backend_available_keeps_normal_selection() {
        let tmp = TempDir::new().unwrap();
        let decision = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "claude");
        assert!(!decision.fallback_used);
    }

    #[test]
    fn expired_temporary_record_restores_eligibility() {
        let tmp = TempDir::new().unwrap();
        let observed = OffsetDateTime::now_utc() - time::Duration::hours(2);
        record_unavailable(
            &path(&tmp),
            "claude",
            None,
            Reason::RateLimited,
            Source::BackendError,
            Some(observed + time::Duration::minutes(30)),
            None,
            observed,
        )
        .unwrap();

        let decision = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "claude");
    }

    #[test]
    fn backend_wide_block_blocks_all_models() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &path(&tmp),
            "codex",
            None,
            Reason::ManualDisable,
            Source::Manual,
            None,
            None,
            now,
        )
        .unwrap();

        let decision = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "fix",
                requested_backend: "auto",
                requested_model: Some("gpt-5"),
                recommended_backend: Some("codex"),
                recommended_model: Some("gpt-5"),
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap();

        assert_ne!(decision.effective_backend, "codex");
        assert!(decision
            .routing_reason
            .contains("backend-wide manual_disable"));
    }

    #[test]
    fn model_specific_block_only_blocks_that_model() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &path(&tmp),
            "codex",
            Some("gpt-5"),
            Reason::RateLimited,
            Source::BackendError,
            Some(now + time::Duration::minutes(10)),
            None,
            now,
        )
        .unwrap();
        record_available(
            &path(&tmp),
            "codex",
            Some("gpt-5-mini"),
            Source::Manual,
            now,
        )
        .unwrap();

        let blocked = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "fix",
                requested_backend: "auto",
                requested_model: Some("gpt-5"),
                recommended_backend: Some("codex"),
                recommended_model: Some("gpt-5"),
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap();
        assert_ne!(blocked.effective_backend, "codex");

        let allowed = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "fix",
                requested_backend: "auto",
                requested_model: Some("gpt-5-mini"),
                recommended_backend: Some("codex"),
                recommended_model: Some("gpt-5-mini"),
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap();
        assert_eq!(allowed.effective_backend, "codex");
    }

    #[test]
    fn manual_disable_blocks_indefinitely() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &path(&tmp),
            "claude",
            None,
            Reason::ManualDisable,
            Source::Manual,
            None,
            Some("disabled".into()),
            now,
        )
        .unwrap();

        let decision = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now + time::Duration::days(30),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
    }

    #[test]
    fn all_candidates_unavailable_returns_earliest_reset() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();
        for (backend, mins) in [("claude", 30), ("codex", 10), ("openhands", 20)] {
            record_unavailable(
                &path(&tmp),
                backend,
                None,
                Reason::RateLimited,
                Source::BackendError,
                Some(now + time::Duration::minutes(mins)),
                None,
                now,
            )
            .unwrap();
        }

        let err = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap_err();

        let route_err = err.downcast_ref::<RouteError>().unwrap();
        match route_err {
            RouteError::NoEligibleBackend { earliest_reset, .. } => {
                let expected = (now + time::Duration::minutes(10))
                    .format(&Rfc3339)
                    .unwrap();
                assert_eq!(earliest_reset.as_deref(), Some(expected.as_str()));
            }
        }
    }

    #[test]
    fn fallback_route_records_availability_reason() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &path(&tmp),
            "claude",
            None,
            Reason::BackendOutage,
            Source::BackendError,
            Some(now + time::Duration::minutes(5)),
            None,
            now,
        )
        .unwrap();

        let decision = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "review",
                requested_backend: "claude",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert!(decision.routing_reason.contains("backend_outage"));
        assert!(decision.human_required);
        assert_eq!(decision.confidence_impact.as_deref(), Some("low"));
    }

    #[test]
    fn malformed_availability_state_surfaces_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(path(&tmp), "{ not json").unwrap();
        let err = decide_with(
            &defaults(),
            &profile(),
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap_err();

        assert!(format!("{:#}", err).contains("parsing availability state"));
    }

    #[test]
    fn candidate_list_honored_when_available() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            candidate_config("codex", Some("gpt-4"), None),
            candidate_config("claude", None, None),
        ]);

        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-4"));
        assert_eq!(decision.routing_reason, "profile routing policy");
        assert!(!decision.fallback_used);
    }

    #[test]
    fn candidate_list_skips_unavailable_candidates() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();

        record_unavailable(
            &path(&tmp),
            "codex",
            Some("gpt-4"),
            Reason::RateLimited,
            Source::BackendError,
            Some(now + time::Duration::minutes(10)),
            None,
            now,
        )
        .unwrap();

        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            candidate_config("codex", Some("gpt-4"), None),
            candidate_config("claude", None, None),
        ]);

        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "claude");
        assert_eq!(decision.effective_model, None);
        assert!(decision.fallback_used);
        assert!(decision
            .routing_reason
            .contains("codex/gpt-4: model-specific rate_limited"));
        let diagnostics = decision.routing_diagnostics.as_ref().unwrap();
        assert!(!diagnostics.policy_reordered_candidates);
        assert_eq!(diagnostics.candidates.len(), 2);
        assert_eq!(diagnostics.candidates[0].backend, "codex");
        assert_eq!(
            diagnostics.candidates[0].skip_reason.as_deref(),
            Some("model-specific rate_limited")
        );
        assert_eq!(diagnostics.candidates[1].backend, "claude");
        assert_eq!(diagnostics.selected_backend.as_deref(), Some("claude"));
    }

    #[test]
    fn candidate_list_expired_availability_re_enters() {
        let tmp = TempDir::new().unwrap();
        let observed = OffsetDateTime::now_utc() - time::Duration::hours(2);

        record_unavailable(
            &path(&tmp),
            "codex",
            Some("gpt-4"),
            Reason::RateLimited,
            Source::BackendError,
            Some(observed + time::Duration::minutes(30)),
            None,
            observed,
        )
        .unwrap();

        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            candidate_config("codex", Some("gpt-4"), None),
            candidate_config("claude", None, None),
        ]);

        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-4"));
        assert!(!decision.fallback_used);
    }

    #[test]
    fn candidate_list_exhausted_errors() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();

        for (backend, model) in [("codex", Some("gpt-4")), ("claude", None)] {
            record_unavailable(
                &path(&tmp),
                backend,
                model,
                Reason::RateLimited,
                Source::BackendError,
                Some(now + time::Duration::minutes(10)),
                None,
                now,
            )
            .unwrap();
        }

        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            candidate_config("codex", Some("gpt-4"), None),
            candidate_config("claude", None, None),
        ]);

        let err = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap_err();

        let route_err = err.downcast_ref::<RouteError>().unwrap();
        match route_err {
            RouteError::NoEligibleBackend {
                preferred_backend,
                preferred_model,
                skipped,
                earliest_reset,
            } => {
                assert_eq!(preferred_backend, "codex");
                assert_eq!(preferred_model.as_deref(), Some("gpt-4"));
                assert_eq!(skipped.len(), 2);
                assert_eq!(skipped[0].backend, "codex");
                assert_eq!(skipped[0].reason, "model-specific rate_limited");
                assert_eq!(skipped[1].backend, "claude");
                assert_eq!(skipped[1].reason, "backend-wide rate_limited");
                assert!(earliest_reset.is_some());
            }
        }
    }

    #[test]
    fn routing_honors_shared_quota_pool() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();

        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            candidate_config("claude", Some("claude-sonnet"), Some("claude-main")),
            candidate_config("claude", Some("claude-haiku"), Some("claude-main")),
            candidate_config("codex", Some("gpt-4"), Some("codex-main")),
        ]);

        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap();
        assert_eq!(decision.effective_backend, "claude");
        assert_eq!(decision.effective_model.as_deref(), Some("claude-sonnet"));
        assert_eq!(
            decision.effective_quota_pool.as_deref(),
            Some("claude-main")
        );

        crate::availability::record_unavailable(
            &path(&tmp),
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::minutes(10)),
            None,
            now,
        )
        .unwrap();

        let decision2 = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            now,
            backend_available,
        )
        .unwrap();
        assert_eq!(decision2.effective_backend, "codex");
        assert_eq!(decision2.effective_model.as_deref(), Some("gpt-4"));
        assert_eq!(
            decision2.effective_quota_pool.as_deref(),
            Some("codex-main")
        );
        assert!(decision2.fallback_used);
    }

    #[test]
    fn is_genuine_agent_failure_classifies_correctly() {
        // TICKET-089 AC7/8
        assert!(is_genuine_agent_failure(Some("agent_failure")));
        assert!(is_genuine_agent_failure(Some("agent_no_progress")));
        assert!(is_genuine_agent_failure(Some("validation_failure")));
        assert!(!is_genuine_agent_failure(Some("harness_error")));
        assert!(!is_genuine_agent_failure(Some("environment_error")));
        assert!(!is_genuine_agent_failure(Some("backend_error")));
        assert!(!is_genuine_agent_failure(Some("human_blocked")));
        assert!(!is_genuine_agent_failure(Some("unknown")));
        assert!(!is_genuine_agent_failure(None));
    }

    #[test]
    fn genuine_agent_failure_escalates_to_stronger_model() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            crate::config::CandidateConfig {
                backend: "openhands".into(),
                model: Some("deepseek-flash".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
            },
            crate::config::CandidateConfig {
                backend: "codex".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
            },
        ]);

        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: Some("validation_failure"),
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
        assert!(decision
            .routing_reason
            .contains("escalated to stronger model"));
    }

    #[test]
    fn non_agent_failure_does_not_escalate() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            crate::config::CandidateConfig {
                backend: "openhands".into(),
                model: Some("deepseek-flash".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
            },
            crate::config::CandidateConfig {
                backend: "codex".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: None,
                quota_usage_percent: None,
                quota_days_remaining: None,
            },
        ]);

        for failure in [None, Some("backend_error"), Some("harness_error")] {
            let decision = decide_with(
                &defaults(),
                &profile,
                RouteRequest {
                    last_failure_class: failure,
                    mode: "pm",
                    requested_backend: "auto",
                    requested_model: None,
                    recommended_backend: None,
                    recommended_model: None,
                    session_id: None,
                    usage_summary: None,
                },
                &path(&tmp),
                OffsetDateTime::now_utc(),
                backend_available,
            )
            .unwrap();

            assert_eq!(decision.effective_backend, "openhands");
            assert_eq!(decision.effective_model.as_deref(), Some("deepseek-flash"));
            assert!(!decision.routing_reason.contains("escalated"));
        }
    }

    #[test]
    fn cost_aware_ordering_prefers_underpace_included_quota() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            crate::config::CandidateConfig {
                backend: "openhands".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: Some(0.25),
                quota_usage_percent: None,
                quota_days_remaining: None,
            },
            crate::config::CandidateConfig {
                backend: "codex".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: Some("codex-main".into()),
                priority: 0,
                included_in_quota: true,
                marginal_cost_usd: Some(0.0),
                quota_usage_percent: Some(20.0),
                quota_days_remaining: Some(5.0),
            },
        ]);

        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
        assert!(!decision.fallback_used);
        assert!(decision.routing_reason.contains("cost-aware reorder"));
        assert!(decision.routing_reason.contains("openhands/gpt-5.4"));
        let diagnostics = decision.routing_diagnostics.as_ref().unwrap();
        assert!(diagnostics.policy_reordered_candidates);
        assert_eq!(
            diagnostics.selected_quota_pool.as_deref(),
            Some("codex-main")
        );
        assert_eq!(
            diagnostics.selected_pace_band.as_deref(),
            Some("aggressive_burn")
        );
        assert_eq!(
            diagnostics.selected_cost_class.as_deref(),
            Some("included_quota")
        );
        assert_eq!(diagnostics.selected_over.len(), 1);
        assert!(diagnostics
            .human_summary
            .as_deref()
            .unwrap()
            .contains("policy reordered defaults"));
    }

    #[test]
    fn cost_aware_ordering_conserves_scarce_included_quota() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            crate::config::CandidateConfig {
                backend: "codex".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: Some("codex-main".into()),
                priority: 0,
                included_in_quota: true,
                marginal_cost_usd: Some(0.0),
                quota_usage_percent: Some(85.0),
                quota_days_remaining: Some(5.0),
            },
            crate::config::CandidateConfig {
                backend: "openhands".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: Some(0.25),
                quota_usage_percent: None,
                quota_days_remaining: None,
            },
        ]);

        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "openhands");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
        assert!(!decision.fallback_used);
        assert!(decision.routing_reason.contains("codex/gpt-5.4"));
    }

    #[test]
    fn cost_aware_ordering_respects_explicit_priority_override() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.pm_candidates = Some(vec![
            crate::config::CandidateConfig {
                backend: "codex".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: Some("codex-main".into()),
                priority: 10,
                included_in_quota: true,
                marginal_cost_usd: Some(0.0),
                quota_usage_percent: Some(85.0),
                quota_days_remaining: Some(5.0),
            },
            crate::config::CandidateConfig {
                backend: "openhands".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: None,
                priority: 0,
                included_in_quota: false,
                marginal_cost_usd: Some(0.25),
                quota_usage_percent: None,
                quota_days_remaining: None,
            },
        ]);

        let decision = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "pm",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("gpt-5.4"));
        assert!(!decision.routing_reason.contains("cost-aware reorder"));
    }
}
