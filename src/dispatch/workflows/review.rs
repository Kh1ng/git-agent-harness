use super::super::attempts::{
    apply_route_to_ledger, decide_route, mark_backend_unavailable_from_output,
    mark_shutdown_cancelled, record_route_attempt, reserve_backend_slot, review_preflight,
    review_usage, route_identity, route_label,
};
use super::super::prompts::enforce_context_budget;
use super::super::publish::{render_review_comment, review_labels};
use super::super::review::context::{
    lookup_review_state_by_branch, prepare_review_diff, resolve_review_target, ReviewTarget,
};
use super::super::review::policy::{
    check_review_budget, derive_reviewer_tier, next_escalatory_reviewer,
    parse_review_verdict_with_context, review_escalation_reason, reviewer_dedup_class,
    ReviewBudgetExhausted, ReviewGateContext,
};
use super::super::text::{utf8_safe_prefix, utf8_safe_suffix};
use super::super::DispatchArgs;
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
use std::fs;
use std::path::Path;

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
    if target.prior_state.is_none() {
        target.prior_state =
            lookup_review_state_by_branch(cfg, &args.profile, &target.source_branch);
    }
    let diff_bundle = prepare_review_diff(repo, profile, &mut target)?;
    let bundle = session_dir.join("review-bundle");
    fs::create_dir_all(&bundle)?;
    fs::write(bundle.join("diff.patch"), &diff_bundle.diff)?;
    fs::write(bundle.join("changed-files.txt"), &diff_bundle.files)?;
    fs::write(
        bundle.join("mr-description.md"),
        format!(
            "MR: {}\nURL: {}\nSource: {}\nTarget: {}\nSource SHA: {}\nTarget SHA: {}\nRepo: {}\nTitle: {}\nCI: {}",
            target.mr_id.as_deref().unwrap_or("n/a"),
            target.mr_url.as_deref().unwrap_or("n/a"),
            target.source_branch,
            target.target_branch,
            target.source_sha.as_deref().unwrap_or("unknown"),
            target.target_sha.as_deref().unwrap_or("unknown"),
            profile.repo,
            target.mr_title.as_deref().unwrap_or("n/a"),
            target.ci_status.as_deref().unwrap_or("unknown"),
        ),
    )?;
    println!(
        "Diff: {} bytes, files: {}",
        diff_bundle.diff.len(),
        diff_bundle.files.lines().count()
    );
    let review_gate_context =
        ReviewGateContext::from_diff_bundle(&diff_bundle, target.ci_status.as_deref());

    // Everything except the capability-activation prefix is identical
    // regardless of which backend ends up running the review.
    let prompt_suffix = format!(
        "## Review Pack\n\n\
         Review this diff for correctness, test coverage, and safety. \
         Return a JSON object. You may precede it only with the inert heading `Review notes`; put every substantive finding in the JSON arrays, never in prose.\n\
         The JSON object fields are: verdict, confidence, human_required, blocking_findings, non_blocking_findings, risk_notes, evidence, compatibility_evidence.\n\
         blocking_findings, non_blocking_findings, risk_notes, evidence, and compatibility_evidence must be JSON arrays of strings, even when empty or when only one item exists.\n\
         For an APPROVE, evidence must include exactly one or more file:<changed-path> entries copied from Changed files below. You may include ci:passed only when the displayed control-plane CI status is passed. An APPROVE without grounded file evidence is invalid.\n\
         If a contract surface is changed, do not APPROVE unless compatibility_evidence includes file:<changed-contract-path> and mechanism:<schema-version|backward-compatible-default|migration> that is actually present in the diff.\n\
         Verdict must be one of APPROVE, NEEDS_FIX, REJECT, HUMAN_REVIEW, defined as:\n\
         - APPROVE: you believe the change is correct, safe, and complete enough to merge. Report your ACTUAL confidence honestly in the separate `confidence` field (high/medium/low) -- do not inflate confidence to sound more certain, and do not downgrade to NEEDS_FIX just to hedge when you'd otherwise approve. A low-confidence approval is a real, useful signal (insufficient context, a domain you couldn't fully verify, a partial review) and will correctly route to a human -- it is not a failure to be avoided.\n\
         - NEEDS_FIX: you found a concrete, real problem that should be fixed before merge. Put it in blocking_findings, even if it isn't an immediate crash -- e.g. silent data loss, a hidden failure mode, or anything that would take real effort to diagnose later if left in. Do not downgrade a genuine risk into non_blocking_findings/risk_notes just because it wouldn't break the build today.\n\
         - REJECT: the change is fundamentally wrong and should not be merged as-is.\n\
         - HUMAN_REVIEW: you cannot make a confident recommendation at all.\n\
         Repo: {}. MR: {}. Source: {}. Target: {}. CI status: {}.\n\
         MR title: {}\nMR body:\n{}\n\
         Prior run state:\n{}\n\n## Diff\n\n```\n{}\n```\nChanged files:\n{}",
        profile.repo,
        target.mr_id.as_deref().unwrap_or("n/a"),
        target.source_branch,
        target.target_branch,
        target.ci_status.as_deref().unwrap_or("unknown"),
        target.mr_title.as_deref().unwrap_or("n/a"),
        target.mr_body.as_deref().unwrap_or("n/a"),
        target.prior_state.as_deref().unwrap_or("not found"),
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
    let escalation_reason =
        review_escalation_reason(cfg, profile, &args.profile, &target.source_branch);
    let next_escalatory = escalation_reason.and_then(|_| {
        next_escalatory_reviewer(cfg, profile, &args.profile, &target.source_branch, None)
    });
    let (requested_backend, requested_model) = match (escalation_reason, next_escalatory.as_ref()) {
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

    let mut route = decide_route(
        cfg,
        profile,
        RouteRequest {
            last_failure_class: None,
            mode: "review",
            requested_backend,
            requested_model,
            recommended_backend: None,
            recommended_model: None,
            session_id: session_dir.file_name().and_then(|s| s.to_str()),
            usage_summary: None,
        },
        None,
        ledger,
    )?;

    // Duplicate-review short-circuit runs before the budget check: if nothing
    // has changed since the last completed review of the same tier, that is
    // the operator-relevant reason to skip, not a budget refusal, and it must
    // not consume any part of the review-cycle budget below.
    let reviewer_class = reviewer_dedup_class(derive_reviewer_tier(cfg, profile, &route), &route);
    if let (Some(work_id), Some(source_sha)) =
        (ledger.work_id.as_deref(), target.source_sha.as_deref())
    {
        if crate::ledger::review_already_exists(cfg, work_id, source_sha, &reviewer_class)? {
            ledger.validation_result = Some("skipped_duplicate_review".into());
            ledger.review_source_sha = Some(source_sha.to_string());
            ledger.reviewer_class = Some(reviewer_class.to_string());
            println!("Skipping duplicate {reviewer_class} review for {work_id} at {source_sha}");
            return Ok(());
        }
    }
    ledger.review_source_sha = target.source_sha.clone();
    ledger.reviewer_class = Some(reviewer_class.to_string());

    if let Some(block) =
        check_review_budget(cfg, profile, &args.profile, args.work_id.as_deref(), &route)?
    {
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
    let mut applied_capabilities = vec![];
    let mut prior_review_context = String::new();
    let mut result = None;
    for attempt_number in 0..MAX_REVIEW_ATTEMPTS {
        ledger.attempts_started = Some(ledger.attempts_started.unwrap_or(0) + 1);
        apply_route_to_ledger(ledger, &route);
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
        let mut prompt = format!("{capability_prefix}{prompt_suffix}");
        if !fresh_context && !prior_review_context.is_empty() {
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

        let attempt_session = session_dir.join(format!("review-attempt-{}", attempt_number + 1));
        fs::create_dir_all(&attempt_session)?;
        record_route_attempt(ledger, &route);
        let attempt = runner::run_review_backend(
            profile,
            &route.effective_backend,
            repo,
            &prompt,
            &attempt_session,
            route.effective_model.as_deref(),
            &env_vars,
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
                attribution,
                profile.claude_path.as_deref(),
            )
        };
        let (exit_code, validation_result, failure_class, failure_stage) = match &attempt.outcome {
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
            runner::ReviewProcessOutcome::Timeout => (
                None,
                Some("not_run_timeout".to_string()),
                Some(
                    crate::ledger::FailureClass::BackendError
                        .as_str()
                        .to_string(),
                ),
                Some(crate::ledger::FailureStage::Review.as_str().to_string()),
            ),
        };
        ledger.attempts.push(crate::ledger::AttemptRecord {
            attempt_number: attempt_number as u32 + 1,
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
        if !is_last_attempt {
            if let runner::ReviewProcessOutcome::NonZeroExit(_) = attempt.outcome {
                // Provider CLIs commonly put quota/auth diagnostics on stderr
                // while keeping stdout empty.  Routing availability must see
                // both streams or a failed reviewer remains eligible and is
                // selected again on the next loop cycle.
                let failure_output = if attempt.stderr.trim().is_empty() {
                    attempt.stdout.clone()
                } else if attempt.stdout.trim().is_empty() {
                    attempt.stderr.clone()
                } else {
                    format!("{}\n{}", attempt.stdout, attempt.stderr)
                };
                let failure_log = if attempt.stdout.trim().is_empty() {
                    attempt_session.join("review-stderr.log")
                } else {
                    attempt_session.join("review-stdout.log")
                };
                if let Some(parsed) = mark_backend_unavailable_from_output(
                    &route.effective_backend,
                    route.effective_model.as_deref(),
                    None,
                    &failure_output,
                    &failure_log.display().to_string(),
                )? {
                    let rerouted = decide_route(
                        cfg,
                        profile,
                        RouteRequest {
                            last_failure_class: None,
                            mode: "review",
                            requested_backend: config::canonical_backend_name(&args.backend),
                            requested_model: args.model.as_deref(),
                            recommended_backend: None,
                            recommended_model: None,
                            session_id: session_dir.file_name().and_then(|s| s.to_str()),
                            usage_summary: None,
                        },
                        None,
                        ledger,
                    )?;
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
                            route_label(&route.effective_backend, route.effective_model.as_deref(),),
                            parsed.kind
                        );
                        route = rerouted;
                        review_slot = Some(reserve_review_route(profile, &route)?);
                        continue;
                    }
                }
            }
        }
        result = Some(attempt);
        break;
    }
    let result = result.expect("loop always runs at least one attempt (MAX_REVIEW_ATTEMPTS > 0)");
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
            let mut verdict = match parse_review_verdict_with_context(
                &result.stdout,
                &route,
                &review_usage,
                reviewer_tier,
                &review_gate_context,
            ) {
                Ok(mut verdict) => {
                    verdict.applied_capabilities = applied_capabilities.clone();
                    verdict
                }
                Err(err) => {
                    ledger.set_failure(
                        crate::ledger::FailureClass::BackendError,
                        crate::ledger::FailureStage::Review,
                    );
                    ledger.backend_exit_code = Some(0);
                    ledger.validation_result = Some("invalid_output".into());
                    if let Some(attempt) = ledger.attempts.last_mut() {
                        attempt.validation_result = Some("invalid_output".into());
                        attempt.failure_class =
                            Some(crate::ledger::FailureClass::BackendError.as_str().into());
                        attempt.failure_stage =
                            Some(crate::ledger::FailureStage::Review.as_str().into());
                    }
                    return Err(err);
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
                    blocking_findings: &verdict.blocking_findings,
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
            ledger.set_failure(
                crate::ledger::FailureClass::EnvironmentError,
                crate::ledger::FailureStage::Review,
            );
            ledger.validation_result = Some("not_run".into());
            println!("Review backend is unavailable.");
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("review backend is unavailable")
        }
        runner::ReviewProcessOutcome::SpawnFailure => {
            ledger.set_failure(
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::BackendLaunch,
            );
            ledger.validation_result = Some("not_run".into());
            println!("Review backend failed to launch.");
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("review backend failed to launch: {}", result.stderr.trim())
        }
        runner::ReviewProcessOutcome::NonZeroExit(code) => {
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::Review,
            );
            ledger.backend_exit_code = Some(code);
            ledger.validation_result = Some("not_run".into());
            println!("Review backend exited with status {}.", code);
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("review backend exited with status {code}")
        }
        runner::ReviewProcessOutcome::SignalTermination(signal) => {
            mark_review_shutdown_cancelled(ledger, signal);
            println!(
                "Review shutdown requested; terminated backend process group (signal {}).",
                signal
            );
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!(
                "shutdown requested while {} was running",
                route.effective_backend
            )
        }
        runner::ReviewProcessOutcome::Timeout => {
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::Review,
            );
            ledger.validation_result = Some("not_run".into());
            println!(
                "Review backend timed out after {} seconds.",
                profile.review_timeout_seconds()
            );
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!(
                "review backend timed out after {} seconds",
                profile.review_timeout_seconds()
            )
        }
    }
    Ok(())
}

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
    reserve_backend_slot(
        profile,
        &route.effective_backend,
        route.effective_model.as_deref(),
    )
}

fn stop_for_exhausted_review_escalation(
    cfg: &GahConfig,
    profile: &Profile,
    ledger: &mut LedgerEntry,
    target: &ReviewTarget,
    reason: &str,
) -> Result<()> {
    let message = format!(
        "review escalation exhausted after {reason}; no untried escalatory reviewer remains"
    );
    ledger.set_failure(
        crate::ledger::FailureClass::HumanBlocked,
        crate::ledger::FailureStage::Review,
    );
    ledger.validation_result = Some("review_escalation_exhausted".into());
    ledger.review_verdict = Some("HUMAN_REVIEW".into());
    ledger.human_required = true;
    ledger.human_required_reason_code =
        Some(HumanRequiredReason::ReviewEvidenceGate.as_str().to_string());
    ledger.error_summary = Some(message.clone());
    notify_event(
        cfg,
        profile,
        NotifyEvent::HumanRequired {
            reason: "review escalation exhausted",
            reference: target.mr_url.as_deref(),
            reason_code: Some(HumanRequiredReason::ReviewEvidenceGate.as_str()),
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

#[cfg(test)]
mod reservation_tests {
    use super::{
        mark_review_budget_exhausted, mark_review_shutdown_cancelled, reserve_review_route,
    };
    use crate::config::tests::test_profile_for_notifications;
    use crate::ledger::LedgerEntry;
    use crate::routing::{current_concurrent, RouteDecision};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Barrier};

    #[test]
    fn shutdown_clears_provisional_fallback_confidence_and_human_hold() {
        let profile = test_profile_for_notifications();
        let mut ledger = LedgerEntry::new("gah", &profile, "claude", "review", "test", None, None);
        ledger.confidence_impact = Some("low".into());
        ledger.human_required = true;

        mark_review_shutdown_cancelled(&mut ledger, 15);

        assert_eq!(
            ledger.validation_result.as_deref(),
            Some("cancelled_shutdown")
        );
        assert_eq!(ledger.failure_class.as_deref(), Some("harness_error"));
        assert_eq!(ledger.failure_stage.as_deref(), Some("review"));
        assert_eq!(ledger.backend_exit_code, Some(-15));
        assert_eq!(ledger.confidence_impact, None);
        assert!(!ledger.human_required);
    }

    #[test]
    fn route_attribution_does_not_clear_a_review_budget_hold() {
        let profile = test_profile_for_notifications();
        let mut ledger = LedgerEntry::new("gah", &profile, "vibe", "review", "test", None, None);
        let route = RouteDecision {
            requested_backend: "vibe".into(),
            effective_backend: "vibe".into(),
            requested_model: Some("reviewer".into()),
            effective_model: Some("reviewer".into()),
            effective_quota_pool: None,
            routing_reason: "test".into(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        };

        mark_review_budget_exhausted(&mut ledger, &route, "budget exhausted");

        assert!(ledger.human_required);
        assert_eq!(
            ledger.human_required_reason_code.as_deref(),
            Some("retry_budget_exhausted")
        );
        assert_eq!(ledger.failure_class.as_deref(), Some("human_blocked"));
        assert_eq!(ledger.failure_stage.as_deref(), Some("review"));
    }

    #[test]
    fn three_reviews_never_overlap_on_a_backend_model_capped_at_one() {
        let backend = format!("review-reservation-test-{}", std::process::id());
        let model = "sonnet-test";
        let mut profile = test_profile_for_notifications();
        profile
            .max_concurrent_per_model
            .insert(format!("{backend}/{model}"), 1);
        let route = RouteDecision {
            requested_backend: backend.clone(),
            effective_backend: backend.clone(),
            requested_model: Some(model.into()),
            effective_model: Some(model.into()),
            effective_quota_pool: None,
            routing_reason: "test".into(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        };

        let profile = Arc::new(profile);
        let route = Arc::new(route);
        let start = Arc::new(Barrier::new(3));
        let max_seen = Arc::new(AtomicU32::new(0));
        let workers: Vec<_> = (0..3)
            .map(|_| {
                let profile = Arc::clone(&profile);
                let route = Arc::clone(&route);
                let start = Arc::clone(&start);
                let max_seen = Arc::clone(&max_seen);
                std::thread::spawn(move || {
                    start.wait();
                    let _slot = reserve_review_route(&profile, &route).unwrap();
                    max_seen.fetch_max(
                        current_concurrent(
                            &route.effective_backend,
                            route.effective_model.as_deref(),
                        ),
                        Ordering::SeqCst,
                    );
                    std::thread::sleep(std::time::Duration::from_millis(25));
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }

        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
        assert_eq!(current_concurrent(&backend, Some(model)), 0);
    }
}
