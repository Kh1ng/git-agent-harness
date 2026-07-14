use super::super::attempts::{
    apply_route_to_ledger, attempt_usage, classify_git_operation_result, classify_worktree_result,
    clear_wip_checkpoints, decide_route, failure_text_with_internal_log,
    mark_backend_unavailable_from_output, mark_shutdown_cancelled, preflight, reserve_backend_slot,
    resolve_llm, route_identity, run_backend_with_reserved_route, wip_checkpoint_branch,
};
use super::super::claims::ensure_dispatch_capacity;
use super::super::identity::timestamp;
use super::super::issues::{
    parse_ticket_metadata, parse_ticket_metadata_from_issue, resolve_target_to_issue_or_string,
    TicketMetadata,
};
use super::super::metrics::apply_diff_stats;
use super::super::mutation_policy::enforce_policy;
use super::super::prompts::{build_task, enforce_context_budget};
use super::super::publish::{
    build_fix_or_improve_mr_body, build_mr_title, emit_human_handoff,
    ensure_issue_open_for_publish, publishing_allows_publish, MrRenderContext,
};
use super::super::text::utf8_safe_prefix;
use super::super::validation::{
    classify_validation_failure_progress, run_auto_fix_commands, should_skip_per_dispatch_baseline,
    validation_env, validation_failure_no_progress_reason,
};
use super::super::DispatchArgs;
use crate::config::{self, GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::notifications::{notify_event, NotifyEvent};
use crate::routing::RouteRequest;
use crate::usage_attribution::{normalize_attempt_usage, UsageAttribution};
use crate::validation_runner::{validate, validate_with_exit_code};
use crate::{provider, runner, worktree};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn improve(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    // Resolve the Claude executable path so the optional live `/usage` PTY
    // probe (issue #153) can drive a real session when explicitly enabled.
    let claude_path = profile
        .claude_path
        .clone()
        .unwrap_or_else(|| "claude".to_string());

    // Enforce policy before any mutations
    let push_action = if args.prod {
        "git-push-prod"
    } else {
        "git-push"
    };
    enforce_policy(profile, "open-draft-pr")?;
    enforce_policy(profile, push_action)?;

    let target = if args.target.is_empty() {
        let default = PathBuf::from(&profile.artifact_root)
            .join("candidates")
            .join("latest.json");
        if default.exists() {
            println!("Auto-target: {}", default.display());
            default.to_string_lossy().into_owned()
        } else {
            args.target.clone()
        }
    } else {
        args.target.clone()
    };

    // Try to resolve target as an issue number. Propagate a real fetch
    // error (bad issue number, auth, rate limit) instead of silently
    // swallowing it and dispatching an agent against garbage content --
    // `resolve_target_to_issue_or_string` already returns `Ok(None)`
    // cleanly for a target that isn't an issue reference at all.
    let issue_details = resolve_target_to_issue_or_string(profile, &target)?;
    let ticket_meta = if let Some(ref issue) = issue_details {
        Some(parse_ticket_metadata_from_issue(issue))
    } else {
        parse_ticket_metadata(Path::new(&target)).ok().flatten()
    };
    let usage_summary = ledger::usage_summary_for_backend(
        cfg,
        args.backend.as_str(),
        args.model.as_deref(),
        Some(
            session_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
        ),
    )
    .ok();
    let route_req = RouteRequest {
        mode: &args.mode,
        requested_backend: config::canonical_backend_name(&args.backend),
        requested_model: args.model.as_deref(),
        recommended_backend: ticket_meta
            .as_ref()
            .and_then(|m| m.recommended_backend.as_deref()),
        recommended_model: ticket_meta
            .as_ref()
            .and_then(|m| m.recommended_model.as_deref()),
        session_id: session_dir.file_name().and_then(|s| s.to_str()),
        usage_summary,
        last_failure_class: if args.escalate {
            Some(crate::ledger::FailureClass::AgentNoProgress.as_str())
        } else {
            None
        },
    };
    let mut route = decide_route(
        cfg,
        profile,
        route_req.clone(),
        ticket_meta.as_ref(),
        ledger,
    )?;
    apply_route_to_ledger(ledger, &route);
    preflight(profile, &route.effective_backend)?;
    // Reserve the selected slot before telling a parallel controller that it
    // may choose the next action. The reservation stays alive through this
    // first backend attempt, so a sibling sees the live cap and falls through
    // to the next configured backend instance (for example agy-second).
    let mut initial_route_slot = Some(reserve_backend_slot(
        profile,
        &route.effective_backend,
        route.effective_model.as_deref(),
    )?);
    if let Some(route_ready) = &args.route_ready {
        let _ = route_ready.send(());
    }
    let mut llm = resolve_llm(
        cfg,
        args,
        profile.oh_profile.as_deref(),
        route.effective_model.as_deref(),
    )?;

    // Resolve env_file: use env_file_prod if --prod, otherwise env_file (dev)
    let resolved_env = if args.prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    if !resolved_env.is_empty() {
        println!("Env file: {}", resolved_env);
        if args.prod {
            println!("  \u{26a0}\u{fe0f}  PRODUCTION env - agent has live API access");
        }
    }

    let ts = timestamp();
    let branch = if let Some(ref existing_branch) = args.existing_branch {
        existing_branch.clone()
    } else {
        format!("gah/{}-{}", profile.repo_id, ts)
    };
    let worktree_base = PathBuf::from(&cfg.defaults.worktree_base);
    let repo = Path::new(&profile.local_path);
    ensure_dispatch_capacity(profile, &worktree_base)?;

    // TICKET-118: Handle existing branch for FixMr action
    let (branch, wt) = if let Some(ref existing_branch) = args.existing_branch {
        println!(
            "Creating worktree from existing branch '{}'...",
            existing_branch
        );
        let wt = classify_worktree_result(
            ledger,
            worktree::create_existing(repo, existing_branch, &worktree_base),
        )?;
        (existing_branch.clone(), wt)
    } else {
        println!(
            "Creating worktree from {}...",
            profile.default_target_branch
        );
        let wt = classify_worktree_result(
            ledger,
            worktree::create(
                repo,
                &profile.default_target_branch,
                &branch,
                &worktree_base,
            ),
        )?;
        (branch, wt)
    };
    ledger.branch = Some(branch.clone());
    apply_authoritative_work_identity(ledger, ticket_meta.as_ref(), &branch);
    println!("Worktree: {}", wt.display());
    println!("Branch:   {}", branch);
    let _cargo_target =
        crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root, session_dir)?;
    let validation_environment = validation_env(profile, session_dir);

    let mut base_task = build_task(profile, &wt, &args.mode, &target, issue_details.as_ref());

    let (baseline_failure, baseline_exit_code) = if should_skip_per_dispatch_baseline(
        profile.validation_commands.is_empty(),
        args.existing_branch.is_some(),
        args.skip_validation_gate,
    ) {
        (None, None)
    } else {
        println!("Baseline validation on pristine worktree...");
        match validate_with_exit_code(&profile.validation_commands, &wt, &validation_environment) {
            Ok(()) => {
                println!("Baseline validation passed.");
                (None, None)
            }
            Err((text, code)) => (Some(text), code),
        }
    };
    // TICKET-110/111: classify why the baseline failed, then apply policy.
    // clean/expected_red proceed (existing warning-in-prompt behavior);
    // harness_error/environment_error always stop; unknown_red stops unless
    // explicitly overridden. Never let this improvise -- see baseline.rs.
    let baseline_disposition = crate::baseline::classify_baseline(
        baseline_failure.as_deref().unwrap_or(""),
        baseline_exit_code,
        &profile.known_baseline_failure_markers,
    );
    if let Some(b) = &baseline_failure {
        fs::write(session_dir.join("baseline-validation-failure.txt"), b)?;
        println!(
            "Baseline validation ALREADY FAILING on untouched branch ({}).",
            baseline_disposition.as_str()
        );
        use crate::baseline::BaselineDisposition as BD;
        match baseline_disposition {
            BD::Clean => unreachable!("failure text implies a non-Clean disposition"),
            BD::HarnessError | BD::EnvironmentError => {
                ledger.set_failure(
                    match baseline_disposition {
                        BD::HarnessError => crate::ledger::FailureClass::HarnessError,
                        _ => crate::ledger::FailureClass::EnvironmentError,
                    },
                    crate::ledger::FailureStage::BaselineValidation,
                );
                worktree::cleanup(&wt, repo);
                anyhow::bail!(
                    "baseline validation stopped ({}): {}",
                    baseline_disposition.as_str(),
                    utf8_safe_prefix(b, 4_000),
                );
            }
            BD::UnknownRed if !args.allow_unknown_red_baseline => {
                ledger.set_failure(
                    crate::ledger::FailureClass::Unknown,
                    crate::ledger::FailureStage::BaselineValidation,
                );
                worktree::cleanup(&wt, repo);
                anyhow::bail!(
                    "baseline validation stopped (unknown_red): {}\n\nUse --allow-unknown-red-baseline to proceed anyway.",
                    utf8_safe_prefix(b, 4_000),
                );
            }
            BD::UnknownRed | BD::ExpectedRed => {
                base_task.push_str(&format!(
                    "\n\n## Warning: validation already fails on the untouched branch\n\n```\n{}\n```\n\nIf this ticket is about fixing that failure, fix it. Otherwise it is pre-existing — your changes must not add new failures.\n",
                    utf8_safe_prefix(b, 4_000),
                ));
            }
        }
    }

    let mut task = base_task.clone();
    let max_attempts = args.retries + 1;
    let mut validation_failed = false;
    let mut prev_failure: Option<String> = None;
    let mut prior_phase_context: Option<String> = None;
    let mut backend_summary = String::new();
    // Retry checkpoints are temporary recovery refs. They are deliberately
    // retained on any terminal failure, then removed only after a successful
    // publish so real partial work is never silently discarded.
    let mut wip_checkpoints = Vec::new();
    for attempt in 0..max_attempts {
        println!(
            "\nAttempt {}/{}: running {} backend...",
            attempt + 1,
            max_attempts,
            route.effective_backend
        );
        let attempt_session = session_dir.join(format!("attempt-{}", attempt + 1));
        fs::create_dir_all(&attempt_session)?;
        let attempt_state_before = classify_git_operation_result(
            ledger,
            crate::ledger::FailureStage::AgentRun,
            worktree::state_snapshot(&wt),
        )?;
        ledger.attempts_started = Some(ledger.attempts_started.unwrap_or(0) + 1);
        let attempt_start = std::time::Instant::now();

        let env_path = if !resolved_env.is_empty() {
            Some(resolved_env)
        } else {
            None
        };
        let fresh_context = if args.mode == "fix" {
            cfg.context
                .effective(&args.profile, &route.effective_backend)
                .fresh_context_on_fix
        } else {
            true
        };
        if !fresh_context {
            if let Some(previous) = prior_phase_context.as_deref() {
                task = format!("{task}\n\n## Prior Phase Context\n{previous}");
            }
        }
        task = match enforce_context_budget(
            cfg,
            profile,
            &args.profile,
            &route.effective_backend,
            if args.mode == "fix" { "fix" } else { "coding" },
            fresh_context,
            &task,
            &attempt_session,
            args.run_id.as_deref(),
            ledger,
        ) {
            Ok(prompt) => prompt,
            Err(err) => {
                worktree::cleanup(&wt, repo);
                return Err(err);
            }
        };
        let reserved_route_slot = if attempt == 0 {
            initial_route_slot.take()
        } else {
            None
        };
        let result = run_backend_with_reserved_route(
            &route.effective_backend,
            profile,
            &wt,
            &task,
            &attempt_session,
            &llm,
            route.effective_model.as_deref(),
            env_path,
            reserved_route_slot.is_some(),
        );
        let result = match result {
            Ok(r) => r,
            Err(e) => {
                // The backend process itself couldn't launch (binary missing,
                // exec failure) — this is a setup/harness problem, not the
                // agent or backend failing at its job.
                ledger.set_failure(
                    crate::ledger::FailureClass::HarnessError,
                    crate::ledger::FailureStage::BackendLaunch,
                );
                ledger.attempts.push(crate::ledger::AttemptRecord {
                    attempt_number: attempt + 1,
                    backend: route.effective_backend.clone(),
                    effective_model: Some(llm.model.clone()),
                    exit_code: None,
                    validation_result: None,
                    failure_class: Some(crate::ledger::FailureClass::HarnessError.as_str().into()),
                    failure_stage: Some(crate::ledger::FailureStage::BackendLaunch.as_str().into()),
                    duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                    diff_path: None,
                    cli_version: None,
                    usage: normalize_attempt_usage(
                        crate::ledger::LedgerUsage::default(),
                        UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                        false,
                    ),
                });
                worktree::cleanup(&wt, repo);
                return Err(e);
            }
        };
        // The backend process launched and ran to an exit code, regardless
        // of what that code was — "completed" tracks whether the attempt
        // got a fair shot, not whether it succeeded.
        ledger.attempts_completed = Some(ledger.attempts_completed.unwrap_or(0) + 1);

        println!(
            "Backend finished: exit={} duration={:.0}s log={}",
            result.exit_code, result.duration_secs, result.log_path
        );
        ledger.backend_exit_code = Some(result.exit_code);

        // SIGINT/SIGTERM is an operator lifecycle event, not a backend
        // failure to retry. The runner already killed and reaped the backend
        // process group; return so the controller can write the matching
        // terminal dispatch event.
        if crate::runner::shutdown_requested() {
            mark_shutdown_cancelled(
                ledger,
                crate::ledger::FailureStage::AgentRun,
                Some(result.exit_code),
            );
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt + 1,
                backend: route.effective_backend.clone(),
                effective_model: Some(llm.model.clone()),
                exit_code: Some(result.exit_code),
                validation_result: Some("cancelled_shutdown".into()),
                failure_class: Some(crate::ledger::FailureClass::HarnessError.as_str().into()),
                failure_stage: Some(crate::ledger::FailureStage::AgentRun.as_str().into()),
                duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                diff_path: None,
                usage: attempt_usage(
                    &result.log_path,
                    result.agy_cli_log_delta.as_deref(),
                    UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                    result.transcript_path.as_deref(),
                    Some(&claude_path),
                ),
                cli_version: result.agy_version.clone(),
            });
            worktree::preserve_wip(
                &wt,
                &profile.default_target_branch,
                &format!("gah: WIP interrupted {} attempt {}", args.mode, attempt + 1),
            )?;
            worktree::cleanup(&wt, repo);
            anyhow::bail!(
                "shutdown requested while {} was running",
                route.effective_backend
            );
        }

        backend_summary = runner::output::publishable_summary(
            result.final_summary.as_deref(),
            ledger.target_summary.as_deref(),
            &wt,
        );

        if result.exit_code != 0 {
            // The backend launched but exited nonzero — the backend itself
            // failed at its job, distinct from it never starting at all.
            let output_log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
            let log_text = failure_text_with_internal_log(
                &output_log_text,
                result.internal_log_delta.as_deref(),
            );
            let failure_log_path = result
                .internal_log_path
                .as_deref()
                .unwrap_or(&result.log_path);
            let stalled = log_text.contains("GAH: killed after ")
                && log_text.contains("(stalled, not just slow).");
            let semantic_no_progress = stalled
                && log_text.contains("with no new worktree progress (stalled, not just slow).");
            let failure_class = if semantic_no_progress {
                crate::ledger::FailureClass::AgentNoProgress
            } else if stalled {
                crate::ledger::FailureClass::HarnessError
            } else {
                crate::ledger::FailureClass::BackendError
            };
            ledger.set_failure(failure_class, crate::ledger::FailureStage::AgentRun);
            if stalled {
                ledger.validation_result = Some("not_run_backend_stalled".into());
            }
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt + 1,
                backend: route.effective_backend.clone(),
                effective_model: Some(llm.model.clone()),
                exit_code: Some(result.exit_code),
                validation_result: stalled.then(|| "not_run_backend_stalled".into()),
                failure_class: Some(failure_class.as_str().into()),
                failure_stage: Some(crate::ledger::FailureStage::AgentRun.as_str().into()),
                duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                diff_path: None,
                usage: attempt_usage(
                    &result.log_path,
                    result.agy_cli_log_delta.as_deref(),
                    UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                    result.transcript_path.as_deref(),
                    Some(&claude_path),
                ),
                cli_version: result.agy_version.clone(),
            });
            if stalled {
                notify_event(
                    cfg,
                    profile,
                    NotifyEvent::BackendStalled {
                        work_id: ledger.work_id.as_deref().unwrap_or("unknown"),
                        backend: &route.effective_backend,
                        model: route.effective_model.as_deref().unwrap_or(&llm.model),
                        duration_seconds: result.duration_secs,
                    },
                );
            }
            if semantic_no_progress {
                worktree::preserve_wip(
                    &wt,
                    &profile.default_target_branch,
                    &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
                )?;
                worktree::cleanup(&wt, repo);
                anyhow::bail!(
                    "{} made no repository progress on attempt {}; not retrying blindly",
                    route.effective_backend,
                    attempt + 1
                );
            }
            if attempt + 1 < max_attempts {
                if let Some(parsed) = mark_backend_unavailable_from_output(
                    &route.effective_backend,
                    route.effective_model.as_deref(),
                    route.effective_quota_pool.as_deref(),
                    &log_text,
                    failure_log_path,
                )? {
                    let rerouted = decide_route(
                        cfg,
                        profile,
                        route_req.clone(),
                        ticket_meta.as_ref(),
                        ledger,
                    )?;
                    let current_identity =
                        route_identity(&route.effective_backend, route.effective_model.as_deref());
                    let rerouted_identity = route_identity(
                        &rerouted.effective_backend,
                        rerouted.effective_model.as_deref(),
                    );
                    if rerouted_identity != current_identity {
                        let checkpoint = wip_checkpoint_branch(&branch, attempt + 1);
                        let resumed = worktree::checkpoint_wip(
                            &wt,
                            &profile.default_target_branch,
                            &checkpoint,
                            &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
                        )?;
                        if resumed {
                            println!(
                                "Preserved retry progress on local branch {checkpoint}; next backend will continue it"
                            );
                            wip_checkpoints.push(checkpoint);
                        }
                        prior_phase_context = Some(task.clone());
                        task = format!(
                            "{}\n\n## Previous backend became unavailable (attempt {}/{})\n\nThe existing checkpointed repository changes remain in this worktree. Inspect them, preserve useful progress, and complete or repair the ticket with the next backend.",
                            base_task,
                            attempt + 1,
                            max_attempts,
                        );
                        println!(
                            "Backend unavailable; retrying next attempt with {} instead of {} ({:?})",
                            rerouted.effective_backend, route.effective_backend, parsed.kind
                        );
                        route = rerouted;
                        apply_route_to_ledger(ledger, &route);
                        preflight(profile, &route.effective_backend)?;
                        llm = resolve_llm(
                            cfg,
                            args,
                            profile.oh_profile.as_deref(),
                            route.effective_model.as_deref(),
                        )?;
                        continue;
                    }
                }
                // Live-observed bug: a generic backend error (an idle-timeout
                // kill, a transient crash, anything `mark_backend_unavailable_from_output`
                // doesn't recognize as a quota/rate-limit message) fell straight
                // through to bail!() below, ending the ENTIRE dispatch after a
                // single attempt regardless of --retries -- the reroute branch
                // above was the ONLY retry path that existed, so a non-quota
                // failure never got a second attempt at all. Retry with the
                // SAME backend/model instead, mirroring the validation-failure
                // retry path (wipe partial changes, rebuild task with context).
                println!(
                    "Backend error (exit {}) on attempt {}/{}, not a recognized quota/rate-limit signal -- retrying with the same backend...",
                    result.exit_code, attempt + 1, max_attempts
                );
                let checkpoint = wip_checkpoint_branch(&branch, attempt + 1);
                let resumed = worktree::checkpoint_wip(
                    &wt,
                    &profile.default_target_branch,
                    &checkpoint,
                    &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
                )?;
                if resumed {
                    println!("Preserved failed attempt on local branch {checkpoint}");
                    wip_checkpoints.push(checkpoint.clone());
                }
                prior_phase_context = Some(task.clone());
                task = if resumed {
                    format!(
                        "{}\n\n## Previous attempt did not complete (attempt {}/{})\n\nThe backend exited with code {} before finishing. Its checkpointed repository changes remain in this worktree. Inspect them, preserve useful progress, and continue the implementation.",
                        base_task,
                        attempt + 1,
                        max_attempts,
                        result.exit_code,
                    )
                } else {
                    format!(
                        "{}\n\n## Previous attempt did not complete (attempt {}/{})\n\nThe backend exited with code {} before producing repository changes. Start the implementation now.",
                        base_task,
                        attempt + 1,
                        max_attempts,
                        result.exit_code,
                    )
                };
                continue;
            }
            worktree::preserve_wip(
                &wt,
                &profile.default_target_branch,
                &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
            )?;
            worktree::cleanup(&wt, repo);
            anyhow::bail!(
                "backend exited {} on attempt {}",
                result.exit_code,
                attempt + 1
            );
        }

        // An exit-0 process that leaves the worktree unchanged did not
        // complete the ticket. Treating it as success would let a backend
        // consume quota, pass the repository's unchanged test suite, and
        // falsely advance the controller with no patch or PR to show for it.
        // Stop before post-change validation: there is no change to validate.
        let attempt_state_after = classify_git_operation_result(
            ledger,
            crate::ledger::FailureStage::PostValidation,
            worktree::state_snapshot(&wt),
        )?;
        if attempt_state_after == attempt_state_before {
            // OpenCode can exit successfully after a provider rejection and
            // put the useful diagnostic only in its internal log. Inspect
            // that run-scoped tail before treating this as generic no-progress
            // so the next route cannot select the unavailable model again.
            let output_log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
            let failure_text = failure_text_with_internal_log(
                &output_log_text,
                result.internal_log_delta.as_deref(),
            );
            let failure_log_path = result
                .internal_log_path
                .as_deref()
                .unwrap_or(&result.log_path);
            if let Some(parsed) = mark_backend_unavailable_from_output(
                &route.effective_backend,
                route.effective_model.as_deref(),
                route.effective_quota_pool.as_deref(),
                &failure_text,
                failure_log_path,
            )? {
                ledger.set_failure(
                    crate::ledger::FailureClass::BackendError,
                    crate::ledger::FailureStage::AgentRun,
                );
                ledger.attempts.push(crate::ledger::AttemptRecord {
                    attempt_number: attempt + 1,
                    backend: route.effective_backend.clone(),
                    effective_model: Some(llm.model.clone()),
                    exit_code: Some(0),
                    validation_result: Some("not_run_backend_unavailable".into()),
                    failure_class: Some(crate::ledger::FailureClass::BackendError.as_str().into()),
                    failure_stage: Some(crate::ledger::FailureStage::AgentRun.as_str().into()),
                    duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                    diff_path: None,
                    usage: attempt_usage(
                        &result.log_path,
                        result.agy_cli_log_delta.as_deref(),
                        UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                        result.transcript_path.as_deref(),
                        Some(&claude_path),
                    ),
                    cli_version: result.agy_version.clone(),
                });
                if attempt + 1 < max_attempts {
                    let rerouted = decide_route(
                        cfg,
                        profile,
                        route_req.clone(),
                        ticket_meta.as_ref(),
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
                            "Backend unavailable after no-progress result; retrying next attempt with {} instead of {} ({:?})",
                            rerouted.effective_backend, route.effective_backend, parsed.kind
                        );
                        route = rerouted;
                        apply_route_to_ledger(ledger, &route);
                        preflight(profile, &route.effective_backend)?;
                        llm = resolve_llm(
                            cfg,
                            args,
                            profile.oh_profile.as_deref(),
                            route.effective_model.as_deref(),
                        )?;
                        continue;
                    }
                }
                worktree::cleanup(&wt, repo);
                anyhow::bail!(
                    "{} reported {:?} after attempt {} made no worktree changes",
                    route.effective_backend,
                    parsed.kind,
                    attempt + 1
                );
            }
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt + 1,
                backend: route.effective_backend.clone(),
                effective_model: Some(llm.model.clone()),
                exit_code: Some(0),
                validation_result: Some("not_run_no_changes".into()),
                failure_class: Some(crate::ledger::FailureClass::AgentNoProgress.as_str().into()),
                failure_stage: Some(crate::ledger::FailureStage::AgentRun.as_str().into()),
                duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                diff_path: None,
                usage: attempt_usage(
                    &result.log_path,
                    result.agy_cli_log_delta.as_deref(),
                    UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                    result.transcript_path.as_deref(),
                    Some(&claude_path),
                ),
                cli_version: result.agy_version.clone(),
            });
            if attempt + 1 < max_attempts {
                // No progress is recoverable: a fresh attempt can get a
                // clearer instruction or a transient backend condition may
                // have cleared. Preserve the failed attempt in the ledger,
                // but do not stamp the overall dispatch as failed unless all
                // bounded attempts make no progress.
                println!(
                    "Backend made no changes on attempt {}/{}; retrying with explicit no-progress context...",
                    attempt + 1,
                    max_attempts
                );
                prior_phase_context = Some(task.clone());
                task = format!(
                    "{}\n\n## Previous attempt made no progress (attempt {}/{})\n\nThe backend exited successfully but did not change the worktree. Re-read the scoped task, make the required implementation change, and do not stop until a concrete diff exists.",
                    base_task,
                    attempt + 1,
                    max_attempts,
                );
                continue;
            }
            ledger.validation_result = Some("not_run_no_changes".into());
            ledger.set_failure(
                crate::ledger::FailureClass::AgentNoProgress,
                crate::ledger::FailureStage::AgentRun,
            );
            worktree::cleanup(&wt, repo);
            anyhow::bail!(
                "backend exited 0 on attempt {} but produced no worktree changes",
                attempt + 1
            );
        }

        if profile.validation_commands.is_empty() {
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt + 1,
                backend: route.effective_backend.clone(),
                effective_model: Some(llm.model.clone()),
                exit_code: Some(0),
                validation_result: None,
                failure_class: None,
                failure_stage: None,
                duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                diff_path: None,
                usage: attempt_usage(
                    &result.log_path,
                    result.agy_cli_log_delta.as_deref(),
                    UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                    result.transcript_path.as_deref(),
                    Some(&claude_path),
                ),
                cli_version: result.agy_version.clone(),
            });
            break;
        }

        run_auto_fix_commands(&profile.auto_fix_commands, &wt, &validation_environment);

        println!(
            "Running validation ({} commands)...",
            profile.validation_commands.len()
        );
        match validate(&profile.validation_commands, &wt, &validation_environment) {
            Ok(()) => {
                println!("Validation passed.");
                validation_failed = false;
                ledger.validation_result = Some("passed".into());
                ledger.attempts.push(crate::ledger::AttemptRecord {
                    attempt_number: attempt + 1,
                    backend: route.effective_backend.clone(),
                    effective_model: Some(llm.model.clone()),
                    exit_code: Some(0),
                    validation_result: Some("passed".into()),
                    failure_class: None,
                    failure_stage: None,
                    duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                    diff_path: None,
                    usage: attempt_usage(
                        &result.log_path,
                        result.agy_cli_log_delta.as_deref(),
                        UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                        result.transcript_path.as_deref(),
                        Some(&claude_path),
                    ),
                    cli_version: result.agy_version.clone(),
                });
                break;
            }
            Err(e) => {
                validation_failed = true;
                let failure_output = format!("{:#}", e);
                let failure_path = attempt_session.join("validation-failure.txt");
                fs::write(&failure_path, &failure_output)?;
                println!("Validation failed ({})", failure_path.display());

                // Identical failure to the previous attempt means the agent's
                // changes had no effect on the error — almost always an
                // environment/config problem the agent cannot fix. Stop burning
                // attempts.
                let failure_progress = classify_validation_failure_progress(
                    baseline_failure.as_deref(),
                    prev_failure.as_deref(),
                    &failure_output,
                );
                prev_failure = Some(failure_output.clone());
                prior_phase_context = Some(task.clone());

                if attempt + 1 < max_attempts
                    && !failure_progress.unchanged_from_baseline()
                    && !failure_progress.unchanged_from_previous_attempt()
                {
                    // Save the failed attempt's diff before wiping, so the
                    // session artifact shows what the agent actually wrote.
                    let _ = worktree::git(&["add", "-A"], &wt);
                    let mut diff_path = None;
                    if let Ok(diff) = worktree::git(&["diff", "--cached"], &wt) {
                        let path = attempt_session.join("attempt-diff.patch");
                        if fs::write(&path, diff).is_ok() {
                            diff_path = Some(path.display().to_string());
                        }
                    }
                    // Checkpoint the actual failed tree before the clean
                    // retry. The old implementation reset it in place and
                    // permanently lost substantial, often nearly-correct
                    // work whenever later attempts also failed.
                    let checkpoint = wip_checkpoint_branch(&branch, attempt + 1);
                    if worktree::checkpoint_wip(
                        &wt,
                        &profile.default_target_branch,
                        &checkpoint,
                        &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
                    )? {
                        println!("Preserved failed attempt on local branch {checkpoint}");
                        wip_checkpoints.push(checkpoint.clone());
                    }
                    worktree::reset_to_target(&wt, &profile.default_target_branch)?;
                    println!("Retrying with failure context...");
                    ledger.attempts.push(crate::ledger::AttemptRecord {
                        attempt_number: attempt + 1,
                        backend: route.effective_backend.clone(),
                        effective_model: Some(llm.model.clone()),
                        exit_code: Some(0),
                        validation_result: Some("failed".into()),
                        failure_class: Some(
                            crate::ledger::FailureClass::ValidationFailure
                                .as_str()
                                .into(),
                        ),
                        failure_stage: Some(
                            crate::ledger::FailureStage::PostValidation.as_str().into(),
                        ),
                        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                        diff_path,
                        usage: attempt_usage(
                            &result.log_path,
                            result.agy_cli_log_delta.as_deref(),
                            UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                            result.transcript_path.as_deref(),
                            Some(&claude_path),
                        ),
                        cli_version: result.agy_version.clone(),
                    });
                    // Rebuild from the base task with only the latest failure —
                    // accumulating retry blocks confuses smaller models.
                    task = format!(
                        "{}\n\n## Previous attempt failed validation (attempt {}/{})\n\nThe previous tree was checkpointed locally as `{}`. This retry starts from a clean target branch. Fix the following before completing the task:\n\n```\n{}\n```",
                        base_task,
                        attempt + 1,
                        max_attempts,
                        checkpoint,
                        utf8_safe_prefix(&failure_output, 8_000),
                    );
                    // TICKET-089 AC7: made real (if imperfect) progress and
                    // failed validation again -- a genuine agent-capability
                    // failure, distinct from harness/backend/quota failures.
                    // Route again with that context so cost-aware ordering
                    // may escalate to a stronger model for the retry.
                    let mut escalation_req = route_req.clone();
                    escalation_req.last_failure_class =
                        Some(crate::ledger::FailureClass::ValidationFailure.as_str());
                    let rerouted =
                        decide_route(cfg, profile, escalation_req, ticket_meta.as_ref(), ledger)?;
                    let current_identity =
                        route_identity(&route.effective_backend, route.effective_model.as_deref());
                    let rerouted_identity = route_identity(
                        &rerouted.effective_backend,
                        rerouted.effective_model.as_deref(),
                    );
                    if rerouted_identity != current_identity {
                        println!(
                            "Escalating retry after validation failure: {} -> {}",
                            route
                                .effective_model
                                .as_deref()
                                .unwrap_or(&route.effective_backend),
                            rerouted
                                .effective_model
                                .as_deref()
                                .unwrap_or(&rerouted.effective_backend),
                        );
                        route = rerouted;
                        apply_route_to_ledger(ledger, &route);
                        preflight(profile, &route.effective_backend)?;
                        llm = resolve_llm(
                            cfg,
                            args,
                            profile.oh_profile.as_deref(),
                            route.effective_model.as_deref(),
                        )?;
                    }
                } else if attempt + 1 < max_attempts && !args.allow_draft_fail {
                    let Some(reason) = validation_failure_no_progress_reason(failure_progress)
                    else {
                        worktree::preserve_wip(
                            &wt,
                            &profile.default_target_branch,
                            &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
                        )?;
                        worktree::cleanup(&wt, repo);
                        anyhow::bail!(
                            "validation failed after {} attempt(s). Use --allow-draft-fail to push anyway.\n\n{}",
                            max_attempts,
                            utf8_safe_prefix(&failure_output, 4_000),
                        );
                    };
                    // Identical to baseline and/or the previous attempt: the
                    // agent made no measurable progress, which is a distinct
                    // failure mode from "tried and failed differently."
                    ledger.set_failure(
                        crate::ledger::FailureClass::AgentNoProgress,
                        crate::ledger::FailureStage::PostValidation,
                    );
                    ledger.attempts.push(crate::ledger::AttemptRecord {
                        attempt_number: attempt + 1,
                        backend: route.effective_backend.clone(),
                        effective_model: Some(llm.model.clone()),
                        exit_code: Some(0),
                        validation_result: Some("failed".into()),
                        failure_class: Some(
                            crate::ledger::FailureClass::AgentNoProgress.as_str().into(),
                        ),
                        failure_stage: Some(
                            crate::ledger::FailureStage::PostValidation.as_str().into(),
                        ),
                        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                        diff_path: None,
                        usage: attempt_usage(
                            &result.log_path,
                            result.agy_cli_log_delta.as_deref(),
                            UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                            result.transcript_path.as_deref(),
                            Some(&claude_path),
                        ),
                        cli_version: result.agy_version.clone(),
                    });
                    worktree::preserve_wip(
                        &wt,
                        &profile.default_target_branch,
                        &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
                    )?;
                    worktree::cleanup(&wt, repo);
                    anyhow::bail!(
                        "{} Aborting early after attempt {}.\n\n{}",
                        reason,
                        attempt + 1,
                        utf8_safe_prefix(&failure_output, 4_000),
                    );
                } else if args.allow_draft_fail {
                    println!(
                        "Validation still failing; --allow-draft-fail set — pushing as draft."
                    );
                    ledger.validation_result = Some("failed-draft".into());
                    ledger.attempts.push(crate::ledger::AttemptRecord {
                        attempt_number: attempt + 1,
                        backend: route.effective_backend.clone(),
                        effective_model: Some(llm.model.clone()),
                        exit_code: Some(0),
                        validation_result: Some("failed-draft".into()),
                        failure_class: None,
                        failure_stage: None,
                        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                        diff_path: None,
                        usage: attempt_usage(
                            &result.log_path,
                            result.agy_cli_log_delta.as_deref(),
                            UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                            result.transcript_path.as_deref(),
                            Some(&claude_path),
                        ),
                        cli_version: result.agy_version.clone(),
                    });
                    break;
                } else {
                    ledger.attempts.push(crate::ledger::AttemptRecord {
                        attempt_number: attempt + 1,
                        backend: route.effective_backend.clone(),
                        effective_model: Some(llm.model.clone()),
                        exit_code: Some(0),
                        validation_result: Some("failed".into()),
                        failure_class: Some(
                            crate::ledger::FailureClass::ValidationFailure
                                .as_str()
                                .into(),
                        ),
                        failure_stage: Some(
                            crate::ledger::FailureStage::PostValidation.as_str().into(),
                        ),
                        duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                        diff_path: None,
                        usage: attempt_usage(
                            &result.log_path,
                            result.agy_cli_log_delta.as_deref(),
                            UsageAttribution::from_route(&route).with_fallback_model(&llm.model),
                            result.transcript_path.as_deref(),
                            Some(&claude_path),
                        ),
                        cli_version: result.agy_version.clone(),
                    });
                    worktree::preserve_wip(
                        &wt,
                        &profile.default_target_branch,
                        &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
                    )?;
                    worktree::cleanup(&wt, repo);
                    anyhow::bail!(
                        "validation failed after {} attempt(s). Use --allow-draft-fail to push anyway.\n\n{}",
                        max_attempts,
                        utf8_safe_prefix(&failure_output, 4_000),
                    );
                }
            }
        }
    }

    if profile.validation_commands.is_empty() && ledger.validation_result.is_none() {
        ledger.validation_result = Some("not_run".into());
    }

    // ── Architecture note ──────────────────────────────────────────────────
    // The retry loop above cold-restarts the backend on each attempt. It does
    // NOT maintain a persistent agent session across retries. Each attempt
    // launches a fresh backend process with accumulated failure context in the
    // task prompt. This is intentional — the current design prioritizes
    // simplicity and observability over session persistence. A future version
    // could keep the backend running (e.g., via a socket or API) and push
    // validation feedback into the existing conversation, but that would
    // require each backend to expose a continuation API. For now, the retry
    // loop is stateless: fail → append context → re-launch.
    //
    // The validation_commands list runs sequentially in the worktree directory.
    // All commands must exit 0 for the attempt to count as passing. The full
    // stdout+stderr of any failing command is fed back into the next attempt's
    // prompt, truncated to 8 000 chars to stay within context windows.
    // Because the backend is re-launched from scratch each attempt, the agent
    // must re-read the repo state — it cannot carry working memory between
    // attempts. This is acceptable for bounded code-generation tasks where
    // each attempt is self-contained.
    // ────────────────────────────────────────────────────────────────────────

    let has_changes = classify_git_operation_result(
        ledger,
        crate::ledger::FailureStage::PostValidation,
        worktree::has_changes(&wt, &profile.default_target_branch),
    )?;
    if !has_changes {
        // Defensive backstop: auto-fix commands or a future post-validation
        // transform could remove every change after the normal early check.
        // Do not let that become a successful no-op dispatch either.
        ledger.validation_result = Some("passed_no_changes".into());
        ledger.set_failure(
            crate::ledger::FailureClass::AgentNoProgress,
            crate::ledger::FailureStage::AgentRun,
        );
        if let Some(last_attempt) = ledger.attempts.last_mut() {
            last_attempt.validation_result = Some("passed_no_changes".into());
            last_attempt.failure_class =
                Some(crate::ledger::FailureClass::AgentNoProgress.as_str().into());
            last_attempt.failure_stage =
                Some(crate::ledger::FailureStage::AgentRun.as_str().into());
        }
        worktree::cleanup(&wt, repo);
        anyhow::bail!("all worktree changes disappeared before publish");
    }
    let commit_title = if validation_failed {
        format!(
            "gah: {} changes for {} [validation-failing draft]",
            args.mode, profile.repo_id
        )
    } else {
        format!("gah: {} changes for {}", args.mode, profile.repo_id)
    };
    let mut commit_msg = commit_title;
    if !backend_summary.is_empty() {
        commit_msg.push_str("\n\n");
        commit_msg.push_str(&backend_summary);
    }

    // TICKET-128: honor the per-profile publishing policy. A restricted profile
    // forbids PR/MR creation and/or LLM-generated commit messages, so we stop
    // at a deterministic human handoff after code generation + validation
    // instead of publishing the work. This is independent of reviewer routing
    // and merge policy: review still runs, the worktree is still cleaned up,
    // only the autonomous publish step is suppressed.
    if !publishing_allows_publish(profile) {
        // Commit only if the policy still permits agent-authored commit text;
        // otherwise leave the worktree uncommitted for human completion.
        if profile.publishing.allow_commit_message_generation {
            if worktree::has_uncommitted_changes(&wt)? {
                ledger.commit_attempted = true;
                worktree::stage_all(&wt)?;
                worktree::ensure_staged(&wt)?;
                worktree::commit_msg(&wt, &commit_msg)?;
                ledger.commit_created = true;
            } else {
                ledger.commit_created = true;
            }
        }
        apply_diff_stats(ledger, &wt, &profile.default_target_branch);
        emit_human_handoff(
            profile,
            ledger,
            &branch,
            "PR/MR creation or commit-message generation disabled by publishing policy",
        );
        clear_wip_checkpoints(repo, &wip_checkpoints);
        worktree::preserve_wip(
            &wt,
            &profile.default_target_branch,
            &format!("gah: WIP handoff {}", args.mode),
        )?;
        worktree::cleanup(&wt, repo);
        return Ok(());
    }

    if let Some(issue) = issue_details.as_ref() {
        if let Err(error) = ensure_issue_open_for_publish(profile, issue) {
            ledger.set_failure(
                crate::ledger::FailureClass::HumanBlocked,
                crate::ledger::FailureStage::Push,
            );
            worktree::preserve_wip(
                &wt,
                &profile.default_target_branch,
                &format!("gah: WIP blocked {}", args.mode),
            )?;
            worktree::cleanup(&wt, repo);
            return Err(error);
        }
    }

    println!("Changes detected. Committing and pushing...");
    let push_url = profile.push_url()?;
    let push_pat = profile.pat();
    if worktree::has_uncommitted_changes(&wt)? {
        ledger.commit_attempted = true;
        worktree::stage_all(&wt)?;
        worktree::ensure_staged(&wt)?;
        worktree::commit_msg(&wt, &commit_msg)?;
        ledger.commit_created = true;
    } else {
        // Backend committed its own work already (e.g. vibe) -- nothing left
        // to stage, just push what's already on HEAD.
        ledger.commit_created = true;
    }
    // Must run after the commit above -- diff_stats/changed_files compare
    // origin/<target> against HEAD, so computing them beforehand (while the
    // real changes are still uncommitted working-tree modifications) always
    // reported "0 file(s) changed, +0, -0" in the MR body.
    apply_diff_stats(ledger, &wt, &profile.default_target_branch);
    ledger.push_attempted = true;
    classify_git_operation_result(
        ledger,
        crate::ledger::FailureStage::Push,
        worktree::push_branch(&wt, &branch, &push_url, &push_pat),
    )?;
    ledger.push_succeeded = true;

    let mr_title = build_mr_title(
        &args.mode,
        &profile.repo_id,
        validation_failed,
        ticket_meta.as_ref(),
    );
    let mr_ctx = MrRenderContext {
        backend: &route.effective_backend,
        model: &llm.model,
        branch: &branch,
        target_branch: &profile.default_target_branch,
        validation_commands: &profile.validation_commands,
        ledger,
        backend_summary: &backend_summary,
    };
    let mr_body = build_fix_or_improve_mr_body(
        &args.mode,
        ticket_meta.as_ref(),
        &mr_ctx,
        !validation_failed,
    );
    ledger.mr_attempted = true;
    let mr = provider::create_draft_mr(profile, &branch, &mr_title, &mr_body)?;
    ledger.mr_created = true;
    ledger.mr_url = Some(mr.url.clone());
    println!("Draft MR: {}", mr.url);
    notify_event(
        cfg,
        profile,
        NotifyEvent::MrCreated {
            url: &mr.url,
            work_id: ledger.work_id.as_deref().unwrap_or("unknown"),
            backend: &route.effective_backend,
            model: route.effective_model.as_deref().unwrap_or("unknown"),
        },
    );

    clear_wip_checkpoints(repo, &wip_checkpoints);
    worktree::cleanup(&wt, repo);
    Ok(())
}

/// TICKET-091 AC4: when no authoritative external ticket exists, fall back
/// to the branch name (already unique/timestamped at dispatch time) as a
/// synthetic internal work ID rather than leaving it unset. This never
/// collides with a real ticket's work_id in `check_duplicate_work`, which
/// only ever computes its lookup key from a ticket file or candidate JSON.
fn apply_authoritative_work_identity(
    ledger: &mut LedgerEntry,
    ticket: Option<&TicketMetadata>,
    fallback_work_id: &str,
) {
    if let Some(ticket) = ticket {
        ledger.task_class = ticket.task_class.clone();
        ledger.difficulty = ticket.difficulty.clone();
    }
    match ticket {
        Some(ticket) if ticket.is_authoritative => {
            ledger.work_id = ticket.work_id.clone().or_else(|| ticket.ticket_id.clone());
            ledger.source_issue_number = ticket.issue_number.clone();
            ledger.work_title = ticket.title.clone();
        }
        _ => {
            ledger.work_id = Some(fallback_work_id.to_string());
        }
    }
}

#[cfg(test)]
mod tests;
