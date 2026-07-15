use super::super::attempts::{
    apply_route_to_ledger, decide_route, mark_backend_unavailable_from_output,
    mark_shutdown_cancelled, record_route_attempt, reserve_backend_slot, review_preflight,
    review_usage, route_identity, route_label,
};
use super::super::issues::{
    extract_markdown_section, fetch_issue_details, parse_ticket_metadata_from_issue, IssueDetails,
};
use super::super::prompts::{enforce_context_budget, indent_untrusted_text};
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
use crate::ledger::{self, LedgerEntry};
use crate::notifications::{notify_event, NotifyEvent};
use crate::routing::{ConcurrencyGuard, RouteDecision, RouteRequest};
use crate::usage_attribution::{
    aggregate_attempt_usage, normalize_attempt_usage, UsageAttribution,
};
use crate::{provider, runner};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

mod source_issue_sections;

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
    let source_issue_context = resolve_source_issue_context(
        cfg,
        profile,
        &args.profile,
        ledger.work_id.as_deref(),
        &target,
    )?;
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
         {}\n\
         ## Prior Run State\n\n{}\n\n## Diff\n\n```\n{}\n```\nChanged files:\n{}",
        profile.repo,
        target.mr_id.as_deref().unwrap_or("n/a"),
        target.source_branch,
        target.target_branch,
        target.ci_status.as_deref().unwrap_or("unknown"),
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
        if let Some(contract) =
            verified_post_budget_source_contract(source_issue_context.contract.as_deref(), &prompt)?
        {
            fs::write(bundle.join("source-issue-contract.md"), contract)?;
        }

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

const SOURCE_ISSUE_TITLE_MAX_BYTES: usize = 1_024;
const SOURCE_ISSUE_PROBLEM_MAX_BYTES: usize = 4_096;
const SOURCE_ISSUE_ACCEPTANCE_MAX_BYTES: usize = 8_192;
const SOURCE_ISSUE_LIST_MAX_BYTES: usize = 4_096;
const SOURCE_ISSUE_LIST_ITEM_MAX_BYTES: usize = 1_024;
const SOURCE_ISSUE_DETAIL_MAX_BYTES: usize = 3_072;
const SOURCE_ISSUE_DETAILS_MAX_BYTES: usize = 12_288;
const SOURCE_ISSUE_FALLBACK_MAX_BYTES: usize = 12_000;
const REVIEW_MR_TITLE_MAX_BYTES: usize = 1_024;
const REVIEW_MR_BODY_MAX_BYTES: usize = 16_384;
const REVIEW_PRIOR_STATE_MAX_BYTES: usize = 8_192;

struct SourceIssueIdentity {
    issue_number: String,
    resolved_from: &'static str,
}

struct SourceIssueContext {
    prompt_section: Option<String>,
    contract: Option<String>,
    lookup_report: serde_json::Value,
}

fn render_untrusted_review_text(value: &str, max_bytes: usize) -> String {
    indent_untrusted_text(utf8_safe_prefix(value, max_bytes))
}

fn render_untrusted_inline_review_text(value: &str, max_bytes: usize) -> String {
    utf8_safe_prefix(value, max_bytes)
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}

fn verified_post_budget_source_contract<'a>(
    contract: Option<&'a str>,
    post_budget_prompt: &str,
) -> Result<Option<&'a str>> {
    let Some(contract) = contract else {
        return Ok(None);
    };
    if !post_budget_prompt.contains(contract) {
        anyhow::bail!(
            "post-budget review prompt does not contain the exact canonical source issue contract"
        );
    }
    Ok(Some(contract))
}

fn resolve_source_issue_context(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    work_id: Option<&str>,
    target: &ReviewTarget,
) -> Result<SourceIssueContext> {
    let Some(identity) = resolve_source_issue_identity(cfg, profile_name, work_id, target) else {
        return Ok(missing_source_issue_context());
    };

    match fetch_issue_details(profile, &identity.issue_number) {
        Ok(issue) => {
            let contract = render_source_issue_contract(&issue);
            Ok(SourceIssueContext {
                prompt_section: Some(contract.clone()),
                contract: Some(contract.clone()),
                lookup_report: serde_json::json!({
                    "state": "fetched",
                    "source": identity.resolved_from,
                    "issue_number": identity.issue_number,
                    "contract_bytes": contract.len(),
                }),
            })
        }
        Err(err) => {
            let message = format!(
                "Source issue #{} lookup failed: {err:#}",
                identity.issue_number
            );
            Ok(SourceIssueContext {
                prompt_section: Some(format!("## Source Issue Lookup\n\n{message}")),
                contract: None,
                lookup_report: serde_json::json!({
                    "state": "lookup_failed",
                    "source": identity.resolved_from,
                    "issue_number": identity.issue_number,
                    "error": err.to_string(),
                }),
            })
        }
    }
}

fn missing_source_issue_context() -> SourceIssueContext {
    SourceIssueContext {
        prompt_section: Some(
            "## Source Issue Lookup\n\nSource issue identity could not be resolved from the ledger or MR body; no canonical issue contract was fetched."
                .to_string(),
        ),
        contract: None,
        lookup_report: serde_json::json!({
            "state": "missing",
            "source": "none",
            "issue_number": serde_json::Value::Null,
            "error": "source issue identity not found",
        }),
    }
}

fn resolve_source_issue_identity(
    cfg: &GahConfig,
    profile_name: &str,
    work_id: Option<&str>,
    target: &ReviewTarget,
) -> Option<SourceIssueIdentity> {
    if let Some(work_id) = work_id.filter(|value| !value.trim().is_empty()) {
        if let Ok(entries) = ledger::entries_for_work_id(cfg, work_id) {
            if let Some(issue_number) = entries.into_iter().rev().find_map(|entry| {
                (entry.profile == profile_name && matches!(entry.mode.as_str(), "fix" | "improve"))
                    .then(|| entry.source_issue_number.clone())
                    .flatten()
            }) {
                return Some(SourceIssueIdentity {
                    issue_number,
                    resolved_from: "ledger",
                });
            }
        }
    }

    extract_issue_number_from_text(target.mr_body.as_deref()).map(|issue_number| {
        SourceIssueIdentity {
            issue_number,
            resolved_from: "mr_body",
        }
    })
}

fn extract_issue_number_from_text(text: Option<&str>) -> Option<String> {
    const CLOSING_KEYWORDS: [&str; 9] = [
        "close #",
        "closes #",
        "closed #",
        "fix #",
        "fixes #",
        "fixed #",
        "resolve #",
        "resolves #",
        "resolved #",
    ];
    let text = text?;
    for raw_line in text.lines() {
        let trimmed = raw_line.trim();
        let lowercase = trimmed.to_ascii_lowercase();
        for (start, _) in lowercase.char_indices() {
            let preceding_is_word = lowercase[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if preceding_is_word {
                continue;
            }
            let candidate = &lowercase[start..];
            let Some(keyword) = CLOSING_KEYWORDS
                .iter()
                .find(|keyword| candidate.starts_with(**keyword))
            else {
                continue;
            };
            let rest = &candidate[keyword.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return Some(digits);
            }
        }
    }
    None
}

fn render_source_issue_contract(issue: &IssueDetails) -> String {
    let meta = parse_ticket_metadata_from_issue(issue);
    let unheaded_sections = source_issue_sections::extract(&issue.body);
    let non_label_constraints: Vec<String> = meta
        .constraints
        .iter()
        .filter(|constraint| {
            !issue
                .labels
                .iter()
                .any(|label| label == constraint.as_str())
        })
        .cloned()
        .collect();
    let non_label_affected_files: Vec<String> = meta
        .affected_files
        .iter()
        .filter(|path| !issue.labels.iter().any(|label| label == path.as_str()))
        .cloned()
        .collect();
    let mut acceptance_criteria = meta.acceptance_criteria.clone();
    for criterion in &unheaded_sections.acceptance_criteria {
        if !acceptance_criteria.contains(criterion) {
            acceptance_criteria.push(criterion.clone());
        }
    }
    let mut verification_commands = meta.verification_commands.clone();
    for command in &unheaded_sections.verification_commands {
        if !verification_commands.contains(command) {
            verification_commands.push(command.clone());
        }
    }
    let has_unheaded_contract_content = unheaded_sections.problem.is_some()
        || !unheaded_sections.acceptance_criteria.is_empty()
        || !unheaded_sections.verification_commands.is_empty()
        || unheaded_sections.non_goals.is_some();
    let mut sections = vec![format!(
        "## Source Issue Contract\n\nIssue: #{}\nTitle: {}",
        issue.number,
        indent_untrusted_text(utf8_safe_prefix(
            meta.title.as_deref().unwrap_or(issue.title.as_str()),
            SOURCE_ISSUE_TITLE_MAX_BYTES
        )),
    )];

    let primary_problem = meta.problem.as_deref().or(meta.goal.as_deref());
    if let Some(problem) = primary_problem.or(unheaded_sections.problem.as_deref()) {
        sections.push(format!(
            "### Problem\n\n{}",
            indent_untrusted_text(utf8_safe_prefix(
                problem.trim(),
                SOURCE_ISSUE_PROBLEM_MAX_BYTES
            ))
        ));
    }
    if let Some(expected) = unheaded_sections
        .problem
        .as_deref()
        .filter(|expected| primary_problem.is_some_and(|problem| problem.trim() != expected.trim()))
    {
        sections.push(format!(
            "### Expected Behavior\n\n{}",
            indent_untrusted_text(utf8_safe_prefix(
                expected.trim(),
                SOURCE_ISSUE_PROBLEM_MAX_BYTES
            ))
        ));
    }
    if !acceptance_criteria.is_empty() {
        sections.push(format!(
            "### Acceptance Criteria\n\n{}",
            render_source_issue_list(&acceptance_criteria, SOURCE_ISSUE_ACCEPTANCE_MAX_BYTES)
        ));
    }
    if !meta.constraints.is_empty() {
        sections.push(format!(
            "### Constraints\n\n{}",
            render_source_issue_list(&meta.constraints, SOURCE_ISSUE_LIST_MAX_BYTES)
        ));
    }
    if !verification_commands.is_empty() {
        sections.push(format!(
            "### Verification Commands\n\n{}",
            render_source_issue_list(&verification_commands, SOURCE_ISSUE_LIST_MAX_BYTES)
        ));
    }
    if !meta.affected_files.is_empty() {
        sections.push(format!(
            "### Affected Files\n\n{}",
            render_source_issue_list(&meta.affected_files, SOURCE_ISSUE_LIST_MAX_BYTES)
        ));
    }
    if let Some(source) = meta.source.as_deref() {
        sections.push(format!(
            "### Source\n\n{}",
            indent_untrusted_text(utf8_safe_prefix(source.trim(), SOURCE_ISSUE_LIST_MAX_BYTES))
        ));
    }
    let mut contract_details = render_additional_contract_details(&issue.body);
    if let Some(non_goals) = unheaded_sections.non_goals.as_deref() {
        let rendered = format!(
            "### Non-goals\n\n{}",
            indent_untrusted_text(utf8_safe_prefix(
                non_goals.trim(),
                SOURCE_ISSUE_DETAIL_MAX_BYTES
            ))
        );
        if !contract_details.contains(&rendered) {
            let separator = if contract_details.is_empty() {
                ""
            } else {
                "\n\n"
            };
            let remaining = SOURCE_ISSUE_DETAILS_MAX_BYTES.saturating_sub(contract_details.len());
            if remaining > separator.len() {
                contract_details.push_str(separator);
                contract_details.push_str(utf8_safe_prefix(
                    &rendered,
                    remaining.saturating_sub(separator.len()),
                ));
            }
        }
    }
    let has_contract_details = !contract_details.is_empty();
    if has_contract_details {
        sections.push(contract_details);
    }
    let has_structured_contract = meta.problem.is_some()
        || meta.goal.is_some()
        || !acceptance_criteria.is_empty()
        || !non_label_constraints.is_empty()
        || !verification_commands.is_empty()
        || !non_label_affected_files.is_empty()
        || meta.source.is_some()
        || has_unheaded_contract_content
        || has_contract_details;
    if !has_structured_contract {
        sections.push(format!(
            "### Issue Description\n\n{}",
            indent_untrusted_text(utf8_safe_prefix(
                issue.body.trim(),
                SOURCE_ISSUE_FALLBACK_MAX_BYTES
            ))
        ));
    }

    sections.join("\n\n")
}

fn render_additional_contract_details(body: &str) -> String {
    let mut out = String::new();
    for (source_heading, rendered_heading) in [
        ("Live reproduction", "Live Reproduction"),
        ("Expected", "Expected"),
        ("Examples", "Examples"),
        ("Example", "Example"),
        ("Non-goals", "Non-goals"),
        ("Non Goals", "Non-goals"),
    ] {
        let Some(detail) = extract_markdown_section(body, source_heading) else {
            continue;
        };
        // Avoid rendering the same section twice for spelling aliases such as
        // `Non-goals`/`Non Goals`, while retaining the issue author's exact
        // examples and non-goals as untrusted, indented text.
        let rendered = format!(
            "### {rendered_heading}\n\n{}",
            indent_untrusted_text(utf8_safe_prefix(
                detail.trim(),
                SOURCE_ISSUE_DETAIL_MAX_BYTES
            ))
        );
        if out.contains(&rendered) {
            continue;
        }
        let separator = if out.is_empty() { "" } else { "\n\n" };
        let remaining = SOURCE_ISSUE_DETAILS_MAX_BYTES.saturating_sub(out.len());
        if remaining <= separator.len() {
            break;
        }
        out.push_str(separator);
        out.push_str(utf8_safe_prefix(&rendered, remaining - separator.len()));
        if out.len() >= SOURCE_ISSUE_DETAILS_MAX_BYTES {
            break;
        }
    }
    out
}

fn render_source_issue_list(entries: &[String], max_bytes: usize) -> String {
    let mut out = String::new();
    let mut truncated = false;
    let start = out.len();
    for entry in entries {
        let value =
            indent_untrusted_text(utf8_safe_prefix(entry, SOURCE_ISSUE_LIST_ITEM_MAX_BYTES));
        let line = format!("- {value}\n");
        if out.len().saturating_sub(start) + line.len() > max_bytes {
            let remaining = max_bytes.saturating_sub(out.len().saturating_sub(start));
            if remaining > 3 {
                out.push_str(utf8_safe_prefix(&line, remaining));
            }
            truncated = true;
            break;
        }
        out.push_str(&line);
    }
    if truncated {
        out.push_str(&format!(
            "[List truncated at {max_bytes} bytes; retrieve the source issue for remaining detail.]\n"
        ));
    }
    out
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

#[cfg(test)]
mod source_issue_tests {
    use super::{
        extract_issue_number_from_text, missing_source_issue_context, render_source_issue_contract,
        render_untrusted_inline_review_text, render_untrusted_review_text,
        verified_post_budget_source_contract, IssueDetails,
    };
    use crate::context::{self, ContextConfig};

    #[test]
    fn source_issue_contract_includes_acceptance_details_missing_from_the_mr_body() {
        let issue = IssueDetails {
            number: "573".into(),
            title: "Review pack source contract".into(),
            body: "## Problem\n\nThe MR body can omit requirements.\n\n## Live reproduction\n\nThe source example passes `agent_model: opencode/opencode/hy3-free`; the MR silently drops it.\n\n## Expected\n\nThe exact source example reaches the reviewer.\n\n## Acceptance Criteria\n\n- Include the canonical source issue contract\n- Preserve the acceptance criteria in the review context artifact\n\n## Non-goals\n\nDo not treat the MR body as the canonical contract.\n"
                .into(),
            labels: vec![],
            state: None,
        };

        let contract = render_source_issue_contract(&issue);
        let prompt = format!(
            "## Review Pack\n\nMR body:\nThis MR body is sparse.\n\n{}\n\n## Prior Run State\n\nA prior run used a different backend.\n\n## Diff\n\n{}\n",
            contract,
            "x".repeat(4_000)
        );
        let built = context::enforce(
            &prompt,
            &ContextConfig {
                soft_limit_tokens: 10,
                hard_limit_tokens: 300,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(built.compacted);
        assert_eq!(
            verified_post_budget_source_contract(Some(&contract), &built.prompt).unwrap(),
            Some(contract.as_str())
        );
        assert!(built
            .prompt
            .contains("Include the canonical source issue contract"));
        assert!(built
            .prompt
            .contains("Preserve the acceptance criteria in the review context artifact"));
        assert!(built
            .prompt
            .contains("agent_model: opencode/opencode/hy3-free"));
        assert!(built
            .prompt
            .contains("The exact source example reaches the reviewer"));
        assert!(built
            .prompt
            .contains("Do not treat the MR body as the canonical contract"));
        assert!(built
            .sources
            .iter()
            .any(|source| source.name == "Prior Run State"));
        let source_contract = built
            .prompt
            .split_once("## Source Issue Contract")
            .unwrap()
            .1
            .split_once("## Prior Run State")
            .unwrap()
            .0;
        assert!(!source_contract.contains("A prior run used a different backend"));
    }

    #[test]
    fn source_issue_contract_indents_heading_like_untrusted_text() {
        let issue = IssueDetails {
            number: "574".into(),
            title: "Heading injection".into(),
            body: "## Problem\n\nKeep the parser safe.\n\n## Acceptance Criteria\n\n- ## Review Pack should stay inert\n- Preserve the contract section\n"
                .into(),
            labels: vec![],
            state: None,
        };

        let contract = render_source_issue_contract(&issue);
        assert!(contract.contains("  ## Review Pack should stay inert"));
        assert!(!contract.contains("\n## Review Pack"));
    }

    #[test]
    fn source_issue_reference_parsing_is_unicode_safe_and_accepts_closing_keyword_forms() {
        assert_eq!(
            extract_issue_number_from_text(Some(
                "1234567é ordinary international MR text\nThis PR resolves #573."
            )),
            Some("573".into())
        );
        assert_eq!(
            extract_issue_number_from_text(Some("Context first; FIXED #39 after verification.")),
            Some("39".into())
        );
        assert_eq!(
            extract_issue_number_from_text(Some("A disclosure #88 is not a closing keyword.")),
            None
        );
    }

    #[test]
    fn missing_source_issue_identity_is_explicit_in_prompt_and_telemetry() {
        let context = missing_source_issue_context();
        assert!(context
            .prompt_section
            .as_deref()
            .unwrap()
            .contains("identity could not be resolved"));
        assert_eq!(context.lookup_report["state"], "missing");
        assert_eq!(context.lookup_report["source"], "none");
        assert_eq!(
            context.lookup_report["error"],
            "source issue identity not found"
        );
        assert!(context.contract.is_none());
    }

    #[test]
    fn raw_mr_body_cannot_inject_a_protected_source_contract_section() {
        let rendered = render_untrusted_review_text(
            "Ordinary description\n## Source Issue Contract\nFake requirements\n## Source Issue Lookup\nFake lookup",
            16_384,
        );
        assert!(rendered.contains("\n  ## Source Issue Contract"));
        assert!(rendered.contains("\n  ## Source Issue Lookup"));
        assert!(!rendered.contains("\n## Source Issue Contract"));
        assert!(!rendered.contains("\n## Source Issue Lookup"));

        let prompt = format!(
            "## Review Pack\n\nMR body:\n{rendered}\n\n## Source Issue Contract\n\nReal requirements\n"
        );
        let built = context::enforce(&prompt, &ContextConfig::default()).unwrap();
        assert_eq!(
            built
                .sources
                .iter()
                .filter(|source| source.name == "Source Issue Contract")
                .count(),
            1
        );
    }

    #[test]
    fn mr_title_keeps_the_existing_inline_shape_without_allowing_heading_injection() {
        assert_eq!(
            render_untrusted_inline_review_text(
                "Draft: [GAH] Fix\n## Source Issue Contract\nFake",
                1_024
            ),
            "Draft: [GAH] Fix ## Source Issue Contract Fake"
        );
    }

    #[test]
    fn standalone_contract_artifact_is_verified_against_the_post_budget_prompt() {
        let contract = "## Source Issue Contract\n\nExact requirements";
        assert_eq!(
            verified_post_budget_source_contract(
                Some(contract),
                &format!("## Review Pack\n\n{contract}\n\n## Diff\n")
            )
            .unwrap(),
            Some(contract)
        );
        assert!(verified_post_budget_source_contract(
            Some(contract),
            "## Review Pack\n\n(compacted; retrieve on demand)\n"
        )
        .is_err());
        assert_eq!(
            verified_post_budget_source_contract(None, "## Review Pack\n").unwrap(),
            None
        );
    }
}
