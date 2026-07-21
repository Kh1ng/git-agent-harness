use super::command::which;
use super::DispatchArgs;
use crate::config::{self, GahConfig, Profile};
use crate::controller::HumanRequiredReason;
use crate::ledger::{self, LedgerEntry};
use crate::models::WorkMetadata;
use crate::routing::{
    self, CandidateIdentity, RouteDecision, RouteError, RouteRequest, RoutingRuntimeState,
    TaskRoutingContext,
};
use crate::usage_attribution::{normalize_attempt_usage, UsageAttribution};
use crate::{runner, usage, worktree};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub(super) fn mark_shutdown_cancelled(
    ledger: &mut LedgerEntry,
    stage: crate::ledger::FailureStage,
    backend_exit_code: Option<i32>,
) {
    ledger.set_failure(crate::ledger::FailureClass::HarnessError, stage);
    ledger.backend_exit_code = backend_exit_code;
    ledger.validation_result = Some("cancelled_shutdown".into());
}
pub(super) fn resolve_llm(
    cfg: &GahConfig,
    args: &DispatchArgs,
    profile_oh: Option<&str>,
    effective_model: Option<&str>,
) -> Result<runner::LlmConfig> {
    // CLI flag wins, then profile config, then default
    let effective_oh_profile = args.oh_profile.as_deref().or(profile_oh);
    if let Some(name) = effective_oh_profile {
        let mut llm = runner::load_oh_profile(name)?;
        if let Some(m) = &args.model {
            llm.model = m.clone();
        }
        if let Some(m) = effective_model {
            llm.model = m.to_string();
        }
        if let Ok(v) = std::env::var("LLM_BASE_URL") {
            llm.base_url = v;
        }
        if let Ok(v) = std::env::var("LLM_API_KEY") {
            llm.api_key = v;
        }
        if let Ok(v) = std::env::var("LLM_MODEL") {
            llm.model = v;
        }
        return Ok(llm);
    }
    // --model flag always wins
    if let Some(m) = &args.model {
        return Ok(runner::LlmConfig {
            base_url: cfg.defaults.llm_base_url(),
            api_key: cfg.defaults.llm_api_key(),
            model: m.clone(),
        });
    }
    if let Some(m) = effective_model {
        return Ok(runner::LlmConfig {
            base_url: cfg.defaults.llm_base_url(),
            api_key: cfg.defaults.llm_api_key(),
            model: m.to_string(),
        });
    }
    // Check profile-level mode-specific override, then global default
    let profile_model =
        config::get_profile(cfg, &args.profile)
            .ok()
            .and_then(|p| match args.mode.as_str() {
                "improve" | "fix" => p.model_improve.clone(),
                "pm" => p.model_pm.clone(),
                "review" => p.model_review.clone(),
                _ => None,
            });
    let cloud = args.backend == "cloud-coder";
    Ok(runner::LlmConfig {
        base_url: cfg.defaults.llm_base_url(),
        api_key: cfg.defaults.llm_api_key(),
        model: profile_model.unwrap_or_else(|| cfg.defaults.llm_model(cloud)),
    })
}

pub(super) fn reserve_backend_slot(
    profile: &Profile,
    identity: &crate::execution_identity::ExecutionIdentity,
) -> Result<routing::ConcurrencyGuard> {
    let concurrency_cap = profile
        .max_concurrent_per_model
        .get(&format!(
            "{}/{}",
            identity.logical_backend,
            identity.effective_model.as_deref().unwrap_or("")
        ))
        .copied();
    routing::ConcurrencyGuard::acquire_shared_for_identity(
        identity,
        concurrency_cap,
        crate::runner::shutdown_requested,
    )
}

pub(super) fn apply_backend_instance_env(
    profile: &Profile,
    backend: &str,
    env_vars: &mut Vec<(String, String)>,
) {
    if backend == "agy-second" {
        if let Some(home) = profile.agy_second_home.as_deref().filter(|h| !h.is_empty()) {
            // Keep one authoritative HOME so both Command and usage/log
            // capture resolve the same backend instance. Several capture
            // helpers intentionally read the first matching environment key.
            env_vars.retain(|(key, _)| key != "HOME");
            env_vars.push(("HOME".to_string(), home.to_string()));
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_backend(
    backend: &str,
    profile: &Profile,
    wt: &Path,
    task: &str,
    session_dir: &Path,
    llm: &runner::LlmConfig,
    effective_model: Option<&str>,
    env_path: Option<&str>,
    hard_timeout_seconds: Option<u64>,
) -> Result<runner::RunResult> {
    run_backend_with_reserved_route(
        backend,
        profile,
        wt,
        task,
        session_dir,
        llm,
        effective_model,
        env_path,
        false,
        hard_timeout_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_backend_with_reserved_route(
    backend: &str,
    profile: &Profile,
    wt: &Path,
    task: &str,
    session_dir: &Path,
    llm: &runner::LlmConfig,
    effective_model: Option<&str>,
    env_path: Option<&str>,
    route_slot_already_reserved: bool,
    hard_timeout_seconds: Option<u64>,
) -> Result<runner::RunResult> {
    // Live incident (2026-07-11): concurrent dispatches landing on the same
    // shared free-tier backend+model (opencode/hy3-free) silently rate-limit.
    // Held for the duration of the actual backend call -- dropped on every
    // exit path (success, error, or panic) -- so routing's
    // `max_concurrent_per_model` check sees an accurate live count.
    let compatibility_identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        backend,
        effective_model,
        None::<String>,
    );
    let _concurrency_slot = (!route_slot_already_reserved)
        .then(|| reserve_backend_slot(profile, &compatibility_identity))
        .transpose()?;
    let origin_before = worktree::git(&["remote", "get-url", "origin"], wt).ok();
    let mut env_vars = env_path.map(runner::load_env_file).unwrap_or_default();
    // Every agent and any test command it launches inherit this repository-
    // scoped target directory. Cargo safely serializes concurrent builds in a
    // shared target dir, while separate worktree-local `target/` directories
    // otherwise multiply multi-gigabyte artifacts until the host fills.
    env_vars.push((
        "CARGO_TARGET_DIR".to_string(),
        crate::build_cache::target_dir(&profile.artifact_root, session_dir)
            .to_string_lossy()
            .into_owned(),
    ));
    apply_backend_instance_env(profile, backend, &mut env_vars);
    env_vars.retain(|(key, _)| key != crate::runner::process::HARD_TIMEOUT_ENV);
    if let Some(seconds) = hard_timeout_seconds.filter(|seconds| *seconds > 0) {
        env_vars.push((
            crate::runner::process::HARD_TIMEOUT_ENV.to_string(),
            seconds.to_string(),
        ));
    }
    let result = match backend {
        "codex" => runner::run_codex_with_executable(
            &runner::require_backend_executable(profile, backend)?,
            wt,
            task,
            session_dir,
            effective_model,
            &profile.codex_args,
            &env_vars,
            profile.codex_idle_timeout_seconds(),
        ),
        "claude" => runner::run_claude_with_executable(
            &runner::require_backend_executable(profile, backend)?,
            wt,
            task,
            session_dir,
            effective_model,
            &profile.claude_args,
            &env_vars,
            profile.claude_idle_timeout_seconds(),
        ),
        "agy" | "agy-main" | "agy-second" => runner::run_agy_with_executable(
            &runner::require_backend_executable(profile, backend)?,
            wt,
            task,
            session_dir,
            llm,
            &env_vars,
            profile
                .agy_print_timeout_seconds
                .get(llm.model.as_str())
                .copied(),
            profile.agy_idle_timeout_seconds(),
        ),
        "vibe" => runner::run_vibe_with_executable(
            &runner::require_backend_executable(profile, backend)?,
            wt,
            task,
            session_dir,
            effective_model,
            &profile.vibe_args,
            &env_vars,
            profile.vibe_idle_timeout_seconds(),
        ),
        "opencode" => runner::run_opencode_with_executable(
            &runner::require_backend_executable(profile, backend)?,
            wt,
            task,
            session_dir,
            effective_model,
            &profile.opencode_args,
            &env_vars,
            effective_model
                .and_then(|m| {
                    profile
                        .opencode_idle_timeout_seconds_by_model
                        .get(m)
                        .copied()
                })
                .unwrap_or_else(|| profile.opencode_idle_timeout_seconds()),
        ),
        _ => runner::run_openhands(
            wt,
            task,
            session_dir,
            llm,
            &profile.openhands_args,
            &env_vars,
            profile.openhands_idle_timeout_seconds(),
        ),
    };
    if let Some(origin_before) = origin_before {
        let origin_after = worktree::git(&["remote", "get-url", "origin"], wt)
            .context("checking git origin after backend run")?;
        if origin_after != origin_before {
            anyhow::bail!(
                "git origin changed during backend run: before='{origin_before}' after='{origin_after}'"
            );
        }
    }
    result
}

/// TICKET-101: usage the backend reported for exactly this attempt.
/// `RunResult` only carries a log path, not captured stdout in memory (see
/// `ReviewRunResult` for the pattern that does), so this reads that one
/// attempt's own log from disk. A read or parse failure yields an empty
/// (all-`None`) `LedgerUsage`, never a fabricated zero.
///
/// Issue #152: tries the codex exec --json parser first (JSONL event stream
/// produced when `--json` is passed to codex exec). Falls back to the
/// generic regex-based parser for non-JSONL output from other backends.
///
/// Issue #155: for AGY, also merges in the run-scoped cli.log delta
/// (quota/reset messages) -- the offset-scoped tail captured by runner,
/// NOT a fresh read of the whole cli.log, so a single attempt's usage is
/// never polluted by prior runs or concurrent appends.
pub(super) fn attempt_usage(
    log_path: &str,
    agy_cli_log_delta: Option<&str>,
    attribution: UsageAttribution<'_>,
    transcript_path: Option<&str>,
    claude_path: Option<&str>,
) -> crate::ledger::LedgerUsage {
    let text = match fs::read_to_string(log_path) {
        Ok(t) => t,
        Err(_) => {
            return normalize_attempt_usage(
                crate::ledger::LedgerUsage::default(),
                attribution,
                true,
            );
        }
    };
    let behavior_metrics = crate::telemetry::extractor::parse_structured_behavior_events(&text);
    let finalize = |mut usage: crate::ledger::LedgerUsage| {
        if let Some(metrics) = &behavior_metrics {
            usage = usage::merge_usage(
                usage,
                crate::ledger::LedgerUsage {
                    behavior_metrics: Some(metrics.clone()),
                    ..crate::ledger::LedgerUsage::default()
                },
            );
        }
        normalize_attempt_usage(usage, attribution, true)
    };

    // Claude Code: prefer the structured session transcript for real
    // per-attempt token/cost usage (issue #153). Never scrape stdout text.
    if attribution.backend == Some("claude") {
        if let Some(transcript) = transcript_path {
            if let Ok(t) = fs::read_to_string(transcript) {
                let transcript_usage = crate::claude_monitor::parse_claude_transcript_usage(&t);
                if transcript_usage.usage_source.is_some() {
                    let mut merged = transcript_usage;
                    // Merge any quota/cost info the log text parser still finds.
                    let log_usage = usage::parse_generic_usage(&text, "attempt_output_log");
                    merged = usage::merge_usage(merged, log_usage);
                    // Optionally enrich with a live `/usage` PTY probe (issue
                    // #153). Gated behind GAH_CLAUDE_LIVE_USAGE so normal
                    // dispatch stays bounded — the probe only runs when the
                    // operator explicitly opts in.
                    if let Some(path) = claude_path {
                        if std::env::var_os("GAH_CLAUDE_LIVE_USAGE").is_some() {
                            if let Ok(capture) =
                                crate::claude_monitor::capture_usage_via_pty(path, None)
                            {
                                let live =
                                    crate::claude_monitor::parse_claude_usage_text(&capture.raw);
                                merged = usage::merge_usage(merged, live);
                            }
                        }
                    }
                    if merged.usage_source.is_some() {
                        merged.observed_at = Some(
                            time::OffsetDateTime::now_utc()
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap_or_default(),
                        );
                    }
                    return finalize(merged);
                }
            }
        }
        // No transcript yet (or none located): fall back to the generic
        // stdout parser so partial observations are still recorded.
        let mut usage = usage::parse_generic_usage(&text, "attempt_output_log");
        if usage.usage_source.is_some() {
            usage.observed_at = Some(
                time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            );
        }
        return finalize(usage);
    }

    // Vibe's structured session metadata is passed through the same artifact
    // slot as Claude's transcript.
    if attribution.backend == Some("vibe") {
        if let Some(metadata) = transcript_path {
            if let Ok(metadata_json) = fs::read_to_string(metadata) {
                let session_usage = usage::parse_vibe_session_metadata(&metadata_json);
                if session_usage.usage_source.is_some() {
                    return finalize(session_usage);
                }
            }
        }
    }

    // OpenCode persists exact per-session model and token counters in its
    // local SQLite store. The runner snapshots only this invocation's row
    // into a JSON artifact, avoiding a racy global "latest session" lookup.
    if attribution.backend == Some("opencode") {
        if let Some(metadata) = transcript_path {
            if let Ok(metadata_json) = fs::read_to_string(metadata) {
                let session_usage = usage::parse_opencode_session_metadata(&metadata_json);
                if session_usage.usage_source.is_some() {
                    return finalize(session_usage);
                }
            }
        }
    }

    // Try codex exec --json parser first — handles JSONL output from
    // codex exec --json where the generic regex parser would find nothing.
    let mut usage = if attribution.backend == Some("codex") {
        usage::parse_codex_exec_json(&text)
    } else {
        crate::ledger::LedgerUsage::default()
    };
    if attribution.backend == Some("codex") {
        if let Some(transcript) = transcript_path {
            if let Ok(transcript_jsonl) = fs::read_to_string(transcript) {
                usage = usage::merge_usage(
                    usage,
                    usage::parse_codex_transcript_attribution(&transcript_jsonl),
                );
            }
        }
    }
    if attribution.backend == Some("openhands") {
        let openhands_usage = usage::parse_openhands_usage(&text);
        if openhands_usage.usage_source.is_some() {
            usage = usage::merge_usage(openhands_usage, usage);
        }
    }
    let has_json_lines = text.lines().any(|line| line.trim_start().starts_with('{'));
    if usage.usage_source.is_none() && (attribution.backend != Some("codex") || !has_json_lines) {
        // Fall back to the generic regex-based parser for other backends (or
        // for codex running in non-JSON mode).
        usage = usage::parse_generic_usage(&text, "attempt_output_log");
    }

    if let Some(delta) = agy_cli_log_delta {
        let agy = usage::parse_agy_cli_log_delta(delta, "agy_cli_log_delta");
        usage = usage::merge_usage(usage, agy);
    }

    if usage.usage_source.is_some() {
        usage.observed_at = Some(
            time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        );
    }
    finalize(usage)
}

/// Attribute a review invocation even when the reviewer does not expose token
/// counters. Reviews consume the same subscription/API capacity as coding
/// attempts, so an empty review-output parser result must not turn a completed
/// reviewer run into an invisible zero-usage ledger record.
///
/// AGY receives its fully-qualified model label as an explicit `--model`
/// argument. In that case the invoked model is directly observable from the
/// command contract, so retain it as the actual model when AGY itself did not
/// emit a more specific observation. Other backends may resolve aliases or
/// proxy routes after launch; leave their actual model unknown unless their
/// backend artifact reports it.
pub(super) fn review_usage(
    log_path: &str,
    agy_cli_log_delta: Option<&str>,
    attribution: UsageAttribution<'_>,
    usage_artifact_path: Option<&str>,
    claude_path: Option<&str>,
) -> crate::ledger::LedgerUsage {
    attempt_usage(
        log_path,
        agy_cli_log_delta,
        attribution,
        usage_artifact_path,
        claude_path,
    )
}

pub(super) fn preflight(profile: &Profile, backend: &str) -> Result<()> {
    ensure_bin("git")?;
    runner::require_backend_executable(profile, backend)?;
    Ok(())
}

/// TICKET-109: capabilities required for `backend` during review, profile
/// config taking precedence over shared defaults (same precedence
/// convention as `strong_review_backend`/`weak_review_backend`).
fn required_review_capabilities(cfg: &GahConfig, profile: &Profile, backend: &str) -> Vec<String> {
    profile
        .routing
        .review_required_capabilities
        .get(backend)
        .or_else(|| {
            cfg.defaults
                .routing
                .review_required_capabilities
                .get(backend)
        })
        .cloned()
        .unwrap_or_default()
}

/// TICKET-105: preflight for review, extended beyond plain binary
/// existence. Distinguishes exactly why a review can't proceed with its
/// configured capability policy:
/// - "backend unavailable" -- the executable itself doesn't resolve
/// - "required capability missing" -- executable present, but the
///   capability (e.g. Ponytail) isn't installed
/// - "reviewer degraded" -- the capability is required but GAH has no known
///   way to activate it for this backend (never silently downgrades)
///
/// Shared by `review()` (actual invocation) and `doctor.rs --validate`
/// (preflight only) so the two can never drift into inconsistent checks.
/// Returns the capabilities that will be applied on success.
pub fn review_preflight(cfg: &GahConfig, profile: &Profile, backend: &str) -> Result<Vec<String>> {
    if !matches!(
        runner::resolve_backend_executable(profile, backend),
        runner::ExecutableResolution::Found(_)
    ) {
        anyhow::bail!("backend unavailable: '{}' executable not found", backend);
    }
    let required = required_review_capabilities(cfg, profile, backend);
    for capability in &required {
        if !crate::capability::is_capability_available(capability, None) {
            anyhow::bail!(
                "required capability missing: '{}' is required for backend '{}' review but is not installed",
                capability,
                backend
            );
        }
        if crate::capability::activation_prefix(capability).is_none() {
            anyhow::bail!(
                "reviewer degraded: capability '{}' is required for backend '{}', but GAH does not know how to activate it -- refusing to silently run an ordinary review",
                capability,
                backend
            );
        }
    }
    Ok(required)
}

pub(super) fn ensure_bin(bin: &str) -> Result<()> {
    if which(bin).is_some() {
        Ok(())
    } else {
        anyhow::bail!("required binary '{}' not found on PATH", bin);
    }
}

pub(super) fn apply_route_to_ledger(ledger: &mut LedgerEntry, route: &RouteDecision) {
    ledger.backend = route.effective_backend.clone();
    ledger.requested_backend = route.requested_backend.clone();
    ledger.effective_backend = route.effective_backend.clone();
    ledger.requested_model = route.requested_model.clone();
    ledger.effective_model = route.effective_model.clone();
    ledger.routing_reason = Some(route.routing_reason.clone());
    ledger.fallback_used = route.fallback_used;
    ledger.confidence_impact = route.confidence_impact.clone();
    ledger.human_required_reason_code = None;
    ledger.human_required = route.human_required;
    ledger.routing_diagnostics = route.routing_diagnostics.clone();
}

pub(super) fn record_route_attempt(ledger: &mut LedgerEntry, route: &RouteDecision) -> Result<()> {
    route.identity.validate_for_persistence()?;
    ledger
        .routing_runtime
        .dispatch_attempted
        .insert(CandidateIdentity::from_execution_identity(&route.identity));
    ledger
        .attempt_routing
        .push(crate::ledger::AttemptRoutingRecord {
            attempt_number: ledger.attempt_routing.len() as u32 + 1,
            backend_instance: route.identity.backend_instance.clone(),
            effective_model: route.effective_model.clone(),
            identity: Some(route.identity.clone()),
            routing_diagnostics: route.routing_diagnostics.clone(),
        });
    Ok(())
}

/// Live-observed bug: `worktree::create`/`create_existing` failures (e.g. a
/// transient `git fetch` auth/network error) were propagating via `?`
/// straight past every `ledger.set_failure()` call site, reaching `run()`'s
/// top-level handler with `failure_class` still `None`. An unclassified
/// ticket is invisible to both of `decide_next_action`'s retry/escalate
/// loops (both gate on `Some(failure_class)`), so it becomes permanently
/// stuck once `prior_attempt_count > 0`. This is a harness/setup problem
/// (git plumbing), not the agent or backend failing at its job -- same
/// reasoning as the `BackendLaunch` classification below -- so classify it
/// the same way before propagating.
pub(super) fn classify_worktree_result<T>(
    ledger: &mut LedgerEntry,
    result: Result<T>,
) -> Result<T> {
    classify_git_operation_result(ledger, crate::ledger::FailureStage::Preflight, result)
}

pub(super) fn classify_git_operation_result<T>(
    ledger: &mut LedgerEntry,
    stage: crate::ledger::FailureStage,
    result: Result<T>,
) -> Result<T> {
    if let Err(err) = &result {
        let class = if worktree::is_transient_network_error(&format!("{err:#}")) {
            crate::ledger::FailureClass::EnvironmentError
        } else {
            crate::ledger::FailureClass::HarnessError
        };
        ledger.set_failure(class, stage);
    }
    result
}

pub(super) fn decide_route(
    cfg: &GahConfig,
    profile: &Profile,
    req: RouteRequest<'_>,
    task: Option<&WorkMetadata>,
    ledger: &mut LedgerEntry,
) -> Result<RouteDecision> {
    let runtime = routing_runtime_state(cfg, ledger)?;
    let decision = if let Some(task) = task {
        routing::decide_for_task_with_state(
            &cfg.defaults,
            profile,
            req,
            TaskRoutingContext {
                task_class: task.task_class.as_deref(),
                difficulty: task.difficulty.as_deref(),
                risk: task.risk.as_deref(),
            },
            &runtime,
        )
    } else {
        routing::decide_with_state(&cfg.defaults, profile, req, &runtime)
    };
    match decision {
        Ok(route) => Ok(route),
        Err(err) => {
            if let Some(route_err) = err.downcast_ref::<RouteError>() {
                let (selected_backend, selected_model, skipped) = match route_err {
                    RouteError::ApprovalRequired {
                        backend,
                        model,
                        skipped,
                    } => (Some(backend.clone()), model.clone(), skipped),
                    RouteError::NoEligibleBackend {
                        preferred_backend,
                        preferred_model,
                        skipped,
                        ..
                    } => (
                        Some(preferred_backend.clone()),
                        preferred_model.clone(),
                        skipped,
                    ),
                };
                ledger.routing_diagnostics = Some(crate::ledger::RoutingDiagnostics {
                    selected_backend,
                    selected_model,
                    candidates: skipped
                        .iter()
                        .enumerate()
                        .map(
                            |(index, candidate)| crate::ledger::RoutingCandidateDiagnostic {
                                backend: candidate.backend.clone(),
                                model: candidate.model.clone(),
                                consideration_order: Some(index),
                                skip_reason: Some(candidate.reason.clone()),
                                unavailable_until: candidate.unavailable_until.clone(),
                                ..Default::default()
                            },
                        )
                        .collect(),
                    human_summary: Some(route_err.to_string()),
                    ..Default::default()
                });
                // Transient: every candidate backend is momentarily unavailable
                // (quota/cooldown), and this self-resolves once an
                // `unavailable_until`/`earliest_reset` window passes -- same
                // "harness/setup, not agent failure" reasoning as
                // `classify_worktree_result` above. Match exhaustively so a
                // future non-transient `RouteError` variant doesn't silently
                // inherit this classification.
                let class = match route_err {
                    RouteError::NoEligibleBackend { .. } => {
                        if route_err.is_capacity_deferral() {
                            ledger.validation_result = Some("deferred_capacity".into());
                        }
                        crate::ledger::FailureClass::BackendError
                    }
                    RouteError::ApprovalRequired { backend, model, .. } => {
                        ledger.human_required = true;
                        ledger.human_required_reason_code =
                            Some(HumanRequiredReason::PolicyApproval.as_str().to_string());
                        ledger.error_summary = Some(format!(
                            "paid route approval required; run: gah route-approval grant --profile {} {} --backend {}{}",
                            ledger.profile,
                            ledger.work_id.as_deref().unwrap_or("<work-id>"),
                            backend,
                            model
                                .as_deref()
                                .map(|model| format!(" --model {model}"))
                                .unwrap_or_default()
                        ));
                        crate::ledger::FailureClass::HumanBlocked
                    }
                };
                ledger.set_failure(class, crate::ledger::FailureStage::Route);
            } else if format!("{:#}", err).contains("parsing availability state") {
                ledger.set_failure(
                    crate::ledger::FailureClass::EnvironmentError,
                    crate::ledger::FailureStage::Route,
                );
            }
            Err(err)
        }
    }
}

pub(super) fn routing_runtime_state(
    cfg: &GahConfig,
    current: &LedgerEntry,
) -> Result<RoutingRuntimeState> {
    let entries = ledger::read_entries(cfg)?;
    Ok(routing_runtime_state_from_entries(&entries, current))
}

pub(crate) fn routing_runtime_state_from_entries(
    entries: &[LedgerEntry],
    current: &LedgerEntry,
) -> RoutingRuntimeState {
    let cutoff = OffsetDateTime::now_utc() - time::Duration::days(7);
    let mut state = RoutingRuntimeState::default();

    for entry in entries
        .iter()
        .filter(|entry| entry.profile == current.profile)
    {
        let in_window = OffsetDateTime::parse(&entry.timestamp, &Rfc3339)
            .map(|timestamp| timestamp >= cutoff)
            .unwrap_or(false);
        if in_window && is_agent_execution_mode(&entry.mode) {
            if entry.attempts.is_empty() {
                if entry.attempts_completed.unwrap_or(0) > 0 || entry.backend_exit_code.is_some() {
                    record_recent_route_run(
                        &mut state,
                        &entry.effective_backend,
                        entry.effective_model.as_deref(),
                    );
                }
            } else {
                for attempt in &entry.attempts {
                    record_recent_route_run(
                        &mut state,
                        &attempt.backend,
                        attempt.effective_model.as_deref(),
                    );
                }
            }
        }
    }
    for attempt in &current.attempts {
        record_recent_route_run(
            &mut state,
            &attempt.backend,
            attempt.effective_model.as_deref(),
        );
    }

    if let Some(work_id) = current.work_id.as_deref() {
        let aliases = ledger::work_id_aliases(work_id);
        if is_implementation_execution_mode(&current.mode) {
            for entry in entries.iter().filter(|entry| {
                entry.profile == current.profile
                    && entry.repo_id == current.repo_id
                    && entry
                        .work_id
                        .as_deref()
                        .is_some_and(|id| aliases.iter().any(|alias| alias == id))
                    && is_implementation_execution_mode(&entry.mode)
            }) {
                record_genuine_failure_routes(&mut state, entry);
            }
            record_genuine_failure_routes(&mut state, current);
        }
        for (backend, model) in
            ledger::active_paid_route_approvals_from_entries(entries, &current.profile, work_id)
        {
            state
                .approved
                .insert(CandidateIdentity::new(backend, model));
        }
    }
    state
        .dispatch_attempted
        .extend(current.routing_runtime.dispatch_attempted.iter().cloned());

    state
}

fn record_recent_route_run(state: &mut RoutingRuntimeState, backend: &str, model: Option<&str>) {
    if backend.is_empty() {
        return;
    }
    *state
        .recent_runs
        .entry(CandidateIdentity::new(backend, model))
        .or_insert(0) += 1;
}

fn is_agent_execution_mode(mode: &str) -> bool {
    matches!(mode, "improve" | "fix" | "experiment" | "pm" | "review")
}

fn is_implementation_execution_mode(mode: &str) -> bool {
    matches!(mode, "improve" | "fix" | "experiment")
}

pub(super) fn record_genuine_failure_routes(state: &mut RoutingRuntimeState, entry: &LedgerEntry) {
    let mut recorded_attempt = false;
    for attempt in &entry.attempts {
        if attempt
            .failure_class
            .as_deref()
            .is_some_and(crate::controller::is_genuine_agent_failure)
        {
            state.attempted.insert(CandidateIdentity::new(
                attempt.backend.as_str(),
                attempt.effective_model.as_deref(),
            ));
            recorded_attempt = true;
        }
    }
    if !recorded_attempt
        && entry
            .failure_class
            .as_deref()
            .is_some_and(crate::controller::is_genuine_agent_failure)
        && !entry.effective_backend.is_empty()
    {
        state.attempted.insert(CandidateIdentity::new(
            entry.effective_backend.as_str(),
            entry.effective_model.as_deref(),
        ));
    }
}

pub(super) fn route_identity(backend: &str, model: Option<&str>) -> String {
    format!("{backend}\u{0}{}", model.unwrap_or(""))
}

pub(super) fn route_label(backend: &str, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{backend}/{model}"),
        None => backend.to_string(),
    }
}

/// Local-only recovery refs for discarded retry attempts. Keep the prefix
/// separate from normal dispatch branches so pruning/inspection can identify
/// them unambiguously without inventing a second user-facing ticket ID.
pub(super) fn wip_checkpoint_branch(dispatch_branch: &str, attempt: u32) -> String {
    format!(
        "gah-wip/{}-attempt-{attempt}",
        dispatch_branch.trim_start_matches("gah/").replace('/', "-")
    )
}

pub(super) fn clear_wip_checkpoints(repo: &Path, checkpoints: &[String]) {
    for checkpoint in checkpoints {
        if let Err(error) = worktree::delete_local_branch(repo, checkpoint) {
            eprintln!(
                "warning: could not remove successful WIP checkpoint {checkpoint}: {error:#}"
            );
        }
    }
}

/// Current instant, but with the *local* UTC offset attached rather than
/// always `+00:00`. `quota_parser::parse` treats a no-timezone time-of-day
/// string in backend output (e.g. Codex's "resets 9:01 PM") as being in
/// `now`'s offset, since that's the only clock available to a backend CLI
/// printing to its own terminal -- it means local wall-clock time, not
/// UTC. Passing `OffsetDateTime::now_utc()` silently mis-resolved every
/// such reset by exactly the host's UTC offset (observed live: a ~3am
/// local reset displaying as "~14h remaining" on a UTC-5 host). Falls back
/// to UTC only if the local offset genuinely can't be determined.
fn now_with_local_offset() -> OffsetDateTime {
    let local_offset_seconds = chrono::Local::now().offset().local_minus_utc();
    let offset =
        time::UtcOffset::from_whole_seconds(local_offset_seconds).unwrap_or(time::UtcOffset::UTC);
    OffsetDateTime::now_utc().to_offset(offset)
}

pub(super) fn mark_backend_unavailable_from_output_for_identity(
    identity: &crate::execution_identity::ExecutionIdentity,
    log_text: &str,
    log_path: &str,
) -> Result<Option<crate::quota_parser::ParsedFailure>> {
    mark_backend_unavailable_from_output_for_identity_at(
        &crate::availability::resolve_state_path(),
        identity,
        log_text,
        log_path,
    )
}

#[cfg(test)]
fn mark_backend_unavailable_from_output_at(
    state_path: &Path,
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
    log_text: &str,
    log_path: &str,
) -> Result<Option<crate::quota_parser::ParsedFailure>> {
    let identity =
        crate::execution_identity::ExecutionIdentity::legacy_candidate(backend, model, quota_pool);
    mark_backend_unavailable_from_output_for_identity_at(state_path, &identity, log_text, log_path)
}

fn mark_backend_unavailable_from_output_for_identity_at(
    state_path: &Path,
    identity: &crate::execution_identity::ExecutionIdentity,
    log_text: &str,
    log_path: &str,
) -> Result<Option<crate::quota_parser::ParsedFailure>> {
    let backend = identity.logical_backend.as_str();
    let now = now_with_local_offset();
    // An idle watchdog kill is a backend outage signal, not an ordinary agent
    // failure. Keep this route out of the candidate set for a short bounded
    // cooldown so the next attempt can use another backend/model instead of
    // burning the same five-minute stall again. Both terminal-attribution
    // variants (stalled before changes / stalled during validation with
    // checkpointed changes) are idle-watchdog stalls and trigger the cooldown.
    if log_text.contains("GAH: killed after ")
        && log_text.contains("(stalled")
        && log_text.contains("not just slow).")
    {
        let cooldown = now + time::Duration::minutes(15);
        crate::availability::record_unavailable_for_identity(
            state_path,
            identity,
            crate::availability::Reason::BackendOutage,
            crate::availability::Source::BackendError,
            Some(cooldown),
            Some(format!(
                "backend idle watchdog stalled; cooldown=15m; log={log_path}"
            )),
            now,
        )?;
        return Ok(Some(crate::quota_parser::ParsedFailure {
            backend: backend.to_string(),
            kind: crate::quota_parser::FailureKind::BackendStalled,
            retryable: true,
            reset_at: Some(cooldown.format(&Rfc3339)?),
            retry_after_seconds: Some(15 * 60),
            confidence: crate::quota_parser::Confidence::High,
            matched_evidence: "GAH idle watchdog stall".to_string(),
            unresolved_timezone: None,
        }));
    }
    let Some(parsed) = crate::quota_parser::parse(backend, log_text, now) else {
        return Ok(None);
    };

    let unavailable_until = if let Some(reset_at) = parsed.reset_at.as_deref() {
        OffsetDateTime::parse(reset_at, &Rfc3339).ok()
    } else {
        parsed
            .retry_after_seconds
            .map(|secs| now + time::Duration::seconds(secs as i64))
    };
    let reason = match parsed.kind {
        crate::quota_parser::FailureKind::QuotaExhausted => {
            crate::availability::Reason::QuotaExhausted
        }
        crate::quota_parser::FailureKind::RateLimited => crate::availability::Reason::RateLimited,
        crate::quota_parser::FailureKind::AuthenticationError => {
            crate::availability::Reason::AuthenticationError
        }
        crate::quota_parser::FailureKind::BackendStalled => {
            crate::availability::Reason::BackendOutage
        }
    };
    let summary = format!(
        "{}; confidence={:?}; log={}",
        parsed.matched_evidence, parsed.confidence, log_path
    );
    crate::availability::record_unavailable_for_identity(
        state_path,
        identity,
        reason,
        crate::availability::Source::BackendError,
        unavailable_until,
        Some(summary),
        now,
    )?;
    Ok(Some(parsed))
}

pub(super) fn route_after_backend_unavailable<'a>(
    cfg: &crate::config::GahConfig,
    profile: &Profile,
    route_req: &RouteRequest<'a>,
    ticket_meta: Option<&WorkMetadata>,
    ledger: &mut LedgerEntry,
    route: &RouteDecision,
    failure_output: (&str, &str),
) -> Result<Option<(crate::quota_parser::ParsedFailure, RouteDecision)>> {
    let Some(parsed) = mark_backend_unavailable_from_output_for_identity(
        &route.identity,
        failure_output.0,
        failure_output.1,
    )?
    else {
        return Ok(None);
    };

    let rerouted = decide_route(cfg, profile, route_req.clone(), ticket_meta, ledger)?;

    Ok(Some((parsed, rerouted)))
}

/// Combine CLI output with the run-scoped diagnostic tail captured from a
/// backend-owned internal log. Missing internal logs intentionally preserve
/// existing output-only behavior.
pub(super) fn failure_text_with_internal_log(
    output: &str,
    internal_log_delta: Option<&str>,
) -> String {
    let Some(delta) = internal_log_delta.filter(|delta| !delta.trim().is_empty()) else {
        return output.to_string();
    };
    if output.trim().is_empty() {
        return format!("[backend internal log]\n{delta}");
    }
    format!("{output}\n\n[backend internal log]\n{delta}")
}

#[cfg(test)]
mod tests;
