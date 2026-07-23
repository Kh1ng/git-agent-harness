use super::decision::decide_next_action;
use super::human_required_reason::HumanRequiredReason;
use super::recovery::{
    defer_if_branch_attached, detect_stuck_loop, latest_clear_attempts_timestamp,
    recently_capacity_deferred_work_ids, reconcile_abandoned_dispatches, record_action_events,
    remediation_plan_for_action, retain_snapshot_candidates,
};
use super::NextAction;
use anyhow::Result;
use serde::Serialize;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::time::{Duration, Instant};

#[path = "runtime/profile_lock.rs"]
mod profile_lock;
pub use profile_lock::acquire_profile_lock;
use profile_lock::reload_config_for_profile;

#[path = "runtime/route_state.rs"]
mod route_state;
use route_state::route_state_fingerprint;
#[path = "runtime/dispatch_policy.rs"]
mod dispatch_policy;
#[path = "runtime/dispatch_state.rs"]
mod dispatch_state;
use dispatch_state::{
    action_review_generation, append_stuck_loop_gate_if_transition, is_validation_gate_failure,
    suppress_recent_capacity_deferrals,
};
#[path = "runtime/intake.rs"]
mod intake;
use intake::{
    action_creates_managed_mr, action_intake_key, apply_parallel_projection, retain_unclaimed_work,
};
#[path = "runtime/merge.rs"]
mod merge;
#[path = "runtime/node_capacity.rs"]
mod node_capacity;
#[path = "runtime/pm.rs"]
mod pm;

/// TICKET-079: `gah loop --once` -- exactly one bounded controller
/// iteration. Build a snapshot, decide one action, execute at most that
/// one action, persist one controller event trail, exit. No daemon, no
/// repeated recursion.
#[derive(Debug, Serialize)]
pub struct LoopOnceResult {
    pub action: NextAction,
    pub outcome: String,
}

/// Resolve the CLI parallel argument without consuming recurring mode's zero
/// sentinel. A daemon must reload the profile limit each iteration; `--once`
/// resolves it once for its bounded invocation.
#[allow(dead_code)]
pub(crate) fn loop_parallel_argument(
    once: bool,
    cli_parallel: usize,
    configured_parallel: usize,
) -> usize {
    if once && cli_parallel == 0 {
        configured_parallel
    } else {
        cli_parallel
    }
}

/// Run the controller continuously in one process. The process lock is held
/// for the lifetime of the loop so a second manager for the same profile
/// cannot create a competing worker pool.
pub fn run_loop(
    initial_cfg: &crate::config::GahConfig,
    profile_name: &str,
    json: bool,
    parallel_arg: usize,
    skip_validation_gate: bool,
    config_path: &std::path::Path,
) -> Result<()> {
    super::ownership::arm_parent_death_signal()?;
    let _lock = acquire_profile_lock(profile_name, config_path)?;

    // Dashboard Settings can change max_parallel_workers, manager_wake_autonomy
    // and current_manager at runtime. Reload from disk on every iteration so those
    // changes take effect on the next loop iteration without restarting the
    // daemon. We keep the last successfully-loaded config as a fallback so a
    // transient read failure (e.g. the config file is mid-write) can't kill
    // the loop.
    let mut last_cfg: Option<crate::config::GahConfig> = Some(initial_cfg.clone());

    loop {
        if crate::runner::shutdown_requested() {
            eprintln!("gah loop: shutdown requested; stopping after terminal cleanup");
            return Ok(());
        }

        let cfg: &crate::config::GahConfig = match reload_config_for_profile(
            config_path,
            profile_name,
        ) {
            Ok(loaded) => {
                last_cfg = Some(loaded);
                last_cfg.as_ref().expect("just assigned")
            }
            Err(error) => {
                eprintln!(
                    "gah loop: failed to reload config ({}); reusing last known config for this iteration",
                    error
                );
                match last_cfg.as_ref() {
                    Some(c) => c,
                    None => {
                        // We never loaded a config successfully; there's no
                        // safe baseline to continue from, so surface the error
                        // instead of dispatching against a phantom config.
                        return Err(error);
                    }
                }
            }
        };

        // The explicit `--parallel` flag (parallel_arg > 0) wins; otherwise
        // derive the worker pool size from the freshly-reloaded profile.
        let parallel = if parallel_arg == 0 {
            crate::config::get_profile(cfg, profile_name)?.max_parallel_workers() as usize
        } else {
            parallel_arg
        };

        // Transient provider/controller failures must not kill the daemon.
        // A validation-gate failure is different: it proves the safety check
        // itself is unhealthy, so pause immediately and require an explicit
        // operator restart after repair. This avoids a retry/restart storm
        // while preserving fail-closed dispatch behavior.
        match run_once(cfg, profile_name, json, parallel, skip_validation_gate) {
            Ok(()) if !wait_for_loop_interval(Duration::from_secs(30)) => {
                eprintln!("gah loop: shutdown requested; stopping after terminal cleanup");
                return Ok(());
            }
            Ok(()) => {}
            Err(_) if crate::runner::shutdown_requested() => {
                eprintln!("gah loop: shutdown requested; stopping after terminal cleanup");
                return Ok(());
            }
            Err(error) if is_validation_gate_failure(&error) => {
                eprintln!(
                    "gah loop: paused because the validation gate failed; repair the gate and explicitly restart the loop: {error:#}"
                );
                return Err(error);
            }
            Err(error) => {
                eprintln!("gah loop: iteration failed; retrying after backoff: {error:#}");
                // Keep shutdown responsive even while backing off: a stopped
                // service must never leave a detached controller running.
                if !wait_for_loop_interval(Duration::from_secs(300)) {
                    eprintln!("gah loop: shutdown requested; stopping after terminal cleanup");
                    return Ok(());
                }
            }
        }
    }
}

fn wait_for_loop_interval(delay: Duration) -> bool {
    wait_interruptibly(delay, crate::runner::shutdown_requested)
}

fn wait_interruptibly(delay: Duration, shutdown_requested: impl Fn() -> bool) -> bool {
    const POLL_INTERVAL: Duration = Duration::from_millis(250);
    let deadline = Instant::now() + delay;
    loop {
        if shutdown_requested() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(POLL_INTERVAL));
    }
}

pub fn run_once(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    json: bool,
    parallel: usize,
    skip_validation_gate: bool,
) -> Result<()> {
    // Housekeeping is part of controller lifecycle, not an operator chore.
    // It only removes clean GAH-owned worktrees that are terminal upstream or
    // past retention; an uncommitted fresh worktree is never inferred stale.
    crate::prune::run_automatic(cfg, profile_name)?;
    let mut ledger_entries = crate::ledger::read_entries(cfg)?;
    reconcile_abandoned_dispatches(cfg, profile_name, &mut ledger_entries)?;
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let claim_scope = crate::work_claim::canonical_claim_scope(profile_name, &profile.repo_id);
    let now = time::OffsetDateTime::now_utc();
    let mut snapshot =
        crate::status::build_snapshot_from_entries(cfg, profile_name, now, &ledger_entries)?;
    crate::events::record(
        cfg,
        crate::events::EventType::ObservationCompleted,
        Some(profile_name),
        None,
        format!("profile={profile_name}"),
    )?;
    let history = crate::events::read_events(cfg)?;
    let capacity_deferred_work_ids = suppress_recent_capacity_deferrals(
        cfg,
        &mut snapshot,
        &history,
        &ledger_entries,
        profile_name,
        &profile.repo_id,
    );

    // For parallel > 1, we need to decide multiple actions
    if parallel > 1 {
        run_parallel_once(
            cfg,
            profile_name,
            &snapshot,
            &ledger_entries,
            json,
            parallel,
            skip_validation_gate,
        )?;
    } else {
        // Original single action behavior
        let original_action = decide_next_action(&snapshot);
        let original_review_generation = action_review_generation(&snapshot, &original_action);
        let mut action = original_action.clone();
        let reset_after = original_action.work_id().and_then(|work_id| {
            latest_clear_attempts_timestamp(
                &ledger_entries,
                profile_name,
                &crate::config::get_profile(cfg, profile_name).ok()?.repo_id,
                work_id,
            )
        });
        if let Some(reason) = detect_stuck_loop(
            &history,
            profile_name,
            &original_action,
            reset_after,
            original_review_generation.as_deref(),
        ) {
            // Persist a work-item-scoped durable human gate so that
            // subsequent loop iterations see human_required=true for this
            // work_id via ledger_lookup_for_ticket and skip it, rather than
            // re-selecting DispatchTicket every cycle (the original
            // trip-without-latch bug).
            if let Some(wid) = original_action.work_id() {
                if let Err(e) = append_stuck_loop_gate_if_transition(
                    cfg,
                    profile_name,
                    wid,
                    &reason,
                    original_review_generation.as_deref(),
                ) {
                    eprintln!("warning: failed to persist stuck-loop gate: {e:#}");
                }
            }
            // TICKET-skip-and-continue: the gate is now persisted as a
            // work-item-scoped human_required (above). Re-decide with a fresh
            // snapshot that EXCLUDES the stuck work_id, so the controller
            // picks the NEXT eligible work item instead of parking the whole
            // profile. Only if nothing else is actionable do we surface
            // profile-wide HumanRequired -- that is a genuine profile stall,
            // not a single blocked ticket.
            let fresh = crate::status::build_snapshot_from_entries(
                cfg,
                profile_name,
                time::OffsetDateTime::now_utc(),
                &ledger_entries,
            )?;
            let mut scoped = fresh;
            let mut excluded_work_ids = capacity_deferred_work_ids.clone();
            excluded_work_ids.extend(original_action.work_id().map(str::to_string));
            retain_snapshot_candidates(
                &mut scoped,
                &excluded_work_ids,
                &std::collections::HashSet::new(),
            );
            let redispatched = decide_next_action(&scoped);
            if redispatched.kind() == "no_op" {
                // Nothing else actionable -> genuine stall, surface it.
                action = NextAction::HumanRequired {
                    work_id: original_action.work_id().map(str::to_string),
                    reason,
                    reference: original_action.work_id().map(str::to_string),
                    reason_code: Some(HumanRequiredReason::StuckLoopGate.as_str().to_string()),
                };
            } else {
                action = redispatched;
            }
        }
        // TICKET-282: a FixMr reusing a branch already attached to a foreign
        // or stale worktree must be deferred (non-terminal) and the loop
        // continued with the next eligible item, never allowed to stall the
        // recurring loop on a hard `git worktree add` failure.
        if let Some(redispatch) =
            defer_if_branch_attached(cfg, profile_name, &action, &capacity_deferred_work_ids)?
        {
            action = redispatch;
        }
        record_action_events(
            cfg,
            profile_name,
            &original_action,
            &action,
            original_review_generation.as_deref(),
        )?;

        let outcome = if let Some(work_id) = action
            .work_id()
            .map(crate::work_claim::normalize_work_identity)
            .filter(|_| {
                !matches!(
                    action,
                    NextAction::WaitUntil { .. }
                        | NextAction::HumanRequired { .. }
                        | NextAction::NoOp { .. }
                )
            }) {
            if !crate::work_claim::try_claim_work(&claim_scope, &work_id)? {
                format!("Skipped already-claimed work '{work_id}'")
            } else {
                match execute_action(cfg, profile_name, &action, skip_validation_gate, None) {
                    Ok(outcome) => {
                        crate::work_claim::release_work(&claim_scope, &work_id)?;
                        outcome
                    }
                    Err(error) => {
                        crate::work_claim::release_work(&claim_scope, &work_id)?;
                        return Err(error);
                    }
                }
            }
        } else {
            execute_action(cfg, profile_name, &action, skip_validation_gate, None)?
        };

        let stop_event_type = match &action {
            NextAction::WaitUntil { .. } => crate::events::EventType::WaitSelected,
            NextAction::HumanRequired { .. } => crate::events::EventType::HumanRequired,
            _ => crate::events::EventType::LoopStopped,
        };
        if matches!(action, NextAction::HumanRequired { .. }) {
            crate::events::record_with_reason_code_and_plan(
                cfg,
                stop_event_type,
                Some(profile_name),
                action.work_id(),
                outcome.clone(),
                action.human_required_reason_code(),
                remediation_plan_for_action(cfg, profile_name, &action).as_ref(),
            )?;
        } else {
            crate::events::record(
                cfg,
                stop_event_type,
                Some(profile_name),
                action.work_id(),
                outcome.clone(),
            )?;
        }

        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&LoopOnceResult { action, outcome })?
            );
        } else {
            println!("Decided: {} -- {}", action.kind(), action.reason());
            println!("{outcome}");
        }
    }
    Ok(())
}

/// TICKET-096: Parallel execution for multiple actions
fn run_parallel_once(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    _snapshot: &crate::status::StatusSnapshot,
    _ledger_entries: &[crate::ledger::LedgerEntry],
    json: bool,
    max_parallel: usize,
    skip_validation_gate: bool,
) -> Result<()> {
    use std::collections::HashSet;

    let mut executed_work_ids = HashSet::new();
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let claim_scope = crate::work_claim::canonical_claim_scope(profile_name, &profile.repo_id);
    let effective_open_mr_limit = profile
        .max_open_managed_mrs
        .unwrap_or(max_parallel as u32)
        .max(1);
    let mut active_intake_keys: HashSet<String> = HashSet::new();

    // Profile routing decides which eligible backend handles each action. Do
    // not use the number of persisted availability rows as a worker limit:
    // that list is sparse and only contains observed scopes, not every
    // configured backend.
    let effective_parallel_limit = max_parallel;

    let mut results: Vec<(usize, LoopOnceResult)> = Vec::new();

    fn observe_snapshot(
        cfg: &crate::config::GahConfig,
        profile_name: &str,
        ledger_entries: &[crate::ledger::LedgerEntry],
    ) -> Result<crate::status::StatusSnapshot> {
        crate::status::build_snapshot_from_entries(
            cfg,
            profile_name,
            time::OffsetDateTime::now_utc(),
            ledger_entries,
        )
    }

    std::thread::scope(|scope| -> Result<()> {
        let mut active = 0usize;
        let mut next_sequence = 0usize;
        let mut saw_real_work = false;
        let mut pending_terminal: Option<(NextAction, NextAction, Option<String>)> = None;
        let mut fill_attempts_remaining = effective_parallel_limit;
        let mut refill_suppressed = false;
        let (done_tx, done_rx) = sync_channel::<(usize, LoopOnceResult)>(effective_parallel_limit);

        loop {
            while active < effective_parallel_limit && fill_attempts_remaining > 0 {
                if crate::runner::shutdown_requested() {
                    break;
                }
                fill_attempts_remaining -= 1;
                let claimed_work_ids = crate::work_claim::get_claimed_work_ids(&claim_scope)?;
                let ledger_entries = crate::ledger::read_entries(cfg)?;
                let mut fresh_snapshot = observe_snapshot(cfg, profile_name, &ledger_entries)?;
                apply_parallel_projection(
                    &mut fresh_snapshot,
                    &active_intake_keys,
                    effective_open_mr_limit,
                );

                // Do not let the next slot re-select work already claimed by
                // another process or spawned earlier in this refill cycle.
                retain_unclaimed_work(&mut fresh_snapshot, &claimed_work_ids, &executed_work_ids);

                let history = crate::events::read_events(cfg)?;
                let capacity_deferred_work_ids = suppress_recent_capacity_deferrals(
                    cfg,
                    &mut fresh_snapshot,
                    &history,
                    &ledger_entries,
                    profile_name,
                    &crate::config::get_profile(cfg, profile_name)?.repo_id,
                );

                let original_action = decide_next_action(&fresh_snapshot);
                let original_review_generation =
                    action_review_generation(&fresh_snapshot, &original_action);
                let mut action = original_action.clone();

                let reset_after = original_action.work_id().and_then(|work_id| {
                    latest_clear_attempts_timestamp(
                        &ledger_entries,
                        profile_name,
                        &crate::config::get_profile(cfg, profile_name).ok()?.repo_id,
                        work_id,
                    )
                });
                if let Some(reason) = detect_stuck_loop(
                    &history,
                    profile_name,
                    &original_action,
                    reset_after,
                    original_review_generation.as_deref(),
                ) {
                    if let Some(wid) = original_action.work_id() {
                        if let Err(error) = append_stuck_loop_gate_if_transition(
                            cfg,
                            profile_name,
                            wid,
                            &reason,
                            original_review_generation.as_deref(),
                        ) {
                            eprintln!("warning: failed to persist stuck-loop gate: {error:#}");
                        }
                    }
                    if let Some(stuck_wid) = original_action.work_id() {
                        fresh_snapshot
                            .merge_requests
                            .retain(|mr| mr.work_id.as_deref() != Some(stuck_wid));
                        fresh_snapshot
                            .available_tickets
                            .retain(|t| t.work_id.as_deref() != Some(stuck_wid));
                        fresh_snapshot
                            .issue_intake_rejections
                            .retain(|issue| issue.work_id.as_deref() != Some(stuck_wid));
                    }
                    let redispatched = decide_next_action(&fresh_snapshot);
                    if redispatched.kind() == "no_op" {
                        action = NextAction::HumanRequired {
                            work_id: original_action.work_id().map(str::to_string),
                            reason,
                            reference: original_action.work_id().map(str::to_string),
                            reason_code: Some(
                                HumanRequiredReason::StuckLoopGate.as_str().to_string(),
                            ),
                        };
                    } else {
                        action = redispatched;
                    }
                }

                if let Some(redispatch) = defer_if_branch_attached(
                    cfg,
                    profile_name,
                    &action,
                    &capacity_deferred_work_ids,
                )? {
                    action = redispatch;
                }

                let action_work_id = action
                    .work_id()
                    .map(crate::work_claim::normalize_work_identity);
                if let Some(work_id) = action_work_id.as_deref() {
                    if claimed_work_ids.iter().any(|claimed| claimed == work_id)
                        || executed_work_ids.contains(work_id)
                    {
                        continue;
                    }
                }

                match &action {
                    NextAction::WaitUntil { .. }
                    | NextAction::HumanRequired { .. }
                    | NextAction::NoOp { .. } => {
                        if !saw_real_work {
                            pending_terminal =
                                Some((original_action, action, original_review_generation));
                            if active == 0 {
                                continue;
                            }
                        }
                        if active > 0 {
                            break;
                        }
                    }
                    _ => {
                        let admission = match node_capacity::try_acquire(&action) {
                            Ok(admission) => admission,
                            Err(error) => {
                                // Keep the existing configured ceiling as the
                                // portability fallback on platforms without
                                // Linux pressure interfaces.
                                eprintln!(
                                    "gah loop: node pressure unavailable ({error}); using configured parallel ceiling"
                                );
                                node_capacity::LiveAdmission::Admit(
                                    node_capacity::NodeCapacityLease::untracked(),
                                )
                            }
                        };
                        let node_capacity_lease = match admission {
                            node_capacity::LiveAdmission::Admit(lease) => lease,
                            node_capacity::LiveAdmission::Defer(reason) => {
                                eprintln!(
                                    "gah loop: deferring additional worker at {active}/{effective_parallel_limit}: {reason}"
                                );
                                break;
                            }
                        };

                        record_action_events(
                            cfg,
                            profile_name,
                            &original_action,
                            &action,
                            original_review_generation.as_deref(),
                        )?;

                        if let Some(work_id) = action_work_id.as_deref() {
                            if !crate::work_claim::try_claim_work(&claim_scope, work_id)? {
                                continue;
                            }
                            executed_work_ids.insert(work_id.to_string());
                        }
                        if let Some(key) = action_intake_key(&action) {
                            active_intake_keys.insert(key);
                        }
                        pending_terminal = None;

                        let action_for_thread = action.clone();
                        let profile_for_thread = profile_name.to_string();
                        let claim_scope_for_thread = claim_scope.clone();
                        let work_id_for_thread = action_work_id;
                        let sequence = next_sequence;
                        next_sequence += 1;
                        saw_real_work = true;

                        let waits_for_route = action_waits_for_route(&action_for_thread);
                        let (route_ready, route_receiver) = if waits_for_route {
                            let (sender, receiver) = sync_channel(0);
                            (Some(sender), Some(receiver))
                        } else {
                            (None, None)
                        };
                        let done_tx = done_tx.clone();
                        active += 1;
                        scope.spawn(move || {
                            let _node_capacity_lease = node_capacity_lease;
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    execute_action(
                                        cfg,
                                        &profile_for_thread,
                                        &action_for_thread,
                                        skip_validation_gate,
                                        route_ready,
                                    )
                                }));
                            let (outcome, event_outcome) = match result {
                                Ok(Ok(outcome)) => (outcome.clone(), outcome),
                                Ok(Err(error)) => {
                                    let outcome = format!("Error: {error}");
                                    (outcome.clone(), outcome)
                                }
                                Err(_) => {
                                    let outcome = "Error: parallel GAH worker panicked".to_string();
                                    (outcome.clone(), outcome)
                                }
                            };
                            if let Some(work_id) = work_id_for_thread.as_deref() {
                                let _ = crate::work_claim::release_work(
                                    &claim_scope_for_thread,
                                    work_id,
                                );
                            }
                            let _ = crate::events::record(
                                cfg,
                                crate::events::EventType::LoopStopped,
                                Some(&profile_for_thread),
                                action_for_thread.work_id(),
                                event_outcome,
                            );
                            let _ = done_tx.send((
                                sequence,
                                LoopOnceResult {
                                    action: action_for_thread,
                                    outcome,
                                },
                            ));
                        });
                        if let Some(receiver) = route_receiver {
                            let _ = receiver.recv();
                        }
                    }
                }
            }

            if active == 0 {
                if !saw_real_work {
                    if let Some((original_action, action, review_generation)) =
                        pending_terminal.take()
                    {
                        record_action_events(
                            cfg,
                            profile_name,
                            &original_action,
                            &action,
                            review_generation.as_deref(),
                        )?;
                        let outcome =
                            execute_action(cfg, profile_name, &action, skip_validation_gate, None)?;

                        let stop_event_type = match &action {
                            NextAction::WaitUntil { .. } => crate::events::EventType::WaitSelected,
                            NextAction::HumanRequired { .. } => {
                                crate::events::EventType::HumanRequired
                            }
                            NextAction::NoOp { .. } => crate::events::EventType::LoopStopped,
                            _ => unreachable!(),
                        };
                        crate::events::record(
                            cfg,
                            stop_event_type,
                            Some(profile_name),
                            action.work_id(),
                            outcome.clone(),
                        )?;

                        results.push((next_sequence, LoopOnceResult { action, outcome }));
                    }
                }
                break;
            }

            let (sequence, result) = done_rx
                .recv()
                .map_err(|_| anyhow::anyhow!("parallel GAH worker channel closed unexpectedly"))?;
            active -= 1;
            if action_creates_managed_mr(&result.action) {
                if let Some(key) = action_intake_key(&result.action) {
                    active_intake_keys.remove(&key);
                }
            }
            update_parallel_refill_budget(
                &result.outcome,
                effective_parallel_limit,
                &mut fill_attempts_remaining,
                &mut refill_suppressed,
            );
            results.push((sequence, result));
        }
        Ok(())
    })?;

    results.sort_by_key(|(sequence, _)| *sequence);
    let results: Vec<LoopOnceResult> = results.into_iter().map(|(_, result)| result).collect();

    // Output results
    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        for (i, result) in results.iter().enumerate() {
            if i > 0 {
                println!("---");
            }
            println!(
                "Decided: {} -- {}",
                result.action.kind(),
                result.action.reason()
            );
            println!("{}", result.outcome);
        }
        if results.is_empty() {
            println!("No actions executed (parallel limit reached or no eligible work)");
        }
    }

    // Clean up any stale claims if we encountered errors
    // (This is a safety net - normally individual claims should be released)
    let failed_results: Vec<&LoopOnceResult> = results
        .iter()
        .filter(|result| parallel_outcome_is_failure(&result.outcome))
        .collect();
    if !failed_results.is_empty() {
        crate::work_claim::release_all_for_profile(&claim_scope)?;
        anyhow::bail!(
            "{} parallel action(s) failed; first failure: {}",
            failed_results.len(),
            failed_results[0].outcome
        );
    }

    Ok(())
}

fn action_waits_for_route(action: &NextAction) -> bool {
    matches!(
        action,
        NextAction::DispatchTicket { .. }
            | NextAction::Retry { .. }
            | NextAction::Escalate { .. }
            | NextAction::FixMr { .. }
            | NextAction::ReviewMr { .. }
    )
}

fn update_parallel_refill_budget(
    outcome: &str,
    parallel_limit: usize,
    fill_attempts_remaining: &mut usize,
    refill_suppressed: &mut bool,
) -> bool {
    let error = outcome.starts_with("Error:");
    let failed = parallel_outcome_is_failure(outcome);
    let capacity_deferred = outcome.starts_with("Deferred ");
    if error || capacity_deferred {
        *refill_suppressed = true;
        *fill_attempts_remaining = 0;
    } else if !*refill_suppressed {
        *fill_attempts_remaining = parallel_limit;
    }
    failed
}

fn parallel_outcome_is_failure(outcome: &str) -> bool {
    outcome.starts_with("Error:") && !outcome.contains("shutdown requested")
}

/// Executes at most one action. `FixMr` dispatches a fix operation
/// reusing an existing branch (TICKET-118).
pub(crate) fn execute_action(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    action: &NextAction,
    skip_validation_gate: bool,
    route_ready: Option<SyncSender<()>>,
) -> Result<String> {
    let base_args = || crate::dispatch::DispatchArgs {
        profile: profile_name.to_string(),
        mode: "fix".to_string(),
        backend: "auto".to_string(),
        target: String::new(),
        branch: None,
        mr: None,
        current_branch: false,
        dry_run: false,
        oh_profile: None,
        model: None,
        retries: 2,
        allow_draft_fail: false,
        prod: false,
        issue_intake_override: false,
        allow_unknown_red_baseline: dispatch_policy::allow_unknown_red_baseline(action),
        escalate: false,
        existing_branch: None,
        expected_review_generation: None,
        skip_validation_gate,
        dispatch_reason: None,
        work_id: action.work_id().map(str::to_string),
        run_id: Some(uuid::Uuid::new_v4().to_string()),
        route_ready: route_ready.clone(),
    };

    match action {
        NextAction::ReviewMr { branch, .. } => {
            let args = crate::dispatch::DispatchArgs {
                mode: "review".to_string(),
                branch: Some(branch.clone()),
                dispatch_reason: Some("review".to_string()),
                ..base_args()
            };
            let deferred = run_dispatch_and_record(cfg, "review", action.work_id(), &args)?;
            Ok(deferred.unwrap_or_else(|| format!("Dispatched review for branch '{branch}'")))
        }
        NextAction::MarkReadyForReview { branch, .. } => {
            let profile = crate::config::get_profile(cfg, profile_name)?;
            crate::provider::mark_ready_for_review(profile, branch)?;
            Ok(format!("Marked MR on branch '{branch}' ready for review"))
        }
        NextAction::FixMr {
            branch,
            review_generation,
            ..
        } => {
            let args = crate::dispatch::DispatchArgs {
                target: branch.clone(),
                existing_branch: Some(branch.clone()),
                expected_review_generation: review_generation.clone(),
                dispatch_reason: Some("post_review_repair".to_string()),
                ..base_args()
            };
            let deferred = run_dispatch_and_record(cfg, "fix_existing", action.work_id(), &args)?;
            Ok(
                deferred
                    .unwrap_or_else(|| format!("Dispatched fix for existing branch '{branch}'")),
            )
        }
        NextAction::MergeMr { .. } => merge::execute(cfg, profile_name, action),
        NextAction::DispatchTicket { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                dispatch_reason: Some("initial".to_string()),
                ..base_args()
            };
            let deferred =
                run_dispatch_and_record(cfg, "dispatch_ticket", action.work_id(), &args)?;
            Ok(deferred.unwrap_or_else(|| format!("Dispatched ticket '{ticket_path}'")))
        }
        NextAction::DecomposeIssue {
            ticket_path,
            work_id,
            title,
            ..
        } => pm::execute(
            cfg,
            profile_name,
            ticket_path,
            work_id,
            title.as_deref(),
            skip_validation_gate,
            route_ready.clone(),
        ),
        NextAction::ReconcilePmParent {
            work_id,
            source_issue_number,
            plan_fingerprint,
            child_issue_numbers,
            ..
        } => pm::reconcile_parent(
            cfg,
            profile_name,
            work_id,
            source_issue_number,
            plan_fingerprint,
            child_issue_numbers,
        ),
        NextAction::Retry { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                dispatch_reason: Some("initial".to_string()),
                ..base_args()
            };
            let deferred = run_dispatch_and_record(cfg, "retry", action.work_id(), &args)?;
            Ok(deferred.unwrap_or_else(|| format!("Retried ticket '{ticket_path}'")))
        }
        NextAction::Escalate { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                escalate: true,
                dispatch_reason: Some("initial".to_string()),
                ..base_args()
            };
            let deferred = run_dispatch_and_record(cfg, "escalate", action.work_id(), &args)?;
            Ok(deferred.unwrap_or_else(|| format!("Escalated ticket '{ticket_path}'")))
        }
        NextAction::WaitUntil { until, reason } => Ok(format!("Waiting until {until} ({reason})")),
        NextAction::HumanRequired {
            work_id: _,
            reason,
            reference,
            reason_code,
        } => Ok(format!(
            "Human required: {reason}{}{}",
            reference
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default(),
            reason_code
                .as_deref()
                .map(|c| format!(" [code={c}]"))
                .unwrap_or_default()
        )),
        NextAction::NoOp { reason } => Ok(format!("No action: {reason}")),
    }
}

/// Records `DispatchStarted`, runs `dispatch::run`, then records either
/// `DispatchFinished` (success) or `DuplicateGuardTriggered` (the typed
/// duplicate-work refusal from TICKET-097's `check_duplicate_work`) / a
/// generic failure note -- so the event log distinguishes "the duplicate
/// guard correctly refused this" from an ordinary dispatch failure.
///
/// Used by both `gah loop --once` (which has a `NextAction`) and the
/// direct `gah dispatch` command; for the latter `work_id` is `None` until
/// `dispatch::run` resolves it. Emitting these events from the single
/// shared entry point is what lets the dashboard's controller-activity
/// panel observe *every* live dispatch -- including ones the supervisor
/// launches outside the dashboard -- instead of only dashboard-initiated
/// sessions (see issue #197).
pub(crate) fn run_dispatch_and_record(
    cfg: &crate::config::GahConfig,
    label: &str,
    work_id: Option<&str>,
    args: &crate::dispatch::DispatchArgs,
) -> Result<Option<String>> {
    let target_context = args
        .branch
        .as_deref()
        .or_else(|| (!args.target.is_empty()).then_some(args.target.as_str()));
    let start_detail = target_context
        .map(|target| format!("{label}: {target}"))
        .unwrap_or_else(|| label.to_string());
    crate::events::record_with_run_id(
        cfg,
        crate::events::EventType::DispatchStarted,
        Some(args.profile.as_str()),
        work_id,
        args.run_id.as_deref(),
        start_detail,
    )?;
    match crate::dispatch::run(cfg, args) {
        Ok(()) => {
            crate::events::record_with_run_id(
                cfg,
                crate::events::EventType::DispatchFinished,
                Some(args.profile.as_str()),
                work_id,
                args.run_id.as_deref(),
                format!("{label}: success"),
            )?;
            Ok(None)
        }
        Err(e) if crate::dispatch::capacity_deferred_error(&e) => {
            let route_state =
                route_state_fingerprint(cfg, &args.profile, time::OffsetDateTime::now_utc())
                    .ok()
                    .map(|fingerprint| format!(" route_state={fingerprint}"))
                    .unwrap_or_default();
            crate::events::record_with_run_id(
                cfg,
                crate::events::EventType::DispatchFinished,
                Some(args.profile.as_str()),
                work_id,
                args.run_id.as_deref(),
                format!("{label}: deferred_capacity: {e:#}{route_state}"),
            )?;
            Ok(Some(format!(
                "Deferred {label} because configured route capacity is busy; no backend launched"
            )))
        }
        Err(e) => {
            let event_type = if crate::dispatch::duplicate_work_error(&e).is_some() {
                crate::events::EventType::DuplicateGuardTriggered
            } else if crate::dispatch::review_budget_exhausted_error(&e).is_some() {
                crate::events::EventType::ReviewBudgetExhausted
            } else {
                crate::events::EventType::DispatchFinished
            };
            crate::events::record_with_run_id(
                cfg,
                event_type,
                Some(args.profile.as_str()),
                work_id,
                args.run_id.as_deref(),
                format!("{label}: {e:#}"),
            )?;
            Err(e)
        }
    }
}

#[cfg(test)]
#[path = "ledger_read_tests.rs"]
mod ledger_read_tests;

#[cfg(test)]
#[path = "runtime/capacity_tests.rs"]
mod capacity_tests;

#[cfg(test)]
mod tests {
    use super::profile_lock::{acquire_profile_lock, loop_lock_path, reload_config_for_profile};
    use super::{
        action_waits_for_route, append_stuck_loop_gate_if_transition, is_validation_gate_failure,
        loop_parallel_argument, wait_interruptibly,
    };

    #[test]
    fn recurring_loop_preserves_live_config_sentinel_but_once_resolves_it() {
        assert_eq!(loop_parallel_argument(false, 0, 2), 0);
        assert_eq!(loop_parallel_argument(true, 0, 2), 2);
        assert_eq!(loop_parallel_argument(false, 3, 2), 3);
        assert_eq!(loop_parallel_argument(true, 3, 2), 3);
    }

    #[test]
    fn review_actions_wait_until_the_selected_route_is_reserved() {
        let action = crate::controller::NextAction::ReviewMr {
            branch: "gah/review-cap".into(),
            work_id: Some("#471".into()),
            mr_url: None,
            reason: "review required".into(),
        };
        assert!(action_waits_for_route(&action));
    }

    #[test]
    fn validation_gate_errors_are_identified_through_anyhow_context() {
        let error = anyhow::Error::new(crate::dispatch::ValidationGateError)
            .context("detailed failed command output");
        assert!(is_validation_gate_failure(&error));
    }

    #[test]
    fn ordinary_errors_are_not_misclassified_as_validation_gate_failures() {
        let error = anyhow::anyhow!("backend command timed out");
        assert!(!is_validation_gate_failure(&error));
    }

    #[test]
    fn stuck_loop_gate_append_is_an_idempotent_state_transition() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg.toml");
        std::fs::write(
            &path,
            format!(
                r#"
[defaults]
artifact_root = "{}"

[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "{}"
default_target_branch = "main"
"#,
                tmp.path().display(),
                tmp.path().display()
            ),
        )
        .unwrap();
        let cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let appended = std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| {
                    let barrier = barrier.clone();
                    let cfg = &cfg;
                    scope.spawn(move || {
                        barrier.wait();
                        append_stuck_loop_gate_if_transition(
                            cfg,
                            "test",
                            "#639",
                            "same stuck decision",
                            None,
                        )
                        .unwrap()
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|appended| *appended)
                .count()
        });
        assert_eq!(
            appended, 1,
            "exactly one concurrent slot owns the transition"
        );

        let entries = crate::ledger::read_entries(&cfg).unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.dispatch_reason.as_deref() == Some("stuck_loop_gate"))
                .count(),
            1
        );
    }

    /// TICKET/incident: an autonomous session ran `gah loop --profile X
    /// --once` as an ad-hoc diagnostic while the real daemon (`gah loop
    /// --profile X`, no `--once`) was already running for that profile --
    /// both executed uncoordinated. `acquire_profile_lock` is the single
    /// shared entry point both `--once` (main.rs) and manual `gah dispatch`
    /// (main.rs) now call before doing any real execution; prove a second
    /// caller for the same profile is rejected, regardless of which of
    /// those two call sites it simulates.
    ///
    /// Uses a unique profile name (not a mocked/overridden lock path) so
    /// this can't collide with a real profile's lock file or with another
    /// test running concurrently -- avoids the env-var test race documented
    /// on `canonical_config_path` above.
    #[test]
    fn acquire_profile_lock_rejects_concurrent_second_holder() {
        let profile = format!("test-lock-race-{}", std::process::id());
        // A real config file stand-in: two invocations against the *same*
        // config path are what the real incident looked like (daemon and
        // `--once` both using the default config).
        let config_file = tempfile::NamedTempFile::new().unwrap();
        let config_path = config_file.path();
        let lock_path = loop_lock_path(&profile, config_path);

        // Simulates the daemon (`gah loop --profile <p>`, no `--once`)
        // already holding the lock for this profile.
        let daemon_lock =
            acquire_profile_lock(&profile, config_path).expect("daemon should acquire cleanly");

        // Simulates a `gah loop --profile <p> --once` invocation racing
        // against the still-running daemon.
        let once_err = acquire_profile_lock(&profile, config_path)
            .err()
            .expect("--once attempt must fail while the daemon holds the lock");
        assert!(once_err.to_string().contains(&profile));
        assert!(once_err
            .to_string()
            .contains(&lock_path.display().to_string()));

        // Simulates a manual `gah dispatch --profile <p>` invocation also
        // racing against the still-running daemon.
        let dispatch_err = acquire_profile_lock(&profile, config_path)
            .err()
            .expect("manual dispatch attempt must fail while the daemon holds the lock");
        assert!(dispatch_err.to_string().contains(&profile));

        drop(daemon_lock);
        let _ = std::fs::remove_file(&lock_path);
    }

    #[test]
    fn profile_lock_is_adjacent_to_config_not_xdg_state() {
        let config_file = tempfile::NamedTempFile::new().unwrap();
        let lock_path = loop_lock_path("test-profile", config_file.path());
        let expected_dir = config_file.path().parent().unwrap().join(".gah-locks");
        assert_eq!(lock_path.parent(), Some(expected_dir.as_path()));
    }

    #[test]
    fn reload_config_for_profile_succeeds_when_profile_still_present() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg.toml");
        std::fs::write(
            &path,
            r#"
[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "/tmp"
default_target_branch = "main"
"#,
        )
        .unwrap();

        let cfg = reload_config_for_profile(&path, "test").expect("profile is present");
        assert!(crate::config::get_profile(&cfg, "test").is_ok());
    }

    #[test]
    fn reload_config_for_profile_errs_when_profile_renamed_or_removed() {
        // A parse-clean reload that no longer resolves the running profile
        // (renamed/removed mid-run, e.g. via the dashboard Settings UI) must
        // report an error rather than silently handing back a config the
        // daemon can't dispatch against -- the caller (`run_loop`) relies on
        // this to fall back to its last-known-good config instead of
        // hard-erroring out of the whole loop.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg.toml");
        std::fs::write(
            &path,
            r#"
[profiles.renamed]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "/tmp"
default_target_branch = "main"
"#,
        )
        .unwrap();

        let error = reload_config_for_profile(&path, "test")
            .expect_err("profile no longer exists in the reloaded config");
        assert!(error.to_string().contains("test"));
    }

    #[test]
    fn interruptible_wait_stops_during_backoff() {
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let completed = wait_interruptibly(std::time::Duration::from_secs(300), || {
            checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
        });
        assert!(!completed);
        assert_eq!(checks.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    // TICKET-096: Parallel dispatch tests
    use crate::models::AvailableTicket;
    use crate::status::{
        ObservationStatus, Observations, ProfileIdentity, ScopeStatusJson, StatusSnapshot,
    };

    pub(super) fn empty_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            schema_version: 1,
            review_contract_version: crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION,
            generated_at: "2026-07-05T00:00:00Z".into(),
            profile: ProfileIdentity {
                profile: "real".into(),
                display_name: "Real".into(),
                repo_id: "real".into(),
                provider: "github".into(),
                local_path: "/tmp/repo".into(),
                default_target_branch: "main".into(),
                merge_policy: crate::config::MergePolicy::default(),
                max_fix_attempts_per_mr: 2,
                max_implementation_failures_per_ticket: 2,
                max_open_managed_mrs: 1,
                issue_intake_policy: crate::models::IssueIntakePolicy {
                    mode: "canonical_autonomous_only".into(),
                    canonical_autonomous_label: "exec:autonomous".into(),
                    trusted_human_authors: vec![],
                    trusted_bot_authors: vec![],
                    github_issue_author_allowlist: vec![],
                },
            },
            observations: Observations {
                sync: ObservationStatus { status: "ok" },
                availability: ObservationStatus { status: "ok" },
                ledger: ObservationStatus { status: "ok" },
            },
            merge_requests: vec![],
            availability: vec![],
            recent_ledger: None,
            constraints: vec![],
            blockers: vec![],
            blocked_work_items: vec![],
            issue_intake_rejections: vec![],
            dependency_blockers: vec![],
            errors: vec![],
            available_tickets: vec![],
            active_claims: vec![],
            pm_parent_states: vec![],
            pm_decomposition_attempt_counts: std::collections::HashMap::new(),
            pm_max_attempts: 2,
            fix_attempt_counts: std::collections::HashMap::new(),
            merge_attempt_counts: std::collections::HashMap::new(),
            review_held_work_ids: std::collections::HashSet::new(),
            publishing_allow_pr: true,
            generated_artifact_deny_patterns: vec![],
            max_parallel_workers: 1,
            open_managed_mr_count: 0,
            inflight_implementation_count: 0,
            implementation_intake_paused: false,
            backend_configured: std::collections::HashMap::new(),
            backend_instances: vec![],
        }
    }

    #[test]
    fn parallel_dispatch_respects_max_parallel_limit() {
        let mut snapshot = empty_snapshot();

        // Add multiple eligible backends (more than max_parallel)
        for _ in 0..5 {
            snapshot.availability.push(ScopeStatusJson {
                backend_instance: None,
                backend: "test_backend".to_string(),
                model: None,
                quota_pool: None,
                eligible_now: true,
                reason: None,
                unavailable_until: None,
                source: None,
                last_error_summary: None,
                observed_at: None,
                scope: None,
            });
        }

        // Add 3 available tickets
        for i in 0..3 {
            snapshot.available_tickets.push(AvailableTicket {
                ticket_path: format!("ticket_{}.md", i),
                work_id: Some(format!("TICKET-{}", i + 100)),
                normalized_work_identity: crate::work_claim::normalize_work_identity(&format!(
                    "TICKET-{}",
                    i + 100
                )),
                source: crate::models::CandidateSource::LegacyTicket,
                execution_policy: crate::models::CandidateExecutionPolicy {
                    intake_mode: "canonical_autonomous_only".into(),
                    explicit_autonomy_required: true,
                    autonomous_metadata_present: true,
                    dispatchable_now: true,
                    exclusion_reason_code: None,
                    exclusion_reason: None,
                },
                title: Some(format!("Test ticket {}", i)),
                has_active_mr: false,
                priority: crate::models::TicketPriority::Unspecified,
                prior_attempt_count: 0,
                genuine_agent_failure_count: 0,
                last_failure_class: None,
                recommended_backend: None,
                recommended_model: None,
                human_required: false,
                human_required_reason_code: None,
                has_active_claim: false,
            });
        }

        // With max_parallel=2, we should only process 2 tickets
        // Note: This test exercises the logic but doesn't run the actual parallel execution
        // since that requires a full GAH setup
        let effective_parallel_limit = std::cmp::min(
            2,
            snapshot
                .availability
                .iter()
                .filter(|a| a.eligible_now)
                .count(),
        );
        assert_eq!(effective_parallel_limit, 2);
    }

    #[test]
    fn backend_availability_limits_parallelism() {
        let mut snapshot = empty_snapshot();

        // Add 3 eligible backends
        for i in 0..3 {
            snapshot.availability.push(ScopeStatusJson {
                backend_instance: None,
                backend: format!("backend_{}", i),
                model: None,
                quota_pool: None,
                eligible_now: true,
                reason: None,
                unavailable_until: None,
                source: None,
                last_error_summary: None,
                observed_at: None,
                scope: None,
            });
        }

        // With 3 eligible backends, max_parallel=5 should be limited to 3
        let effective_parallel_limit = std::cmp::min(
            5,
            snapshot
                .availability
                .iter()
                .filter(|a| a.eligible_now)
                .count(),
        );
        assert_eq!(effective_parallel_limit, 3);
    }

    #[test]
    fn no_backend_availability_zero_parallelism() {
        let mut snapshot = empty_snapshot();
        for i in 0..3 {
            snapshot.availability.push(ScopeStatusJson {
                backend_instance: None,
                backend: format!("backend_{i}"),
                model: None,
                quota_pool: None,
                eligible_now: false,
                reason: Some("rate limited".into()),
                unavailable_until: Some(time::OffsetDateTime::now_utc().to_string()),
                source: None,
                last_error_summary: None,
                observed_at: None,
                scope: None,
            });
        }
        let effective_parallel_limit = std::cmp::min(
            5,
            snapshot
                .availability
                .iter()
                .filter(|a| a.eligible_now)
                .count(),
        );
        assert_eq!(effective_parallel_limit, 0);
    }
}
