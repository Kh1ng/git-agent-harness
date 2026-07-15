//! Candidate construction and deterministic routing-policy ordering.

use super::diagnostics::{describe_candidate, DiagnosticCandidate};
use super::types::{CandidateIdentity, RoutingRuntimeState, TaskRoutingContext};
use crate::config::{Defaults, Profile, RoutingPolicy, TaskRoutingRule};
use crate::quota::{self, PaceBand};
use std::cmp::Ordering;
use std::collections::HashSet;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub(super) struct RouteCandidate {
    pub(super) backend: String,
    pub(super) model: Option<String>,
    pub(super) quota_pool: Option<String>,
    pub(super) priority: i32,
    pub(super) included_in_quota: bool,
    pub(super) marginal_cost_usd: Option<f64>,
    pub(super) quota_usage_percent: Option<f64>,
    pub(super) quota_days_remaining: Option<f64>,
    pub(super) requires_approval: bool,
    pub(super) original_order: usize,
}

impl DiagnosticCandidate for RouteCandidate {
    fn backend(&self) -> &str {
        &self.backend
    }

    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn quota_pool(&self) -> Option<&str> {
        self.quota_pool.as_deref()
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn included_in_quota(&self) -> bool {
        self.included_in_quota
    }

    fn marginal_cost_usd(&self) -> Option<f64> {
        self.marginal_cost_usd
    }

    fn quota_usage_percent(&self) -> Option<f64> {
        self.quota_usage_percent
    }

    fn quota_days_remaining(&self) -> Option<f64> {
        self.quota_days_remaining
    }

    fn requires_approval(&self) -> bool {
        self.requires_approval
    }

    fn original_order(&self) -> usize {
        self.original_order
    }
}

#[derive(Debug, Clone)]
pub(super) struct ReorderDecision {
    pub(super) selected_over: Vec<String>,
    pub(super) escalated: bool,
}

pub(super) fn append_reorder_reason(
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

pub(super) fn auto_candidates(
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
                requires_approval: false,
                original_order: candidates.len(),
            });
        }
    }
    extend_default_backend_candidates(routing, &mut candidates, mode);
    dedupe_candidates(candidates)
}

pub(super) fn explicit_candidates(
    routing: &RoutingPolicy,
    mode: &str,
    primary: &RouteCandidate,
    review_fallback_backend: Option<String>,
    allow_review_fallback: bool,
    allow_impl_fallback: bool,
) -> Vec<RouteCandidate> {
    let mut candidates = vec![primary.clone()];
    let fallback_allowed = if mode == "review" {
        allow_review_fallback
    } else {
        allow_impl_fallback
    };
    if fallback_allowed {
        // Candidate identity is a backend/model pair. If the explicit route
        // belongs to the configured pool, continue with only the remainder of
        // that ordered pool. For an ad-hoc explicit route, fall back to the
        // complete configured pool. Never copy the explicit model onto a
        // different runner: that was the source of codex aliases reaching
        // OpenHands/Claude and being reported as exit-0 no-progress.
        if let Some(configured) = policy_candidates(routing, mode) {
            if let Some(position) = configured.iter().position(|candidate| {
                candidate.backend == primary.backend && candidate.model == primary.model
            }) {
                candidates.extend(configured.into_iter().skip(position + 1));
            } else {
                candidates.extend(configured);
            }
        }

        if mode == "review" {
            if let Some(weak_backend) = review_fallback_backend {
                let weak_model = review_fallback_model(routing).map(str::to_string);
                let quota_pool =
                    routing.find_quota_pool(mode, &weak_backend, weak_model.as_deref());
                candidates.push(RouteCandidate {
                    backend: weak_backend,
                    model: weak_model,
                    quota_pool,
                    priority: 0,
                    included_in_quota: false,
                    marginal_cost_usd: None,
                    quota_usage_percent: None,
                    quota_days_remaining: None,
                    requires_approval: false,
                    original_order: candidates.len(),
                });
            }
        }
    }
    dedupe_candidates(candidates)
}

fn extend_default_backend_candidates(
    routing: &RoutingPolicy,
    candidates: &mut Vec<RouteCandidate>,
    mode: &str,
) {
    for backend in mode_backend_preference(mode) {
        let quota_pool = routing.find_quota_pool(mode, backend, None);
        candidates.push(RouteCandidate {
            backend: backend.to_string(),
            model: None,
            quota_pool,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
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
pub(super) fn is_genuine_agent_failure(last_failure_class: Option<&str>) -> bool {
    matches!(
        last_failure_class,
        Some("agent_failure") | Some("agent_no_progress") | Some("validation_failure")
    )
}

pub(super) fn order_candidates(
    profile: &Profile,
    candidates: Vec<RouteCandidate>,
    escalate: bool,
    runtime: &RoutingRuntimeState,
    mode: &str,
) -> (Vec<RouteCandidate>, Option<ReorderDecision>) {
    let mut candidates = with_original_order(candidates);
    if !escalate && !candidates.iter().any(RouteCandidate::has_cost_policy) {
        return (candidates, None);
    }

    let original = candidates.clone();
    candidates.sort_by(|left, right| {
        compare_candidates(left, right, &profile.pacing, escalate, runtime, mode)
    });

    let Some(selected) = candidates.first() else {
        return (candidates, None);
    };
    let selected_over = original
        .iter()
        .take_while(|candidate| {
            candidate.backend != selected.backend || candidate.model != selected.model
        })
        .filter(|candidate| {
            compare_candidates(
                selected,
                candidate,
                &profile.pacing,
                escalate,
                runtime,
                mode,
            ) == Ordering::Less
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

/// Load-balancing by recent execution count is scoped to implementation
/// dispatch (improve/fix/experiment) -- the only modes `routing_runtime_state`
/// (src/dispatch.rs) collects `recent_runs` for with balancing in mind.
/// Applying it to review/pm candidate ordering too would silently reorder
/// those pools by usage, contradicting the deterministic-order guarantee
/// review's own escalation chain relies on.
fn is_load_balanced_mode(mode: &str) -> bool {
    matches!(mode, "improve" | "fix" | "experiment")
}

fn compare_candidates(
    left: &RouteCandidate,
    right: &RouteCandidate,
    pacing: &crate::quota::PacingConfig,
    escalate: bool,
    runtime: &RoutingRuntimeState,
    mode: &str,
) -> Ordering {
    right
        .priority
        .cmp(&left.priority)
        .then_with(|| {
            if is_load_balanced_mode(mode) {
                candidate_run_count(left, runtime).cmp(&candidate_run_count(right, runtime))
            } else {
                Ordering::Equal
            }
        })
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

fn candidate_run_count(candidate: &RouteCandidate, runtime: &RoutingRuntimeState) -> u64 {
    runtime
        .recent_runs
        .get(&CandidateIdentity::new(
            candidate.backend.as_str(),
            candidate.model.as_deref(),
        ))
        .copied()
        .unwrap_or(0)
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

pub(super) fn policy_backend_model<'a>(
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

pub(super) fn policy_candidates(policy: &RoutingPolicy, mode: &str) -> Option<Vec<RouteCandidate>> {
    let raw = match mode {
        "pm" => policy.pm_candidates.as_ref(),
        "review" => policy.review_candidates.as_ref(),
        "improve" | "fix" | "experiment" => policy.improve_candidates.as_ref(),
        _ => None,
    };
    raw.map(|list| route_candidates(list))
}

/// Return true when this exact backend/model is configured as an
/// operator-approved-only route anywhere applicable to `mode`.
///
/// Internal escalation uses the explicit-route path. That path must retain
/// the same paid-route guard as normal auto routing instead of rebuilding the
/// candidate with `requires_approval = false`.
pub(super) fn configured_route_requires_approval(
    routing: &RoutingPolicy,
    mode: &str,
    backend: &str,
    model: Option<&str>,
) -> bool {
    let matches = |candidate: &crate::config::CandidateConfig| {
        candidate.backend == backend && candidate.model.as_deref() == model
    };

    let mode_candidates_require_approval = match mode {
        "pm" => routing.pm_candidates.as_ref(),
        "review" => routing.review_candidates.as_ref(),
        "improve" | "fix" | "experiment" => routing.improve_candidates.as_ref(),
        _ => None,
    }
    .is_some_and(|candidates| {
        candidates
            .iter()
            .any(|candidate| matches(candidate) && candidate.requires_approval)
    });

    mode_candidates_require_approval
        || (mode == "review"
            && routing
                .escalatory_reviewers
                .iter()
                .any(|candidate| matches(candidate) && candidate.requires_approval))
        || (matches!(mode, "improve" | "fix" | "experiment")
            && routing.task_routing_rules.iter().any(|rule| {
                (rule.modes.is_empty()
                    || rule
                        .modes
                        .iter()
                        .any(|configured_mode| configured_mode.eq_ignore_ascii_case(mode)))
                    && rule
                        .candidates
                        .iter()
                        .any(|candidate| matches(candidate) && candidate.requires_approval)
            }))
}

pub(super) fn task_rule_candidates(
    routing: &RoutingPolicy,
    mode: &str,
    task: Option<TaskRoutingContext<'_>>,
) -> Option<(usize, Vec<RouteCandidate>)> {
    if !matches!(mode, "improve" | "fix" | "experiment") {
        return None;
    }
    let task = task?;
    routing
        .task_routing_rules
        .iter()
        .enumerate()
        .find(|(_, rule)| task_rule_matches(rule, mode, task))
        .map(|(idx, rule)| (idx, route_candidates(&rule.candidates)))
}

fn task_rule_matches(rule: &TaskRoutingRule, mode: &str, task: TaskRoutingContext<'_>) -> bool {
    task_rule_dimension_matches(&rule.modes, Some(mode))
        && task_rule_dimension_matches(&rule.task_classes, task.task_class)
        && task_rule_dimension_matches(&rule.difficulties, task.difficulty)
        && task_rule_dimension_matches(&rule.risks, task.risk)
}

fn task_rule_dimension_matches(values: &[String], value: Option<&str>) -> bool {
    values.is_empty()
        || value.is_some_and(|value| values.iter().any(|item| item.eq_ignore_ascii_case(value)))
}

fn route_candidates(raw: &[crate::config::CandidateConfig]) -> Vec<RouteCandidate> {
    raw.iter()
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
            requires_approval: c.requires_approval,
            original_order: idx,
        })
        .collect()
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
    // Issue #123: prefer the new ESCALATORY_REVIEW list; fall back to the
    // deprecated single `weak_review_backend` so legacy configs keep working.
    routing
        .escalatory_reviewers
        .first()
        .map(|c| c.backend.as_str())
        .or(routing.weak_review_backend.as_deref())
}

pub(super) fn review_fallback_backend<F>(
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

pub(super) fn review_fallback_model(routing: &RoutingPolicy) -> Option<&str> {
    routing
        .escalatory_reviewers
        .first()
        .and_then(|c| c.model.as_deref())
        .or(routing.weak_review_model.as_deref())
}

pub(super) fn builtin_backend<F>(mode: &str, backend_available: F) -> String
where
    F: Fn(&str) -> bool + Copy,
{
    mode_backend_preference(mode)
        .into_iter()
        .find(|backend| backend_available(backend))
        .unwrap_or("openhands")
        .to_string()
}

pub(super) fn any_available_backend<F>(mode: &str, backend_available: F) -> Option<String>
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
