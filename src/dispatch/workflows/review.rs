use super::super::attempts::{
    apply_backend_instance_env, apply_route_to_ledger, decide_route, mark_shutdown_cancelled,
    record_route_attempt, reserve_backend_slot, review_preflight, review_usage,
    route_after_backend_unavailable, route_identity, route_label,
};
use super::super::prompts::enforce_context_budget;
use super::super::publish::{render_review_comment, review_labels};
use super::super::review::context::{
    lookup_review_state_by_branch, prepare_review_diff, resolve_review_target, ReviewTarget,
};
use super::super::review::policy::{
    check_review_budget, derive_reviewer_tier, is_retryable_format_only_violation,
    next_escalatory_reviewer, next_review_candidate, parse_review_verdict_with_context,
    review_escalation_reason, review_output_invalid_error, reviewer_dedup_class,
    ReviewBudgetExhausted, ReviewGateContext, REVIEW_FORMAT_ONLY_VIOLATION_REASON,
};
use super::super::text::{utf8_safe_prefix, utf8_safe_suffix};
use super::super::DispatchArgs;
use crate::availability;
use crate::config::{self, GahConfig, Profile};
use crate::controller::HumanRequiredReason;
use crate::ledger::LedgerEntry;
use crate::notifications::{notify_event, NotifyEvent};
use crate::routing::{ConcurrencyGuard, RouteDecision, RouteRequest};
use crate::usage_attribution::{
    aggregate_attempt_usage, normalize_attempt_usage, UsageAttribution,
};
use crate::{provider, runner};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

mod identity;
mod source_issue_context;
mod source_issue_sections;
use identity::canonicalize_review_ledger_identity;
use source_issue_context::{
    apply_resolved_source_issue_identity, render_untrusted_inline_review_text,
    render_untrusted_review_text, resolve_source_issue_context,
    verified_post_budget_source_contract,
};

fn review_outcome_allows_reroute(outcome: &runner::ReviewProcessOutcome) -> bool {
    matches!(
        outcome,
        runner::ReviewProcessOutcome::NonZeroExit(_) | runner::ReviewProcessOutcome::IdleTimeout
    )
}

fn review_failure_output(
    outcome: &runner::ReviewProcessOutcome,
    stdout: &str,
    stderr: &str,
    idle_timeout_seconds: u64,
) -> String {
    let mut output = if stdout.trim().is_empty() {
        stderr.to_string()
    } else if stderr.trim().is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    };
    if matches!(outcome, runner::ReviewProcessOutcome::IdleTimeout) {
        let stall_marker = format!(
            "GAH: killed after {idle_timeout_seconds}s with no new worktree progress (stalled before changes, not just slow)."
        );
        if output.trim().is_empty() {
            output = stall_marker;
        } else {
            output.push('\n');
            output.push_str(&stall_marker);
        }
    }
    output
}

fn review_failure_log_path(attempt_session: &Path, stdout: &str) -> String {
    let stream = if stdout.trim().is_empty() {
        "review-stderr.log"
    } else {
        "review-stdout.log"
    };
    attempt_session.join(stream).display().to_string()
}

fn review_terminal_failure_summary(ledger: &LedgerEntry, final_failure: &str) -> String {
    let mut routes = Vec::new();
    let mut seen = HashSet::new();
    for attempt in &ledger.attempts {
        let route = route_label(&attempt.backend, attempt.effective_model.as_deref());
        if seen.insert(route.clone()) {
            routes.push(route);
        }
    }
    let attempts = ledger.attempts_started.unwrap_or_default();
    if routes.is_empty() {
        format!("review failed after {attempts} attempt(s): {final_failure}")
    } else {
        format!(
            "review failed after {attempts} attempt(s): {final_failure}; attempted routes: {}",
            routes.join(" -> ")
        )
    }
}

pub(in crate::dispatch) fn review(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    // Live-observed: a review dispatch that fails resolving its target
    // (e.g. a transient `git fetch` network reset) returns via `?` below
    // before any target info reaches the ledger, so the DispatchFailed
    // notification had nothing to show but "work_id=unknown". Record the
    // requested branch up front -- the caller (controller's ReviewMr
    // action) always knows which branch it asked to review, even if
    // resolving the rest of the target fails.
    ledger.branch = args
        .branch
        .clone()
        .or_else(|| args.mr.as_deref().map(|mr| format!("mr:{mr}")));
    let repo = Path::new(&profile.local_path);
    let mut target = resolve_review_target(cfg, profile, args)?;
    canonicalize_review_ledger_identity(
        ledger,
        &target.source_branch,
        target.mr_url.as_deref(),
        target.mr_title.as_deref(),
    );
    if target.prior_state.is_none() {
        target.prior_state =
            lookup_review_state_by_branch(cfg, &args.profile, &target.source_branch);
    }
    let diff_bundle = prepare_review_diff(repo, profile, &mut target)?;
    if target.mr_url.is_some() {
        // `prepare_review_diff` captures exact fetched SHAs when provider
        // metadata omitted them, so fingerprint only after that identity is
        // final rather than preserving a pre-fetch partial fingerprint.
        target.metadata_fingerprint = Some(crate::sync::review_metadata_fingerprint(
            target.source_sha.as_deref(),
            target.mr_title.as_deref(),
            target.mr_body.as_deref(),
            target.draft,
        ));
    }
    ledger.review_source_sha = target.source_sha.clone();
    ledger.review_metadata_fingerprint = target.metadata_fingerprint.clone();
    ledger.review_contract_version = Some(crate::ledger::REVIEW_CONTRACT_VERSION);
    ledger.review_generation = crate::ledger::review_generation(
        ledger.review_source_sha.as_deref(),
        ledger.review_metadata_fingerprint.as_deref(),
    );
    let bundle = session_dir.join("review-bundle");
    fs::create_dir_all(&bundle)?;
    fs::write(bundle.join("diff.patch"), &diff_bundle.diff)?;
    fs::write(bundle.join("changed-files.txt"), &diff_bundle.files)?;
    fs::write(
        bundle.join("mr-description.md"),
        format!(
            "MR: {}\nURL: {}\nSource: {}\nTarget: {}\nSource SHA: {}\nTarget SHA: {}\nRepo: {}\nTitle: {}\nDraft: {}\nCI: {}\nMergeability: {}",
            target.mr_id.as_deref().unwrap_or("n/a"),
            target.mr_url.as_deref().unwrap_or("n/a"),
            target.source_branch,
            target.target_branch,
            target.source_sha.as_deref().unwrap_or("unknown"),
            target.target_sha.as_deref().unwrap_or("unknown"),
            profile.repo,
            target.mr_title.as_deref().unwrap_or("n/a"),
            target.draft,
            target.ci_status.as_deref().unwrap_or("unknown"),
            target.merge_status.as_deref().unwrap_or("unknown"),
        ),
    )?;
    println!(
        "Diff: {} bytes, files: {}",
        diff_bundle.diff.len(),
        diff_bundle.files.lines().count()
    );
    let source_issue_context = resolve_source_issue_context(
        cfg,
        profile,
        &args.profile,
        ledger.work_id.as_deref(),
        &target,
    )?;
    apply_resolved_source_issue_identity(ledger, &source_issue_context);
    let review_gate_context =
        ReviewGateContext::from_diff_bundle(&diff_bundle, target.ci_status.as_deref())
            .with_source_acceptance(
                source_issue_context.acceptance_criteria.clone(),
                &profile.provider,
            );
    fs::write(
        bundle.join("source-issue-lookup.json"),
        serde_json::to_string_pretty(&source_issue_context.lookup_report)?,
    )?;
    // Everything except the capability-activation prefix is identical
    // regardless of which backend ends up running the review.
    let prompt_suffix = format!(
        "## Review Pack\n\n\
         Review this diff for correctness, test coverage, and safety. \
         Return a JSON object. You may precede it only with the inert heading `Review notes`; put every substantive finding in the JSON arrays, never in prose.\n\
         The JSON object fields are: verdict, confidence, human_required, actionable_findings, non_blocking_findings, risk_notes, evidence, compatibility_evidence.\n\
         actionable_findings must be an array of objects with exactly: summary (string), file (an exact path copied from Changed files), line (string or null), status (the literal confirmed), and evidence (an array containing at least one diff:<same-file>:<specific observation> string). non_blocking_findings, risk_notes, evidence, and compatibility_evidence must be JSON arrays of strings, even when empty or when only one item exists.\n\
         NEEDS_FIX and REJECT require at least one actionable_findings object. Never put a withdrawn, speculative, unverified, contradicted, or explicitly non-blocking concern in actionable_findings; put uncertainty in non_blocking_findings or return HUMAN_REVIEW. GAH rejects invalid actionable findings and reroutes to another reviewer instead of dispatching a repair.\n\
         For an APPROVE, evidence must include exactly one or more file:<changed-path> entries copied from Changed files below. You may include ci:passed only when the displayed control-plane CI status is passed. An APPROVE without grounded file evidence is invalid.\n\
         Every source acceptance criterion is blocking until verified. The canonical Source Issue Contract numbers them explicitly; criterion N is numbered item N in that list. For criterion N, put a separate string directly inside the existing `evidence` array using `ac:N:file:<changed-path>` or `ac:N:test:<command and result>`. Each string must contain exactly one mapping: never append a second `ac:N:` mapping, prose, or a test result to an `ac:N:file:<changed-path>` entry, because the complete suffix is validated as the path. Do not create an `ac_evidence` field, `actionable_findings_ac` field, or any other JSON field; extra fields are ignored and cannot satisfy the gate. Before returning APPROVE, audit the `evidence` array for a contiguous `ac:1:` through `ac:N:` set; ordinary unprefixed file/test evidence does not satisfy this gate. If the criterion claims current, live, latest, exact, open/closed, queued, or other external provider state, file/test evidence alone is insufficient: use `ac:N:provider:<provider>:<queried reference and result>` or `ac:N:snapshot:<changed-path>:<verification command and result>`. The provider must match this profile. If any criterion remains unmet or materially unverified, return NEEDS_FIX with a concrete blocking finding; never hide that admission in non_blocking_findings or risk_notes while approving.\n\
         If a contract surface is changed, do not APPROVE unless compatibility_evidence includes file:<changed-contract-path> and mechanism:<schema-version|backward-compatible-default|migration> that is actually present in the diff.\n\
         Verdict must be one of APPROVE, NEEDS_FIX, REJECT, HUMAN_REVIEW, defined as:\n\
         - APPROVE: you believe the change is correct, safe, and complete enough to merge. Report your ACTUAL confidence honestly in the separate `confidence` field (high/medium/low) -- do not inflate confidence to sound more certain, and do not downgrade to NEEDS_FIX just to hedge when you'd otherwise approve. A low-confidence approval is a real, useful signal (insufficient context, a domain you couldn't fully verify, a partial review) and will correctly route to a human -- it is not a failure to be avoided.\n\
         - NEEDS_FIX: you found a concrete, confirmed problem that should be fixed before merge. Put it in actionable_findings with direct changed-file evidence, even if it isn't an immediate crash -- e.g. silent data loss, a hidden failure mode, or anything that would take real effort to diagnose later if left in. Do not downgrade a confirmed risk into non_blocking_findings/risk_notes just because it wouldn't break the build today.\n\
         - REJECT: the change is fundamentally wrong and should not be merged as-is.\n\
         - HUMAN_REVIEW: you cannot make a confident recommendation at all.\n\
         Repo: {}. MR: {}. Source: {}. Target: {}. Draft: {}. CI status: {}. Mergeability status: {}.\n\
         MR title: {}\nMR body:\n{}\n\
         {}\n\
         ## Prior Run State\n\n{}\n\n## Diff\n\n```\n{}\n```\nChanged files:\n{}",
        profile.repo,
        target.mr_id.as_deref().unwrap_or("n/a"),
        target.source_branch,
        target.target_branch,
        target.draft,
        target.ci_status.as_deref().unwrap_or("unknown"),
        target.merge_status.as_deref().unwrap_or("unknown"),
        render_untrusted_inline_review_text(
            target.mr_title.as_deref().unwrap_or("n/a"),
            REVIEW_MR_TITLE_MAX_BYTES,
        ),
        render_untrusted_review_text(
            target.mr_body.as_deref().unwrap_or("n/a"),
            REVIEW_MR_BODY_MAX_BYTES,
        ),
        source_issue_context
            .prompt_section
            .as_deref()
            .unwrap_or(""),
        render_untrusted_review_text(
            target.prior_state.as_deref().unwrap_or("not found"),
            REVIEW_PRIOR_STATE_MAX_BYTES,
        ),
        utf8_safe_prefix(&diff_bundle.diff, 60_000),
        diff_bundle.files,
    );

    let resolved_env = if args.prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    let mut env_vars = if resolved_env.is_empty() {
        vec![]
    } else {
        runner::load_env_file(resolved_env)
    };
    let cargo_target =
        crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root, session_dir)?;
    env_vars.extend(cargo_target.environment());

    // Escalate to the next untried reviewer in the ordered
    // ESCALATORY_REVIEW list. A routine reviewer may legitimately request
    // human help or fail the deterministic evidence gate; that is an input to
    // this bounded second-opinion chain, not an immediate terminal handoff.
    let escalation_reason = review_escalation_reason(
        cfg,
        profile,
        &args.profile,
        &target.source_branch,
        ledger.review_generation.as_deref(),
    );
    let availability_state_path = availability::resolve_state_path();
    let next_reviewer = escalation_reason.and_then(|reason| {
        if reason == "review_output_invalid" {
            next_review_candidate(
                cfg,
                profile,
                &args.profile,
                &target.source_branch,
                None,
                ledger.review_generation.as_deref(),
                &availability_state_path,
            )
        } else {
            next_escalatory_reviewer(
                cfg,
                profile,
                &args.profile,
                &target.source_branch,
                None,
                ledger.review_generation.as_deref(),
                &availability_state_path,
            )
        }
    });
    let (requested_backend, requested_model) = match (escalation_reason, next_reviewer.as_ref()) {
        (Some(reason), Some(esc)) => {
            println!(
                "Escalating review to {}/{} ({reason}) for branch {}",
                esc.backend,
                esc.model.as_deref().unwrap_or("default"),
                target.source_branch
            );
            (esc.backend.as_str(), esc.model.as_deref())
        }
        (Some(reason), None) => {
            return stop_for_exhausted_review_escalation(cfg, profile, ledger, &target, reason);
        }
        _ => (
            config::canonical_backend_name(&args.backend),
            args.model.as_deref(),
        ),
    };

    // An ordered escalation selects an exact reviewer identity. Generic
    // review fallback must not substitute a previously used reviewer when
    // that exact route needs approval or is unavailable.
    let exact_route_required = next_reviewer.is_some();
    let route_request = RouteRequest {
        last_failure_class: None,
        mode: "review",
        requested_backend,
        requested_model,
        recommended_backend: None,
        recommended_model: None,
        session_id: session_dir.file_name().and_then(|s| s.to_str()),
        usage_summary: None,
        exact_route_required,
    };
    let mut route = decide_route(cfg, profile, route_request.clone(), None, ledger)?;

    // Duplicate-review short-circuit runs before the budget check: if nothing
    // has changed since the last completed review of the same tier, that is
    // the operator-relevant reason to skip, not a budget refusal, and it must
    // not consume any part of the review-cycle budget below.
    let reviewer_class = reviewer_dedup_class(derive_reviewer_tier(cfg, profile, &route), &route);
    if let (Some(work_id), Some(source_sha), Some(metadata_fingerprint)) = (
        ledger.work_id.as_deref(),
        target.source_sha.as_deref(),
        target.metadata_fingerprint.as_deref(),
    ) {
        if crate::ledger::review_already_exists(
            cfg,
            &args.profile,
            &profile.repo_id,
            work_id,
            source_sha,
            metadata_fingerprint,
            &reviewer_class,
        )? {
            ledger.validation_result = Some("skipped_duplicate_review".into());
            ledger.review_source_sha = Some(source_sha.to_string());
            ledger.review_metadata_fingerprint = Some(metadata_fingerprint.to_string());
            ledger.reviewer_class = Some(reviewer_class.to_string());
            println!("Skipping duplicate {reviewer_class} review for {work_id} at {source_sha}");
            return Ok(());
        }
    }
    ledger.reviewer_class = Some(reviewer_class.to_string());

    if let Some(block) = check_review_budget(
        cfg,
        profile,
        &args.profile,
        ledger.work_id.as_deref(),
        &route,
        ledger.review_generation.as_deref(),
    )? {
        mark_review_budget_exhausted(ledger, &route, &block.reason);
        notify_event(
            cfg,
            profile,
            NotifyEvent::HumanRequired {
                reason: "review budget exhausted",
                reference: target.mr_url.as_deref(),
                reason_code: Some(HumanRequiredReason::RetryBudgetExhausted.as_str()),
                failure_class: ledger.failure_class.as_deref().unwrap_or("human_blocked"),
                failure_stage: ledger.failure_stage.as_deref(),
                error_summary: ledger.error_summary.as_deref(),
                attempt_count: ledger.attempts_started,
                mr_url: target
                    .mr_url
                    .as_deref()
                    .or(Some(target.source_branch.as_str())),
            },
        );
        return Err(ReviewBudgetExhausted::new(block.reason).into());
    }

    // Reserve the selected reviewer before the controller lets the next
    // parallel slot route. Implementation dispatches already use this same
    // rendezvous; reviews must participate too or sibling reviews can all
    // observe a zero live count and select a backend/model capped at one.
    let mut review_slot = Some(reserve_review_route(profile, &route)?);
    if let Some(route_ready) = &args.route_ready {
        let _ = route_ready.send(());
    }

    // Bounded retry across review_candidates: an empty/unavailable-backend
    // outcome (e.g. AGY quota exhaustion -- see agy_empty_output_diagnosis)
    // used to fail the whole review outright even though review_candidates
    // often lists real fallbacks (agy-second, claude) that just sat unused.
    const MAX_REVIEW_ATTEMPTS: usize = 3;
    const REVIEW_FORMAT_REPAIR_ATTEMPTS: usize = 1;
    let mut applied_capabilities = vec![];
    let mut prior_review_context = String::new();
    let mut should_repair_format = false;
    let mut format_only_repair_count = 0usize;
    let mut result = None;
    let mut parsed_verdict: Option<Result<crate::models::ReviewVerdict>> = None;
    let mut attempt_index = 0usize;
    'attempts: for attempt_number in 0..MAX_REVIEW_ATTEMPTS {
        let required_capabilities = review_preflight(cfg, profile, &route.effective_backend)?;
        let mut capability_prefix = String::new();
        applied_capabilities.clear();
        for capability in &required_capabilities {
            let prefix = crate::capability::activation_prefix(capability)
                .expect("review_preflight already validated an activation mapping exists");
            capability_prefix.push_str(prefix);
            applied_capabilities.push(capability.clone());
        }
        let fresh_context = cfg
            .context
            .effective(&args.profile, &route.effective_backend)
            .fresh_context_on_review;
        // Inner loop: a single bounded format-repair retry to the same
        // reviewer/route (REVIEW_FORMAT_REPAIR_ATTEMPTS) lives here so it
        // never advances `attempt_number` and never consumes one of the
        // MAX_REVIEW_ATTEMPTS reroute-across-review_candidates slots.
        loop {
            ledger.attempts_started = Some(ledger.attempts_started.unwrap_or(0) + 1);
            apply_route_to_ledger(ledger, &route);
            let mut prompt = format!("{capability_prefix}{prompt_suffix}");
            let is_format_repair = should_repair_format;
            if is_format_repair {
                prompt.push_str("\n\n## Review Format Repair\n");
                prompt.push_str(REVIEW_FORMAT_REPAIR_INSTRUCTIONS);
                should_repair_format = false;
            }
            // The format-repair retry already receives the full original
            // review task above. Re-injecting the violating response as prior
            // context contradicts the repair instruction and encourages the
            // reviewer to repeat the same prose.
            if !is_format_repair && !fresh_context && !prior_review_context.is_empty() {
                prompt.push_str("\n\n## Prior Review Attempt\n");
                prompt.push_str(&prior_review_context);
            }
            let prompt = enforce_context_budget(
                cfg,
                profile,
                &args.profile,
                &route.effective_backend,
                "review",
                fresh_context,
                &prompt,
                session_dir,
                args.run_id.as_deref(),
                ledger,
            )?;
            if let Some(contract) = verified_post_budget_source_contract(
                source_issue_context.contract.as_deref(),
                &prompt,
            )? {
                fs::write(bundle.join("source-issue-contract.md"), contract)?;
            }

            attempt_index += 1;
            let attempt_session = session_dir.join(format!("review-attempt-{attempt_index}"));
            fs::create_dir_all(&attempt_session)?;
            record_route_attempt(ledger, &route);
            let attempt_env_vars =
                review_attempt_environment(profile, &route.effective_backend, &env_vars);
            let attempt = runner::run_review_backend(
                profile,
                &route.effective_backend,
                repo,
                &prompt,
                &attempt_session,
                route.effective_model.as_deref(),
                &attempt_env_vars,
            );
            // The slot covers the backend invocation itself. Release it before
            // parsing/rerouting so another worker can use the reviewer as soon as
            // capacity is genuinely free.
            drop(review_slot.take());
            if !matches!(
                &attempt.outcome,
                runner::ReviewProcessOutcome::ExecutableUnavailable
                    | runner::ReviewProcessOutcome::SpawnFailure
            ) {
                ledger.attempts_completed = Some(ledger.attempts_completed.unwrap_or(0) + 1);
            }
            let attribution = UsageAttribution::from_route(&route);
            let usage = if matches!(
                &attempt.outcome,
                runner::ReviewProcessOutcome::ExecutableUnavailable
                    | runner::ReviewProcessOutcome::SpawnFailure
            ) {
                normalize_attempt_usage(crate::ledger::LedgerUsage::default(), attribution, false)
            } else {
                review_usage(
                    &attempt_session
                        .join("review-stdout.log")
                        .display()
                        .to_string(),
                    attempt.agy_cli_log_delta.as_deref(),
                    attribution,
                    attempt.usage_artifact_path.as_deref(),
                    profile.claude_path.as_deref(),
                )
            };
            let (exit_code, validation_result, failure_class, failure_stage) =
                match &attempt.outcome {
                    runner::ReviewProcessOutcome::Success => (Some(0), None, None, None),
                    runner::ReviewProcessOutcome::ExecutableUnavailable => (
                        None,
                        Some("not_run".to_string()),
                        Some(
                            crate::ledger::FailureClass::EnvironmentError
                                .as_str()
                                .to_string(),
                        ),
                        Some(crate::ledger::FailureStage::Review.as_str().to_string()),
                    ),
                    runner::ReviewProcessOutcome::SpawnFailure => (
                        None,
                        Some("not_run".to_string()),
                        Some(
                            crate::ledger::FailureClass::HarnessError
                                .as_str()
                                .to_string(),
                        ),
                        Some(
                            crate::ledger::FailureStage::BackendLaunch
                                .as_str()
                                .to_string(),
                        ),
                    ),
                    runner::ReviewProcessOutcome::NonZeroExit(code) => (
                        Some(*code),
                        Some("not_run".to_string()),
                        Some(
                            crate::ledger::FailureClass::BackendError
                                .as_str()
                                .to_string(),
                        ),
                        Some(crate::ledger::FailureStage::Review.as_str().to_string()),
                    ),
                    runner::ReviewProcessOutcome::SignalTermination(signal) => (
                        Some(-*signal),
                        Some("cancelled_shutdown".to_string()),
                        Some(
                            crate::ledger::FailureClass::HarnessError
                                .as_str()
                                .to_string(),
                        ),
                        Some(crate::ledger::FailureStage::Review.as_str().to_string()),
                    ),
                    runner::ReviewProcessOutcome::CleanupFailure(_) => (
                        Some(crate::runner::process::PROCESS_CLEANUP_FAILED_EXIT_CODE),
                        Some("not_run_process_cleanup_failed".to_string()),
                        Some(
                            crate::ledger::FailureClass::HarnessError
                                .as_str()
                                .to_string(),
                        ),
                        Some(crate::ledger::FailureStage::Review.as_str().to_string()),
                    ),
                    runner::ReviewProcessOutcome::IdleTimeout => (
                        None,
                        Some("not_run_idle_timeout".to_string()),
                        Some(
                            crate::ledger::FailureClass::AgentNoProgress
                                .as_str()
                                .to_string(),
                        ),
                        Some(crate::ledger::FailureStage::Review.as_str().to_string()),
                    ),
                    runner::ReviewProcessOutcome::HardTimeout => (
                        None,
                        Some("not_run_hard_timeout".to_string()),
                        // A healthy reviewer that merely exceeded the hard safety ceiling
                        // is NOT a backend failure; classify it so it never triggers
                        // retry/escalation to a paid reviewer (issue #540).
                        Some(
                            crate::ledger::FailureClass::HumanBlocked
                                .as_str()
                                .to_string(),
                        ),
                        Some(crate::ledger::FailureStage::Review.as_str().to_string()),
                    ),
                };
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt_index as u32,
                backend: route.effective_backend.clone(),
                effective_model: route.effective_model.clone(),
                exit_code,
                validation_result,
                failure_class,
                failure_stage,
                duration_seconds: Some(attempt.duration_secs),
                diff_path: None,
                cli_version: None,
                usage,
            });
            if !fresh_context && !attempt.stdout.trim().is_empty() {
                prior_review_context = utf8_safe_suffix(&attempt.stdout, 20_000).to_string();
            }
            let is_last_attempt = attempt_number + 1 == MAX_REVIEW_ATTEMPTS;
            if !is_last_attempt && review_outcome_allows_reroute(&attempt.outcome) {
                // The invocation slot was released immediately after the
                // backend exited. Availability parsing and route selection
                // therefore happen without holding stale capacity.
                debug_assert!(review_slot.is_none());
                let failure_output = review_failure_output(
                    &attempt.outcome,
                    &attempt.stdout,
                    &attempt.stderr,
                    attempt.idle_timeout_seconds,
                );
                let failure_log = review_failure_log_path(&attempt_session, &attempt.stdout);
                if let Some((parsed, rerouted)) = route_after_backend_unavailable(
                    cfg,
                    profile,
                    &route_request,
                    None,
                    ledger,
                    &route,
                    (&failure_output, &failure_log),
                )? {
                    let current_identity =
                        route_identity(&route.effective_backend, route.effective_model.as_deref());
                    let rerouted_identity = route_identity(
                        &rerouted.effective_backend,
                        rerouted.effective_model.as_deref(),
                    );
                    if rerouted_identity != current_identity {
                        println!(
                            "Backend unavailable; retrying review with {} instead of {} ({:?})",
                            route_label(
                                &rerouted.effective_backend,
                                rerouted.effective_model.as_deref(),
                            ),
                            route_label(&route.effective_backend, route.effective_model.as_deref()),
                            parsed.kind
                        );
                        route = rerouted;
                        // A newly selected reviewer gets its own bounded
                        // format-repair opportunity.
                        format_only_repair_count = 0;
                    } else {
                        println!(
                            "Review backend {} was unavailable ({:?}) but no alternate route was selected; retrying the same route (attempt {}/{})",
                            route_label(&route.effective_backend, route.effective_model.as_deref()),
                            parsed.kind,
                            attempt_number + 1,
                            MAX_REVIEW_ATTEMPTS
                        );
                    }
                } else {
                    println!(
                        "Review backend {} failed without an availability signal; retrying the same route (attempt {}/{})",
                        route_label(&route.effective_backend, route.effective_model.as_deref()),
                        attempt_number + 1,
                        MAX_REVIEW_ATTEMPTS
                    );
                }
                review_slot = Some(reserve_review_route(profile, &route)?);
                continue 'attempts;
            }
            if matches!(attempt.outcome, runner::ReviewProcessOutcome::Success) {
                let review_usage = ledger
                    .attempts
                    .last()
                    .map(|attempt| attempt.usage.clone())
                    .unwrap_or_default();
                let reviewer_tier = derive_reviewer_tier(cfg, profile, &route);
                let parsed = parse_review_verdict_with_context(
                    &attempt.stdout,
                    &route,
                    &review_usage,
                    reviewer_tier,
                    &review_gate_context,
                );
                match parsed {
                    Ok(verdict) => {
                        if is_retryable_format_only_violation(
                            &verdict,
                            format_only_repair_count >= REVIEW_FORMAT_REPAIR_ATTEMPTS,
                        ) {
                            format_only_repair_count += 1;
                            should_repair_format = true;
                            ledger.validation_result =
                                Some(REVIEW_FORMAT_ONLY_VIOLATION_REASON.to_string());
                            if let Some(attempt_record) = ledger.attempts.last_mut() {
                                attempt_record.validation_result =
                                    Some(REVIEW_FORMAT_ONLY_VIOLATION_REASON.to_string());
                            }
                            // The just-finished invocation released its slot.
                            // Reacquire before the same reviewer performs the
                            // bounded format-only repair attempt.
                            debug_assert!(review_slot.is_none());
                            review_slot = Some(reserve_review_route(profile, &route)?);
                            // Bounded retry to the same reviewer/route: this
                            // `continue` targets the inner loop only, so it
                            // does not advance `attempt_number` or consume a
                            // MAX_REVIEW_ATTEMPTS slot.
                            continue;
                        }
                        parsed_verdict = Some(Ok(verdict));
                    }
                    Err(err) => parsed_verdict = Some(Err(err)),
                }
            }
            result = Some(attempt);
            break 'attempts;
        }
    }
    let result = result.expect("loop always runs at least one attempt (MAX_REVIEW_ATTEMPTS > 0)");
    ledger.review_idle_timeout_seconds = Some(result.idle_timeout_seconds);
    ledger.review_hard_timeout_seconds = result.hard_timeout_seconds;
    ledger.review_last_progress_secs = result.last_progress_secs;
    println!("Review backend duration: {:.1}s", result.duration_secs);
    let report_path = session_dir.join("review-report.md");
    let verdict_path = session_dir.join("review-verdict.json");
    fs::write(&report_path, &result.stdout)?;
    fs::write(session_dir.join("review-stdout.log"), &result.stdout)?;
    if !result.stderr.trim().is_empty() {
        fs::write(session_dir.join("review-stderr.log"), &result.stderr)?;
    }

    match result.outcome {
        runner::ReviewProcessOutcome::Success => {
            let review_usage = ledger
                .attempts
                .last()
                .map(|attempt| attempt.usage.clone())
                .unwrap_or_default();
            let reviewer_tier = derive_reviewer_tier(cfg, profile, &route);
            // Reuse the verdict parsed inside the attempt loop instead of
            // re-parsing `result.stdout` here: the loop already parses each
            // Success outcome exactly once to decide whether a format-repair
            // retry is needed, and the terminal parse result is carried
            // forward via `parsed_verdict` so it is never parsed twice.
            let mut verdict = match parsed_verdict
                .take()
                .expect("Success outcome always sets parsed_verdict before breaking the loop")
            {
                Ok(mut verdict) => {
                    verdict.applied_capabilities = applied_capabilities.clone();
                    verdict
                }
                Err(err) => {
                    return record_review_output_invalid(
                        cfg,
                        profile,
                        args,
                        ledger,
                        &target,
                        &route,
                        reviewer_tier,
                        &review_usage,
                        &err,
                        &verdict_path,
                        session_dir,
                    );
                }
            };
            // A reviewer asking for human attention (including an APPROVE
            // held by the deterministic evidence gate) gets the next
            // configured second opinion first. Human notification and the
            // dashboard block are reserved for the final, exhausted handoff.
            if verdict.human_required
                && next_escalatory_reviewer(
                    cfg,
                    profile,
                    &args.profile,
                    &target.source_branch,
                    Some((&route.effective_backend, route.effective_model.as_deref())),
                    ledger.review_generation.as_deref(),
                    &availability::resolve_state_path(),
                )
                .is_some()
            {
                verdict.human_required = false;
            }
            fs::write(&verdict_path, serde_json::to_string_pretty(&verdict)?)?;
            println!("{}", result.stdout);
            println!("Written: {}", report_path.display());
            println!("Written: {}", verdict_path.display());
            ledger.backend_exit_code = Some(0);
            ledger.validation_result = Some(verdict.verdict.clone());
            ledger.human_required = verdict.human_required;
            ledger.human_required_reason_code = verdict
                .human_required
                .then(|| HumanRequiredReason::ReviewEvidenceGate.as_str().to_string());
            ledger.confidence_impact = Some(verdict.confidence.clone());
            ledger.review_verdict = Some(verdict.verdict.clone());
            ledger.review_confidence = Some(verdict.confidence.clone());
            ledger.reviewer_backend = Some(route.effective_backend.clone());
            ledger.reviewer_model = route.effective_model.clone();
            ledger.reviewer_tier = Some(reviewer_tier.as_str().to_string());
            ledger.review_gate_reason = verdict.safety_gate_reason.clone();
            ledger.review_blocking_findings = verdict.blocking_findings.clone();
            ledger.review_actionable_findings = verdict.actionable_findings.clone();
            ledger.review_non_blocking_findings = verdict.non_blocking_findings.clone();
            ledger.review_risk_notes = verdict.risk_notes.clone();
            ledger.review_evidence = verdict.evidence.clone();
            ledger.review_compatibility_evidence = verdict.compatibility_evidence.clone();
            ledger.usage = aggregate_attempt_usage(&ledger.attempts);
            if let Some(attempt) = ledger.attempts.last_mut() {
                attempt.validation_result = Some(verdict.verdict.clone());
            }
            // TICKET-125: attribute this verdict back to the branch's
            // implementation entry (the backend that wrote the code being
            // reviewed), not this review dispatch's own entry (the reviewer).
            if let Err(err) = crate::ledger::backfill_review_verdict(
                cfg,
                &target.source_branch,
                crate::ledger::ReviewVerdictBackfill {
                    verdict: &verdict.verdict,
                    confidence: &verdict.confidence,
                    reviewer_backend: &route.effective_backend,
                    reviewer_model: route.effective_model.as_deref(),
                    reviewer_tier: verdict.reviewer_tier.as_deref(),
                    review_gate_reason: verdict.safety_gate_reason.as_deref(),
                    review_source_sha: ledger.review_source_sha.as_deref(),
                    review_metadata_fingerprint: ledger.review_metadata_fingerprint.as_deref(),
                    review_contract_version: ledger.review_contract_version,
                    review_generation: ledger.review_generation.as_deref(),
                    blocking_findings: &verdict.blocking_findings,
                    actionable_findings: &verdict.actionable_findings,
                    non_blocking_findings: &verdict.non_blocking_findings,
                    risk_notes: &verdict.risk_notes,
                    evidence: &verdict.evidence,
                    compatibility_evidence: &verdict.compatibility_evidence,
                },
            ) {
                eprintln!(
                    "warning: failed to backfill review verdict onto ledger: {:#}",
                    err
                );
            }
            // Resolve the MR/PR URL this verdict applies to so notifications
            // can reference it. Failure to resolve is non-fatal here.
            let mr_url = provider::mr_url_for_branch(profile, &target.source_branch);
            notify_event(
                cfg,
                profile,
                NotifyEvent::ReviewVerdict {
                    verdict: &verdict.verdict,
                    mr_url: mr_url.as_deref().unwrap_or("unknown"),
                },
            );
            if verdict.human_required {
                ledger.human_required_reason_code =
                    Some(HumanRequiredReason::ReviewEvidenceGate.as_str().to_string());
                notify_event(
                    cfg,
                    profile,
                    NotifyEvent::HumanRequired {
                        reason: "review verdict requires human attention",
                        reference: mr_url.as_deref(),
                        reason_code: Some(HumanRequiredReason::ReviewEvidenceGate.as_str()),
                        failure_class: ledger.failure_class.as_deref().unwrap_or("human_blocked"),
                        failure_stage: ledger.failure_stage.as_deref(),
                        error_summary: ledger.error_summary.as_deref(),
                        attempt_count: ledger.attempts_started,
                        mr_url: mr_url.as_deref().or(Some(target.source_branch.as_str())),
                    },
                );
            }
            let mr_body = render_review_comment(&verdict, session_dir);
            let labels = review_labels(&verdict);
            if profile.provider == "gitlab" {
                match provider::gitlab_find_mr_by_branch(profile, &target.source_branch) {
                    Ok(mr) => println!("Resolved MR: {}", mr.url),
                    Err(err) => {
                        eprintln!("warning: failed to resolve GitLab MR for branch: {:#}", err)
                    }
                }
            }
            // TICKET-128: a restricted profile forbids agent-authored issue/MR
            // comments. The reviewer still ran and produced a deterministic
            // verdict (APPROVE/REJECT) retained locally; we simply do not
            // publish it to the tracker. This is independent of reviewer
            // routing and merge policy.
            if !profile.publishing.allow_issue_comments {
                println!(
                    "Publishing policy forbids agent-authored issue/MR comments; review verdict ({} confidence={}) written locally only.",
                    verdict.verdict, verdict.confidence
                );
            } else {
                provider::post_review_comment(profile, &target.source_branch, &mr_body, &labels)
                    .context("publishing review comment and labels")?;
            }
            if verdict.human_required {
                println!("Review requires human attention.");
            }
        }
        runner::ReviewProcessOutcome::ExecutableUnavailable => {
            let summary = review_terminal_failure_summary(ledger, "review backend unavailable");
            ledger.error_summary = Some(summary.clone());
            ledger.set_failure(
                crate::ledger::FailureClass::EnvironmentError,
                crate::ledger::FailureStage::Review,
            );
            ledger.validation_result = Some("not_run".into());
            println!("Review backend is unavailable.");
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("{summary}")
        }
        runner::ReviewProcessOutcome::SpawnFailure => {
            let summary = review_terminal_failure_summary(
                ledger,
                &format!("review backend launch failed: {}", result.stderr.trim()),
            );
            ledger.error_summary = Some(summary.clone());
            ledger.set_failure(
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::BackendLaunch,
            );
            ledger.validation_result = Some("not_run".into());
            println!("Review backend failed to launch.");
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("{summary}")
        }
        runner::ReviewProcessOutcome::NonZeroExit(code) => {
            let summary = review_terminal_failure_summary(
                ledger,
                &format!("review backend exited with status {code}"),
            );
            ledger.error_summary = Some(summary.clone());
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::Review,
            );
            ledger.backend_exit_code = Some(code);
            ledger.validation_result = Some("not_run".into());
            println!("Review backend exited with status {}.", code);
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("{summary}")
        }
        runner::ReviewProcessOutcome::SignalTermination(signal) => {
            let summary = review_terminal_failure_summary(
                ledger,
                &format!(
                    "shutdown requested while {} was running (signal {signal})",
                    route.effective_backend
                ),
            );
            ledger.error_summary = Some(summary.clone());
            mark_review_shutdown_cancelled(ledger, signal);
            println!(
                "Review shutdown requested; terminated backend process group (signal {}).",
                signal
            );
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("{summary}")
        }
        runner::ReviewProcessOutcome::CleanupFailure(error) => {
            let summary = review_terminal_failure_summary(
                ledger,
                &format!("review backend descendant cleanup failed: {error}"),
            );
            ledger.set_failure(
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::Review,
            );
            ledger.backend_exit_code =
                Some(crate::runner::process::PROCESS_CLEANUP_FAILED_EXIT_CODE);
            ledger.validation_result = Some("not_run_process_cleanup_failed".into());
            ledger.error_summary = Some(summary.clone());
            println!("Review backend descendant cleanup failed: {error}");
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("{summary}")
        }
        runner::ReviewProcessOutcome::IdleTimeout => {
            let summary = review_terminal_failure_summary(
                ledger,
                &format!(
                    "review backend stalled (no progress) after {}s idle budget",
                    result.idle_timeout_seconds
                ),
            );
            ledger.error_summary = Some(summary.clone());
            ledger.set_failure(
                crate::ledger::FailureClass::AgentNoProgress,
                crate::ledger::FailureStage::Review,
            );
            ledger.validation_result = Some("not_run_idle_timeout".into());
            ledger.review_timeout_class = Some("idle".to_string());
            println!(
                "Review backend stalled (no progress) and was killed after the {}s idle budget.",
                result.idle_timeout_seconds
            );
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("{summary}")
        }
        runner::ReviewProcessOutcome::HardTimeout => {
            // A healthy reviewer exceeded the explicit hard safety ceiling. This
            // is NOT a backend failure and must not trigger retry/escalation to
            // a paid reviewer (issue #540).
            let summary = review_terminal_failure_summary(
                ledger,
                &format!(
                    "review backend exceeded the {}s hard safety ceiling",
                    result
                        .hard_timeout_seconds
                        .unwrap_or(result.idle_timeout_seconds)
                ),
            );
            ledger.error_summary = Some(summary.clone());
            ledger.set_failure(
                crate::ledger::FailureClass::HumanBlocked,
                crate::ledger::FailureStage::Review,
            );
            ledger.validation_result = Some("not_run_hard_timeout".into());
            ledger.human_required = true;
            ledger.human_required_reason_code = Some(
                HumanRequiredReason::ReviewCeilingExhausted
                    .as_str()
                    .to_string(),
            );
            ledger.review_timeout_class = Some("hard".to_string());
            if let Some(hard) = result.hard_timeout_seconds {
                println!(
                    "Review backend hit the {}s hard safety ceiling (still making progress); not treating as a backend failure.",
                    hard
                );
            }
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("{summary}")
        }
    }
    Ok(())
}

const REVIEW_MR_TITLE_MAX_BYTES: usize = 1_024;
const REVIEW_MR_BODY_MAX_BYTES: usize = 16_384;
const REVIEW_PRIOR_STATE_MAX_BYTES: usize = 8_192;
const REVIEW_FORMAT_REPAIR_INSTRUCTIONS: &str =
    "Retrying: respond with ONLY the inert heading `Review notes` followed by the JSON object. \
Include no extra prose before or after the JSON.";

fn mark_review_budget_exhausted(ledger: &mut LedgerEntry, route: &RouteDecision, reason: &str) {
    apply_route_to_ledger(ledger, route);
    ledger.set_failure(
        crate::ledger::FailureClass::HumanBlocked,
        crate::ledger::FailureStage::Review,
    );
    ledger.validation_result = Some("review_budget_exhausted".into());
    ledger.human_required = true;
    ledger.human_required_reason_code = Some(
        HumanRequiredReason::RetryBudgetExhausted
            .as_str()
            .to_string(),
    );
    ledger.error_summary = Some(reason.to_string());
}

fn mark_review_shutdown_cancelled(ledger: &mut LedgerEntry, signal: i32) {
    mark_shutdown_cancelled(ledger, crate::ledger::FailureStage::Review, Some(-signal));
    // apply_route_to_ledger may have marked an availability fallback as
    // low-confidence/human-required before the backend started. A killed
    // process produced no verdict, so neither flag is true.
    ledger.confidence_impact = None;
    ledger.human_required = false;
    ledger.human_required_reason_code = None;
}

fn reserve_review_route(profile: &Profile, route: &RouteDecision) -> Result<ConcurrencyGuard> {
    reserve_backend_slot(profile, &route.identity)
}

fn review_attempt_environment(
    profile: &Profile,
    backend: &str,
    base: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env_vars = base.to_vec();
    apply_backend_instance_env(profile, backend, &mut env_vars);
    env_vars
}

#[allow(clippy::too_many_arguments)]
fn record_review_output_invalid(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    ledger: &mut LedgerEntry,
    target: &ReviewTarget,
    route: &RouteDecision,
    reviewer_tier: super::super::review::policy::ReviewerTier,
    review_usage: &crate::ledger::LedgerUsage,
    err: &anyhow::Error,
    verdict_path: &Path,
    session_dir: &Path,
) -> Result<()> {
    let raw_reason = review_output_invalid_error(err)
        .map(|invalid| invalid.reason().to_string())
        .unwrap_or_else(|| err.to_string());
    let reason = crate::redact::redact(utf8_safe_prefix(&raw_reason, 600));
    let verdict = "REVIEW_OUTPUT_INVALID";

    ledger.set_failure(
        crate::ledger::FailureClass::ReviewOutputInvalid,
        crate::ledger::FailureStage::Review,
    );
    ledger.backend_exit_code = Some(0);
    ledger.validation_result = Some("review_output_invalid".into());
    ledger.human_required = false;
    ledger.human_required_reason_code = None;
    ledger.error_summary = Some(reason.clone());
    ledger.confidence_impact = Some("unknown".into());
    ledger.review_verdict = Some(verdict.into());
    ledger.review_confidence = Some("unknown".into());
    ledger.reviewer_backend = Some(route.effective_backend.clone());
    ledger.reviewer_model = route.effective_model.clone();
    ledger.reviewer_tier = Some(reviewer_tier.as_str().to_string());
    ledger.reviewer_class = Some(reviewer_dedup_class(reviewer_tier, route));
    ledger.review_gate_reason = Some(reason.clone());
    ledger.review_blocking_findings.clear();
    ledger.review_actionable_findings.clear();
    ledger.review_non_blocking_findings.clear();
    ledger.review_risk_notes.clear();
    ledger.review_evidence.clear();
    ledger.review_compatibility_evidence.clear();
    ledger.usage = aggregate_attempt_usage(&ledger.attempts);
    if let Some(attempt) = ledger.attempts.last_mut() {
        attempt.validation_result = Some("review_output_invalid".into());
        attempt.failure_class = Some(
            crate::ledger::FailureClass::ReviewOutputInvalid
                .as_str()
                .into(),
        );
        attempt.failure_stage = Some(crate::ledger::FailureStage::Review.as_str().into());
    }

    fs::write(
        verdict_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "verdict": verdict,
            "confidence": "unknown",
            "human_required": false,
            "actionable_findings": [],
            "review_output_invalid_reason": reason,
            "reviewer_backend": route.effective_backend,
            "reviewer_model": route.effective_model,
            "reviewer_tier": reviewer_tier.as_str(),
            "usage": review_usage,
        }))?,
    )?;

    if let Err(backfill_err) = crate::ledger::backfill_review_verdict(
        cfg,
        &target.source_branch,
        crate::ledger::ReviewVerdictBackfill {
            verdict,
            confidence: "unknown",
            reviewer_backend: &route.effective_backend,
            reviewer_model: route.effective_model.as_deref(),
            reviewer_tier: Some(reviewer_tier.as_str()),
            review_gate_reason: Some(&reason),
            review_source_sha: ledger.review_source_sha.as_deref(),
            review_metadata_fingerprint: ledger.review_metadata_fingerprint.as_deref(),
            review_contract_version: ledger.review_contract_version,
            review_generation: ledger.review_generation.as_deref(),
            blocking_findings: &[],
            actionable_findings: &[],
            non_blocking_findings: &[],
            risk_notes: &[],
            evidence: &[],
            compatibility_evidence: &[],
        },
    ) {
        eprintln!(
            "warning: failed to backfill invalid review output onto ledger: {backfill_err:#}"
        );
    }

    let mr_url = provider::mr_url_for_branch(profile, &target.source_branch)
        .or_else(|| target.mr_url.clone())
        .unwrap_or_else(|| target.source_branch.clone());
    notify_event(
        cfg,
        profile,
        NotifyEvent::ReviewOutputInvalid {
            mr_url: &mr_url,
            backend: &route.effective_backend,
            model: route.effective_model.as_deref().unwrap_or("default"),
            reason: &reason,
        },
    );

    if profile.publishing.allow_issue_comments {
        let body = format!(
            "GAH rejected this reviewer response as unsafe repair context: `{reason}`. No FixMr was dispatched. The next configured reviewer will be tried within the bounded review budget.\n\nSession: `{}`",
            session_dir.display()
        );
        provider::post_review_comment(
            profile,
            &target.source_branch,
            &body,
            &["gah-review-escalating"],
        )
        .context("publishing invalid-review reroute state")?;
    } else {
        println!(
            "Publishing policy forbids review comments; invalid output retained locally for profile {}.",
            args.profile
        );
    }
    println!("Review output invalid; queued for bounded reviewer reroute: {reason}");
    Ok(())
}

fn stop_for_exhausted_review_escalation(
    cfg: &GahConfig,
    profile: &Profile,
    ledger: &mut LedgerEntry,
    target: &ReviewTarget,
    reason: &str,
) -> Result<()> {
    let invalid_output_exhausted = reason == "review_output_invalid";
    let message = if invalid_output_exhausted {
        let chain = invalid_review_attempt_chain(cfg, profile, ledger, &target.source_branch);
        format!(
            "review output validation exhausted; no untried configured reviewer remains; attempts: {chain}"
        )
    } else {
        format!(
            "review escalation exhausted after {reason}; no untried escalatory reviewer remains"
        )
    };
    ledger.set_failure(
        crate::ledger::FailureClass::HumanBlocked,
        crate::ledger::FailureStage::Review,
    );
    ledger.validation_result = Some(if invalid_output_exhausted {
        "review_output_invalid_exhausted".into()
    } else {
        "review_escalation_exhausted".into()
    });
    ledger.review_verdict = Some("HUMAN_REVIEW".into());
    ledger.human_required = true;
    let reason_code = if invalid_output_exhausted {
        HumanRequiredReason::ReviewOutputInvalidExhausted
    } else {
        HumanRequiredReason::ReviewEvidenceGate
    };
    ledger.human_required_reason_code = Some(reason_code.as_str().to_string());
    ledger.error_summary = Some(message.clone());
    notify_event(
        cfg,
        profile,
        NotifyEvent::HumanRequired {
            reason: if invalid_output_exhausted {
                "review output validation exhausted"
            } else {
                "review escalation exhausted"
            },
            reference: target.mr_url.as_deref(),
            reason_code: Some(reason_code.as_str()),
            failure_class: ledger.failure_class.as_deref().unwrap_or("human_blocked"),
            failure_stage: ledger.failure_stage.as_deref(),
            error_summary: ledger.error_summary.as_deref(),
            attempt_count: ledger.attempts_started,
            mr_url: target
                .mr_url
                .as_deref()
                .or(Some(target.source_branch.as_str())),
        },
    );
    if profile.publishing.allow_issue_comments {
        provider::post_review_comment(
            profile,
            &target.source_branch,
            &format!("GAH review handoff: `{message}`"),
            &["gah-human-review"],
        )?;
    }
    bail!("{message}")
}

fn invalid_review_attempt_chain(
    cfg: &GahConfig,
    profile: &Profile,
    ledger: &LedgerEntry,
    branch: &str,
) -> String {
    let mut attempts = crate::ledger::read_entries(cfg)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            entry.profile == ledger.profile
                && entry.repo_id == profile.repo_id
                && entry.mode == "review"
                && entry.branch.as_deref() == Some(branch)
                && entry.review_contract_version.unwrap_or(0)
                    >= crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION
                && entry.validation_result.as_deref() == Some("review_output_invalid")
        })
        .map(|entry| {
            let route = route_label(&entry.effective_backend, entry.effective_model.as_deref());
            let reason = entry
                .review_gate_reason
                .as_deref()
                .or(entry.error_summary.as_deref())
                .unwrap_or("unknown invalid-output reason");
            format!("{route} ({})", utf8_safe_prefix(reason, 120))
        })
        .collect::<Vec<_>>();
    if ledger.validation_result.as_deref() == Some("review_output_invalid") {
        let route = route_label(&ledger.effective_backend, ledger.effective_model.as_deref());
        let reason = ledger
            .review_gate_reason
            .as_deref()
            .or(ledger.error_summary.as_deref())
            .unwrap_or("unknown invalid-output reason");
        attempts.push(format!("{route} ({})", utf8_safe_prefix(reason, 120)));
    }
    if attempts.is_empty() {
        "none recorded".to_string()
    } else {
        attempts.join(" -> ")
    }
}

#[cfg(test)]
#[path = "review/reservation_tests.rs"]
mod reservation_tests;
