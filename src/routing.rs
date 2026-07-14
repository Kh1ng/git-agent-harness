use crate::availability::{self, AvailabilityDecision, BlockScope};
use crate::config::{Defaults, Profile, RoutingPolicy, TaskRoutingRule};
use crate::ledger::BackendUsageSummary;
use crate::quota::{self, PaceBand};
use crate::runner;
use anyhow::Result;
use fs2::FileExt;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

mod diagnostics;
mod types;

// Re-export stable types
pub use types::{
    CandidateIdentity, RouteDecision, RouteError, RouteRequest, RoutingRuntimeState,
    SkippedBackend, TaskRoutingContext,
};

// Import helper functions for internal use
use diagnostics::{build_routing_diagnostics, describe_candidate, DiagnosticCandidate};
use types::render_skips;

fn concurrency_key(backend: &str, model: Option<&str>) -> String {
    format!("{backend}/{}", model.unwrap_or(""))
}

fn concurrency_counters() -> &'static Mutex<HashMap<String, u32>> {
    static COUNTERS: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Current number of in-flight dispatches this process has running against a
/// given backend+model pair. Backed by the same counter `ConcurrencyGuard`
/// increments/decrements. Process-wide, not persisted, not cross-process --
/// see `Profile::max_concurrent_per_model` for why that's sufficient.
pub fn current_concurrent(backend: &str, model: Option<&str>) -> u32 {
    let key = concurrency_key(backend, model);
    *concurrency_counters()
        .lock()
        .unwrap()
        .get(&key)
        .unwrap_or(&0)
}

/// RAII marker for one in-flight dispatch against a backend+model pair.
/// Acquire right before the backend call starts; drop releases it -- on
/// success, error, or panic unwind, since it's a plain `Drop` impl rather
/// than a manually-called release on specific exit paths (mirrors the intent
/// of `work_claim::release_work`'s success/error coverage, just via RAII).
pub struct ConcurrencyGuard {
    key: String,
    shared_file: Option<File>,
}

impl ConcurrencyGuard {
    pub fn acquire(backend: &str, model: Option<&str>) -> Self {
        let key = concurrency_key(backend, model);
        *concurrency_counters()
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_insert(0) += 1;
        ConcurrencyGuard {
            key,
            shared_file: None,
        }
    }

    /// Reserve one configured backend/model slot across processes. A flock is
    /// released by the kernel if the worker dies, so a crashed actor cannot
    /// permanently consume quota capacity. This intentionally uses the
    /// smallest safe primitive: one lock file per slot, with a bounded slot
    /// count read from profile policy.
    pub fn acquire_shared(backend: &str, model: Option<&str>, cap: Option<u32>) -> Result<Self> {
        let Some(cap) = cap else {
            return Ok(Self::acquire(backend, model));
        };
        let cap = cap.max(1);
        let key = concurrency_key(backend, model);
        let root = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("gah")
            .join("concurrency");
        std::fs::create_dir_all(&root)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        key.hash(&mut hasher);
        let stem = format!("{:x}", hasher.finish());

        loop {
            for slot in 0..cap {
                let path = root.join(format!("{stem}-{slot}.lock"));
                let file = OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .truncate(false)
                    .open(path)?;
                match file.try_lock_exclusive() {
                    Ok(()) => {
                        let mut guard = Self::acquire(backend, model);
                        guard.shared_file = Some(file);
                        return Ok(guard);
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {}
                    Err(err) => return Err(err.into()),
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for ConcurrencyGuard {
    fn drop(&mut self) {
        if let Some(file) = self.shared_file.take() {
            let _ = file.unlock();
        }
        let mut counters = concurrency_counters().lock().unwrap();
        if let Some(count) = counters.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counters.remove(&self.key);
            }
        }
    }
}

/// Skip a candidate already at its configured `max_concurrent_per_model`
/// cap. `None` when the pair has no configured cap (unlimited, the default)
/// or is still under it.
fn max_concurrent_skip(
    max_concurrent: &HashMap<String, u32>,
    backend: &str,
    model: Option<&str>,
) -> Option<SkippedBackend> {
    let cap = *max_concurrent.get(&concurrency_key(backend, model))?;
    if current_concurrent(backend, model) >= cap {
        Some(SkippedBackend {
            backend: backend.to_string(),
            model: model.map(str::to_string),
            reason: "max_concurrent_reached".into(),
            unavailable_until: None,
        })
    } else {
        None
    }
}
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
    requires_approval: bool,
    original_order: usize,
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
struct ReorderDecision {
    selected_over: Vec<String>,
    escalated: bool,
}

#[derive(Clone, Copy)]
struct RouteEvaluation<'a> {
    state_path: &'a Path,
    now: OffsetDateTime,
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

fn decide_with_runtime<F>(
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
fn decide_with_task<F>(
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

fn decide_with_task_runtime<F>(
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
                requires_approval: false,
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
        // An explicit review request is still a request for a *reviewer*, not
        // permission to fall through to an arbitrary implementation backend.
        // When the requested reviewer belongs to the declared review pool,
        // preserve the remainder of that ordered pool.  This matters for an
        // escalated review: Claude -> GLM must not fall back to AGY again and
        // silently lose the intended second opinion.
        let configured_remainder = routing.review_candidates.as_ref().and_then(|configured| {
            configured
                .iter()
                .position(|candidate| {
                    candidate.backend == primary.backend
                        && (candidate.model.is_none()
                            || primary.model.is_none()
                            || candidate.model == primary.model)
                })
                .map(|position| route_candidates(&configured[position + 1..]))
        });
        if let Some(remainder) = configured_remainder {
            candidates.extend(remainder);
        } else if let Some(weak_backend) = review_fallback_backend {
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
                requires_approval: false,
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
    raw.map(|list| route_candidates(list))
}

fn task_rule_candidates(
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
    routing
        .escalatory_reviewers
        .first()
        .and_then(|c| c.model.as_deref())
        .or(routing.weak_review_model.as_deref())
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
    use super::{
        decide_with, decide_with_runtime, decide_with_task, decide_with_task_runtime,
        is_genuine_agent_failure, CandidateIdentity, ConcurrencyGuard, RouteError, RouteEvaluation,
        RouteRequest, RoutingRuntimeState, TaskRoutingContext,
    };
    use crate::availability::{Reason, Source};
    use crate::config::{Defaults, Profile, RoutingPolicy, TaskRoutingRule};
    use std::sync::Mutex;

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

    // These tests deliberately mutate the process-global live-slot counter.
    // Keep them out of parallel test execution so one test cannot make the
    // other's post-release assertion observe a still-held Claude slot.
    static CONCURRENCY_TEST_LOCK: Mutex<()> = Mutex::new(());

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
            opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
            max_concurrent_per_model: std::collections::HashMap::new(),
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
            "claude" | "codex" | "openhands" | "agy" | "agy-main" | "agy-second" | "opencode"
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
            requires_approval: false,
        }
    }

    fn implementation_request() -> RouteRequest<'static> {
        RouteRequest {
            last_failure_class: None,
            mode: "improve",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: Some("codex"),
            recommended_model: Some("strong"),
            session_id: None,
            usage_summary: None,
        }
    }

    fn easy_docs_rule(candidates: Vec<crate::config::CandidateConfig>) -> TaskRoutingRule {
        TaskRoutingRule {
            modes: vec!["improve".into(), "fix".into()],
            task_classes: vec!["documentation".into()],
            difficulties: vec!["easy".into()],
            risks: vec!["low".into()],
            candidates,
        }
    }

    #[test]
    fn task_rule_precedes_generic_candidates_for_matching_implementation() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.improve_candidates =
            Some(vec![candidate_config("codex", Some("strong"), None)]);
        profile.routing.task_routing_rules = vec![easy_docs_rule(vec![candidate_config(
            "agy",
            Some("cheap"),
            None,
        )])];

        let decision = decide_with_task(
            &defaults(),
            &profile,
            implementation_request(),
            Some(TaskRoutingContext {
                task_class: Some("Documentation"),
                difficulty: Some("EASY"),
                risk: Some("low"),
            }),
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "agy");
        assert_eq!(decision.effective_model.as_deref(), Some("cheap"));
        assert_eq!(decision.routing_reason, "task routing rule #1");
    }

    #[test]
    fn task_rule_falls_through_when_its_first_candidate_is_unavailable() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.task_routing_rules = vec![easy_docs_rule(vec![
            candidate_config("agy", Some("cheap"), None),
            candidate_config("codex", Some("fallback"), None),
        ])];
        record_unavailable(
            &path(&tmp),
            "agy",
            Some("cheap"),
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();

        let decision = decide_with_task(
            &defaults(),
            &profile,
            implementation_request(),
            Some(TaskRoutingContext {
                task_class: Some("documentation"),
                difficulty: Some("easy"),
                risk: Some("low"),
            }),
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert!(decision.fallback_used);
        assert!(decision.routing_reason.contains("quota_exhausted"));
    }

    #[test]
    fn equal_priority_task_pool_selects_least_used_candidate() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        let mut first = candidate_config("opencode", Some("hy3"), None);
        first.priority = 100;
        first.included_in_quota = true;
        let mut second = candidate_config("codex", Some("spark"), None);
        second.priority = 100;
        second.included_in_quota = true;
        profile.routing.task_routing_rules = vec![easy_docs_rule(vec![first, second])];
        let mut runtime = RoutingRuntimeState::default();
        runtime
            .recent_runs
            .insert(CandidateIdentity::new("opencode", Some("hy3")), 4);
        runtime
            .recent_runs
            .insert(CandidateIdentity::new("codex", Some("spark")), 1);

        let decision = decide_with_task_runtime(
            &defaults(),
            &profile,
            implementation_request(),
            Some(TaskRoutingContext {
                task_class: Some("documentation"),
                difficulty: Some("easy"),
                risk: Some("low"),
            }),
            &runtime,
            RouteEvaluation {
                state_path: &path(&tmp),
                now: OffsetDateTime::now_utc(),
            },
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("spark"));
        assert!(
            decision
                .routing_diagnostics
                .unwrap()
                .policy_reordered_candidates
        );
    }

    #[test]
    fn capability_escalation_excludes_previously_attempted_candidate() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        let mut first = candidate_config("opencode", Some("hy3"), None);
        first.priority = 100;
        let mut second = candidate_config("codex", Some("spark"), None);
        second.priority = 90;
        profile.routing.task_routing_rules = vec![easy_docs_rule(vec![first, second])];
        let mut runtime = RoutingRuntimeState::default();
        runtime
            .attempted
            .insert(CandidateIdentity::new("opencode", Some("hy3")));
        let mut request = implementation_request();
        request.last_failure_class = Some("agent_no_progress");

        let decision = decide_with_task_runtime(
            &defaults(),
            &profile,
            request,
            Some(TaskRoutingContext {
                task_class: Some("documentation"),
                difficulty: Some("easy"),
                risk: Some("low"),
            }),
            &runtime,
            RouteEvaluation {
                state_path: &path(&tmp),
                now: OffsetDateTime::now_utc(),
            },
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("spark"));
        assert_eq!(
            decision.routing_diagnostics.unwrap().candidates[0]
                .skip_reason
                .as_deref(),
            Some("already_attempted_after_capability_failure")
        );
    }

    #[test]
    fn capability_escalation_remains_sticky_after_a_later_infrastructure_failure() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        let mut first = candidate_config("opencode", Some("hy3"), None);
        first.priority = 100;
        let mut second = candidate_config("codex", Some("spark"), None);
        second.priority = 90;
        profile.routing.task_routing_rules = vec![easy_docs_rule(vec![first, second])];
        let mut runtime = RoutingRuntimeState::default();
        runtime
            .attempted
            .insert(CandidateIdentity::new("opencode", Some("hy3")));
        let mut request = implementation_request();
        request.last_failure_class = Some("backend_error");

        let decision = decide_with_task_runtime(
            &defaults(),
            &profile,
            request,
            Some(TaskRoutingContext {
                task_class: Some("documentation"),
                difficulty: Some("easy"),
                risk: Some("low"),
            }),
            &runtime,
            RouteEvaluation {
                state_path: &path(&tmp),
                now: OffsetDateTime::now_utc(),
            },
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "codex");
        assert_eq!(decision.effective_model.as_deref(), Some("spark"));
        assert_eq!(
            decision.routing_diagnostics.unwrap().candidates[0]
                .skip_reason
                .as_deref(),
            Some("already_attempted_after_capability_failure")
        );
    }

    #[test]
    fn paid_candidate_requires_exact_operator_approval() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        let mut paid = candidate_config("opencode", Some("openai/gpt-paid"), None);
        paid.priority = 10;
        paid.requires_approval = true;
        profile.routing.task_routing_rules = vec![easy_docs_rule(vec![paid])];
        let task = Some(TaskRoutingContext {
            task_class: Some("documentation"),
            difficulty: Some("easy"),
            risk: Some("low"),
        });

        let err = decide_with_task_runtime(
            &defaults(),
            &profile,
            implementation_request(),
            task,
            &RoutingRuntimeState::default(),
            RouteEvaluation {
                state_path: &path(&tmp),
                now: OffsetDateTime::now_utc(),
            },
            backend_available,
        )
        .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<RouteError>(),
            Some(RouteError::ApprovalRequired { backend, model, .. })
                if backend == "opencode" && model.as_deref() == Some("openai/gpt-paid")
        ));

        let mut approved = RoutingRuntimeState::default();
        approved
            .approved
            .insert(CandidateIdentity::new("opencode", Some("openai/gpt-paid")));
        let decision = decide_with_task_runtime(
            &defaults(),
            &profile,
            implementation_request(),
            task,
            &approved,
            RouteEvaluation {
                state_path: &path(&tmp),
                now: OffsetDateTime::now_utc(),
            },
            backend_available,
        )
        .unwrap();
        assert_eq!(decision.effective_model.as_deref(), Some("openai/gpt-paid"));
        assert_eq!(
            decision
                .routing_diagnostics
                .unwrap()
                .selected_cost_class
                .as_deref(),
            Some("paid")
        );
    }

    #[test]
    fn load_balancing_does_not_reorder_review_candidates() {
        // recent_runs is populated from review/pm history too (routing_runtime_state
        // in src/dispatch.rs deliberately tracks all agent-execution modes for
        // attribution), but the balancing TIE-BREAK must stay scoped to
        // implementation dispatch. Review's configured order is a
        // deliberate escalation chain (see explicit_candidates' "Claude ->
        // GLM must not fall back to AGY again" invariant elsewhere in this
        // file) and must not be silently reshuffled by usage counts.
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        let mut first = candidate_config("claude", Some("sonnet"), None);
        first.priority = 100;
        let mut second = candidate_config("opencode", Some("glm"), None);
        second.priority = 100;
        profile.routing.review_candidates = Some(vec![first, second]);
        let mut runtime = RoutingRuntimeState::default();
        // The configured-first candidate is far more heavily used -- if
        // load-balancing applied here, it would be passed over.
        runtime
            .recent_runs
            .insert(CandidateIdentity::new("claude", Some("sonnet")), 10);
        runtime
            .recent_runs
            .insert(CandidateIdentity::new("opencode", Some("glm")), 0);

        let decision = decide_with_runtime(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "review",
                requested_backend: "auto",
                requested_model: None,
                recommended_backend: None,
                recommended_model: None,
                session_id: None,
                usage_summary: None,
            },
            &runtime,
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();

        assert_eq!(decision.effective_backend, "claude");
        assert_eq!(decision.effective_model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn explicit_cli_model_override_cannot_bypass_approval_gate() {
        // requested_backend defaults to "auto" on every real dispatch (see
        // main.rs's --backend default), so an operator running
        // `gah dispatch --model <paid-model>` without an explicit --backend
        // must not be able to silently reach an unapproved requires_approval
        // candidate just because the model string matches.
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        let mut free = candidate_config("opencode", Some("hy3-free"), None);
        free.priority = 100;
        let mut paid = candidate_config("opencode", Some("openai/gpt-paid"), None);
        paid.priority = 10;
        paid.requires_approval = true;
        profile.routing.task_routing_rules = vec![easy_docs_rule(vec![free, paid])];
        let task = Some(TaskRoutingContext {
            task_class: Some("documentation"),
            difficulty: Some("easy"),
            risk: Some("low"),
        });
        let mut request = implementation_request();
        request.requested_model = Some("openai/gpt-paid");

        // Baseline: without an override, routing naturally picks the free
        // candidate -- confirms the override target genuinely differs from
        // what would have been selected, so this test exercises the
        // override path at all.
        let unoverridden = decide_with_task_runtime(
            &defaults(),
            &profile,
            implementation_request(),
            task,
            &RoutingRuntimeState::default(),
            RouteEvaluation {
                state_path: &path(&tmp),
                now: OffsetDateTime::now_utc(),
            },
            backend_available,
        )
        .unwrap();
        assert_eq!(unoverridden.effective_model.as_deref(), Some("hy3-free"));

        let err = decide_with_task_runtime(
            &defaults(),
            &profile,
            request.clone(),
            task,
            &RoutingRuntimeState::default(),
            RouteEvaluation {
                state_path: &path(&tmp),
                now: OffsetDateTime::now_utc(),
            },
            backend_available,
        )
        .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<RouteError>(),
            Some(RouteError::ApprovalRequired { backend, model, .. })
                if backend == "opencode" && model.as_deref() == Some("openai/gpt-paid")
        ));

        // Once approved, the exact same override succeeds.
        let mut approved = RoutingRuntimeState::default();
        approved
            .approved
            .insert(CandidateIdentity::new("opencode", Some("openai/gpt-paid")));
        let decision = decide_with_task_runtime(
            &defaults(),
            &profile,
            request,
            task,
            &approved,
            RouteEvaluation {
                state_path: &path(&tmp),
                now: OffsetDateTime::now_utc(),
            },
            backend_available,
        )
        .unwrap();
        assert_eq!(decision.effective_model.as_deref(), Some("openai/gpt-paid"));
    }

    #[test]
    fn missing_or_unmatched_task_metadata_preserves_generic_routing() {
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile.routing.improve_candidates =
            Some(vec![candidate_config("codex", Some("strong"), None)]);
        profile.routing.task_routing_rules = vec![easy_docs_rule(vec![candidate_config(
            "agy",
            Some("cheap"),
            None,
        )])];

        let missing = decide_with_task(
            &defaults(),
            &profile,
            implementation_request(),
            Some(TaskRoutingContext::default()),
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();
        assert_eq!(missing.effective_backend, "codex");

        let review = decide_with_task(
            &defaults(),
            &profile,
            RouteRequest {
                mode: "review",
                ..implementation_request()
            },
            Some(TaskRoutingContext {
                task_class: Some("documentation"),
                difficulty: Some("easy"),
                risk: Some("low"),
            }),
            &path(&tmp),
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();
        assert_ne!(review.routing_reason, "task routing rule #1");
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
    fn auto_backend_honors_cli_model_override_in_effective_identity() {
        let tmp = TempDir::new().unwrap();
        let defaults = defaults();
        let profile = profile();

        let decision = decide_with(
            &defaults,
            &profile,
            RouteRequest {
                mode: "improve",
                requested_backend: "auto",
                requested_model: Some("custom/test-model"),
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

        assert_eq!(
            decision.effective_model.as_deref(),
            Some("custom/test-model")
        );
        let diagnostics = decision.routing_diagnostics.unwrap();
        assert_eq!(
            diagnostics.selected_model.as_deref(),
            Some("custom/test-model")
        );
        assert!(diagnostics
            .human_summary
            .unwrap()
            .contains("explicit CLI model override"));
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
    fn preferred_backend_at_max_concurrent_falls_back() {
        let _lock = CONCURRENCY_TEST_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile
            .max_concurrent_per_model
            .insert("claude/".to_string(), 1);
        let _slot = ConcurrencyGuard::acquire("claude", None);

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
        assert!(decision.fallback_used);
        assert!(decision.routing_reason.contains("max_concurrent_reached"));
    }

    /// TICKET/issue (2026-07-11 hy3-free incident): reproduces the real bug
    /// with real OS threads and the actual process-wide counter -- one
    /// thread holds the only slot for a backend/model capped at
    /// `max_concurrent=1` (standing in for an in-flight dispatch already
    /// running against it), and a route decision made concurrently on a
    /// second thread must skip that candidate and fall through to the next
    /// one, exactly like the existing quota_exhausted/backend_outage skip
    /// mechanics. Once the slot is released, routing picks the capped
    /// backend again.
    #[test]
    fn concurrent_dispatch_holding_slot_forces_other_thread_to_fall_back() {
        let _lock = CONCURRENCY_TEST_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let mut profile = profile();
        profile
            .max_concurrent_per_model
            .insert("claude/".to_string(), 1);
        let state_path = path(&tmp);

        let (holder_ready_tx, holder_ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _slot = ConcurrencyGuard::acquire("claude", None);
            holder_ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        holder_ready_rx.recv().unwrap();

        let profile_for_decider = profile.clone();
        let state_path_for_decider = state_path.clone();
        let decider = std::thread::spawn(move || {
            decide_with(
                &defaults(),
                &profile_for_decider,
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
                &state_path_for_decider,
                OffsetDateTime::now_utc(),
                backend_available,
            )
        });
        let decision_while_held = decider.join().unwrap().unwrap();
        release_tx.send(()).unwrap();
        holder.join().unwrap();

        assert_eq!(decision_while_held.effective_backend, "codex");
        assert!(decision_while_held
            .routing_reason
            .contains("max_concurrent_reached"));

        // Slot released -- the capped backend is eligible again.
        let decision_after_release = decide_with(
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
            &state_path,
            OffsetDateTime::now_utc(),
            backend_available,
        )
        .unwrap();
        assert_eq!(decision_after_release.effective_backend, "claude");
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
            other => panic!("expected no eligible backend, got {other:?}"),
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
    fn explicit_review_fallback_preserves_the_remaining_review_order() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();
        let mut profile = profile();
        profile.routing.review_candidates = Some(vec![
            crate::config::CandidateConfig {
                backend: "agy".into(),
                model: Some("sonnet".into()),
                ..Default::default()
            },
            crate::config::CandidateConfig {
                backend: "agy-second".into(),
                model: Some("sonnet".into()),
                ..Default::default()
            },
            crate::config::CandidateConfig {
                backend: "claude".into(),
                model: Some("sonnet-5".into()),
                ..Default::default()
            },
            crate::config::CandidateConfig {
                backend: "opencode".into(),
                model: Some("nous-portal/z-ai/glm-5.2".into()),
                ..Default::default()
            },
        ]);
        record_unavailable(
            &path(&tmp),
            "agy",
            Some("sonnet"),
            Reason::BackendOutage,
            Source::BackendError,
            None,
            None,
            now,
        )
        .unwrap();
        let via_agy = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "review",
                requested_backend: "agy",
                requested_model: Some("sonnet"),
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
        assert_eq!(via_agy.effective_backend, "agy-second");

        record_unavailable(
            &path(&tmp),
            "claude",
            Some("sonnet-5"),
            Reason::BackendOutage,
            Source::BackendError,
            None,
            None,
            now,
        )
        .unwrap();
        let via_claude = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "review",
                requested_backend: "claude",
                requested_model: Some("sonnet-5"),
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
        assert_eq!(via_claude.effective_backend, "opencode");
        assert_eq!(
            via_claude.effective_model.as_deref(),
            Some("nous-portal/z-ai/glm-5.2")
        );
    }

    #[test]
    fn explicit_review_fallback_preserves_order_when_request_omits_model() {
        let tmp = TempDir::new().unwrap();
        let now = OffsetDateTime::now_utc();
        let mut profile = profile();
        profile.routing.review_candidates = Some(vec![
            crate::config::CandidateConfig {
                backend: "agy".into(),
                model: Some("sonnet".into()),
                ..Default::default()
            },
            crate::config::CandidateConfig {
                backend: "claude".into(),
                model: Some("sonnet-5".into()),
                ..Default::default()
            },
        ]);
        record_unavailable(
            &path(&tmp),
            "agy",
            None,
            Reason::BackendOutage,
            Source::BackendError,
            None,
            None,
            now,
        )
        .unwrap();
        // A manual/escalated review request that names the backend but not a
        // model must still locate its position in the configured pool and
        // preserve the remainder, not fall through to weak_review_backend.
        let via_agy = decide_with(
            &defaults(),
            &profile,
            RouteRequest {
                last_failure_class: None,
                mode: "review",
                requested_backend: "agy",
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
        assert_eq!(via_agy.effective_backend, "claude");
        assert_eq!(via_agy.effective_model.as_deref(), Some("sonnet-5"));
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
            other => panic!("expected no eligible backend, got {other:?}"),
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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: false,
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
                requires_approval: false,
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
