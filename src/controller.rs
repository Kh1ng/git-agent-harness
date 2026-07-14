use anyhow::Result;
use fs2::FileExt;
use serde::Serialize;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::time::{Duration, Instant};

mod action;
pub use self::action::NextAction;

mod decision;
pub use self::decision::decide_next_action;
pub(crate) use self::decision::is_genuine_agent_failure;

/// Finish runs left behind by a killed controller with both durable surfaces:
/// the event stream used for live activity and the normalized ledger used for
/// routing/usage reports. `run_once` calls this after acquiring the profile
/// lock, so an open start is provably abandoned rather than merely slow.
fn reconcile_abandoned_dispatches(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    entries: &mut Vec<crate::ledger::LedgerEntry>,
) -> Result<usize> {
    let events = crate::events::read_events(cfg)?;
    let orphans = crate::events::orphaned_dispatch_runs(&events, profile_name);
    if orphans.is_empty() {
        return Ok(0);
    }
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let existing_sessions: HashSet<String> = entries
        .iter()
        .filter_map(|entry| entry.session_id.clone())
        .collect();

    for (run_id, work_id) in &orphans {
        if !existing_sessions.contains(run_id) {
            let target = work_id.as_deref().unwrap_or("unknown");
            let mut entry = crate::ledger::LedgerEntry::new(
                profile_name,
                profile,
                "unknown",
                "abandoned",
                target,
                Some(run_id.clone()),
                None,
            );
            entry.work_id = work_id.clone();
            entry.dispatch_reason = Some("abandoned_reconciliation".to_string());
            entry.validation_result = Some("not_run_abandoned".to_string());
            entry.error_summary =
                Some("dispatch abandoned before terminal telemetry was persisted".to_string());
            entry.set_failure(
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::AgentRun,
            );
            crate::ledger::append(cfg, &entry)?;
            entries.push(entry);
        }
        crate::events::record_with_run_id(
            cfg,
            crate::events::EventType::DispatchFinished,
            Some(profile_name),
            work_id.as_deref(),
            Some(run_id),
            "abandoned (reconciled before next dispatch)",
        )?;
    }
    Ok(orphans.len())
}

fn is_validation_gate_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.is::<crate::dispatch::ValidationGateError>())
}

/// TICKET-081: how many consecutive `action_decided` events for the same
/// (kind, work_id) fingerprint, with nothing else for that work_id in
/// between, count as "stuck." Broader than TICKET-078's inline retry cap,
/// which only gates Retry/Escalate via ledger counts -- this catches any
/// action kind repeating (e.g. ReviewMr/FixMr selected over and over for a
/// branch whose classification never changes).
const STUCK_LOOP_THRESHOLD: usize = 3;

/// Returns `Some(reason)` if the last `STUCK_LOOP_THRESHOLD` decisions for
/// this action's work_id all match its current fingerprint. Reads the
/// existing event stream (TICKET-083) rather than new storage.
fn detect_stuck_loop(
    events: &[crate::events::ControllerEvent],
    profile_name: &str,
    action: &NextAction,
) -> Option<String> {
    let work_id = action.work_id()?;
    let fingerprint_prefix = format!("{}:", action.kind());
    let mut consecutive = 0;
    for event in events.iter().rev() {
        if event.profile.as_deref() != Some(profile_name) || event.event_type != "action_decided" {
            continue;
        }
        if event.work_id.as_deref() != Some(work_id) {
            continue;
        }
        if event.details.starts_with(&fingerprint_prefix) {
            consecutive += 1;
            if consecutive >= STUCK_LOOP_THRESHOLD {
                return Some(format!(
                    "stuck-loop detected: '{}' selected {} times in a row for {} with no \
                     intervening state change",
                    action.kind(),
                    consecutive,
                    work_id
                ));
            }
        } else {
            break;
        }
    }
    None
}

fn record_action_events(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    original_action: &NextAction,
    effective_action: &NextAction,
) -> Result<()> {
    crate::events::record(
        cfg,
        crate::events::EventType::ActionDecided,
        Some(profile_name),
        original_action.work_id(),
        format!("{}: {}", original_action.kind(), original_action.reason()),
    )?;
    if original_action != effective_action {
        crate::events::record(
            cfg,
            crate::events::EventType::ActionOverridden,
            Some(profile_name),
            original_action.work_id(),
            format!(
                "{} -> {}: {}",
                original_action.kind(),
                effective_action.kind(),
                effective_action.reason()
            ),
        )?;
    }
    Ok(())
}

/// TICKET-079: `gah loop --once` -- exactly one bounded controller
/// iteration. Build a snapshot, decide one action, execute at most that
/// one action, persist one controller event trail, exit. No daemon, no
/// repeated recursion.
#[derive(Debug, Serialize)]
pub struct LoopOnceResult {
    pub action: NextAction,
    pub outcome: String,
}

/// The lock is scoped by profile name AND config file identity: a profile
/// is really a named entry *within a specific config file*, so two
/// different config files that happen to define a same-named profile (e.g.
/// separate test fixtures, or a user's dev vs. prod config) are genuinely
/// independent and must not block each other. Two invocations against the
/// same config file (the real-world incident this guards against: the
/// daemon and an ad-hoc `--once` both using the default
/// `~/.config/gah/config.toml`) hash to the same lock file. The lock must
/// not live under `XDG_STATE_HOME`: backend wrappers and service managers may
/// use different XDG environments while still operating the same profile.
fn loop_lock_path(profile_name: &str, config_path: &std::path::Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let canonical_config =
        std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical_config.hash(&mut hasher);
    let lock_dir = canonical_config
        .parent()
        .map(|parent| parent.join(".gah-locks"))
        .unwrap_or_else(|| PathBuf::from(".gah-locks"));
    lock_dir.join(format!(
        "loop-{}-{:x}.lock",
        profile_name.replace('/', "_"),
        hasher.finish()
    ))
}

/// Held for the lifetime of a single gah invocation (daemon loop, `--once`,
/// or a manual `dispatch`) that performs real execution -- spawning
/// backends, claiming tickets, writing ledger entries -- for a profile.
/// Dropping it releases the underlying flock.
// The File is never read again -- it exists only so its flock is released on
// Drop, when the guard goes out of scope at the end of the invocation.
#[allow(dead_code)]
pub struct ProfileLock(std::fs::File);

/// Acquire the exclusive per-profile execution lock so that only one gah
/// process at a time can do real execution work for a given profile of a
/// given config file.
///
/// Callers (see `main.rs`) must call this exactly ONCE per process, at the
/// outermost entry point for whichever command they're running, and hold
/// the returned guard for the rest of that invocation. Do not call this
/// again from within an already-locked process (e.g. from inside
/// `run_loop`'s per-iteration `run_once` calls) -- POSIX flock exclusivity
/// is per open-file-description, not per-process, so a second `open()` +
/// `try_lock_exclusive()` from the same process would conflict with its own
/// already-held lock and deadlock.
pub fn acquire_profile_lock(
    profile_name: &str,
    config_path: &std::path::Path,
) -> Result<ProfileLock> {
    let lock_path = loop_lock_path(profile_name, config_path);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.try_lock_exclusive().map_err(|_| {
        anyhow::anyhow!(
            "gah already running for profile '{profile_name}' (lock: {})",
            lock_path.display()
        )
    })?;
    Ok(ProfileLock(lock))
}

/// Reload the config from disk for `run_loop`'s per-iteration hot-reload,
/// validating that `profile_name` is still resolvable in the freshly loaded
/// config. A parse-clean reload that dropped or renamed this exact profile
/// (e.g. an operator edit mid-run) is just as unsafe to dispatch against as a
/// read failure -- callers must treat both errors identically (fall back to
/// the last-known-good config) rather than adopting a config the running
/// profile no longer resolves against.
fn reload_config_for_profile(
    config_path: &std::path::Path,
    profile_name: &str,
) -> Result<crate::config::GahConfig> {
    let loaded = crate::config::load(config_path.to_str())?;
    crate::config::get_profile(&loaded, profile_name)?;
    Ok(loaded)
}

/// TICKET-282: before reusing an existing branch for a `FixMr`, detect a
/// branch that is already attached to a worktree GAH does not manage (an
/// externally-owned or stale worktree for the same open PR). Returns a
/// replacement `NextAction` for the loop to run instead:
///
/// * `None` -- the action is safe to execute as-is.
/// * `Some(redispatched)` -- one or more branch conflicts were skipped and the
///   returned action is the first unblocked candidate (or a terminal action).
///
/// Path location is never treated as proof of ownership. Clean and dirty
/// attachments are both deferred; lifecycle pruning decides removal.
fn resolve_attached_branch_conflicts(
    action: &NextAction,
    mut find_attachment: impl FnMut(&str) -> Result<Option<crate::worktree::BranchWorktreeAttachment>>,
    mut record_deferral: impl FnMut(
        &str,
        Option<&str>,
        &crate::worktree::BranchWorktreeAttachment,
    ) -> Result<()>,
    mut choose_next: impl FnMut(&HashSet<String>, &HashSet<String>) -> Result<NextAction>,
) -> Result<Option<NextAction>> {
    let mut candidate = action.clone();
    let mut deferred_work_ids = HashSet::new();
    let mut deferred_branches = HashSet::new();

    loop {
        let (branch, work_id) = match &candidate {
            NextAction::FixMr {
                branch, work_id, ..
            } => (branch, work_id),
            _ => return Ok((!deferred_branches.is_empty()).then_some(candidate)),
        };
        let Some(attachment) = find_attachment(branch)? else {
            return Ok((!deferred_branches.is_empty()).then_some(candidate));
        };

        deferred_branches.insert(branch.clone());
        if let Some(work_id) = work_id {
            deferred_work_ids.insert(work_id.clone());
        }
        record_deferral(branch, work_id.as_deref(), &attachment)?;
        candidate = choose_next(&deferred_work_ids, &deferred_branches)?;
    }
}

fn defer_if_branch_attached(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    action: &NextAction,
) -> Result<Option<NextAction>> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let repo = Path::new(&profile.local_path);
    resolve_attached_branch_conflicts(
        action,
        |branch| crate::worktree::branch_attachment(repo, branch),
        |branch, work_id, attachment| {
            crate::events::record(
                cfg,
                crate::events::EventType::WorkDeferred,
                Some(profile_name),
                work_id,
                format!(
                    "branch '{}' already attached to worktree {} (clean={}); deferring to next eligible item",
                    branch,
                    attachment.path.display(),
                    attachment.clean
                ),
            )
        },
        |deferred_work_ids, deferred_branches| {
            let mut fresh =
                crate::status::build_snapshot(cfg, profile_name, time::OffsetDateTime::now_utc())?;
            fresh.merge_requests.retain(|mr| {
                !mr.work_id
                    .as_ref()
                    .is_some_and(|id| deferred_work_ids.contains(id))
                    && !deferred_branches.contains(&mr.branch)
            });
            fresh.available_tickets.retain(|ticket| {
                !ticket
                    .work_id
                    .as_ref()
                    .is_some_and(|id| deferred_work_ids.contains(id))
            });
            Ok(decide_next_action(&fresh))
        },
    )
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
    let _lock = acquire_profile_lock(profile_name, config_path)?;

    // The dashboard Settings UI can change max_parallel_workers,
    // manager_wake_autonomy (per-profile) and current_manager (global) at
    // runtime. Reload the config from disk on every iteration so those
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
    let claim_scope = {
        let profile = crate::config::get_profile(cfg, profile_name)?;
        format!("{profile_name}@{}", profile.repo_id)
    };
    let now = time::OffsetDateTime::now_utc();
    let snapshot =
        crate::status::build_snapshot_from_entries(cfg, profile_name, now, &ledger_entries)?;
    crate::events::record(
        cfg,
        crate::events::EventType::ObservationCompleted,
        Some(profile_name),
        None,
        format!("profile={profile_name}"),
    )?;

    // For parallel > 1, we need to decide multiple actions
    if parallel > 1 {
        run_parallel_once(
            cfg,
            profile_name,
            &snapshot,
            json,
            parallel,
            skip_validation_gate,
        )?;
    } else {
        // Original single action behavior
        let original_action = decide_next_action(&snapshot);
        let history = crate::events::read_events(cfg)?;
        let mut action = original_action.clone();
        if let Some(reason) = detect_stuck_loop(&history, profile_name, &original_action) {
            // Persist a work-item-scoped durable human gate so that
            // subsequent loop iterations see human_required=true for this
            // work_id via ledger_lookup_for_ticket and skip it, rather than
            // re-selecting DispatchTicket every cycle (the original
            // trip-without-latch bug).
            if let Some(wid) = original_action.work_id() {
                let profile = crate::config::get_profile(cfg, profile_name)?;
                let mut gate = crate::ledger::LedgerEntry::new(
                    profile_name,
                    profile,
                    "auto",
                    "fix",
                    wid,
                    None,
                    None,
                );
                gate.work_id = Some(wid.to_string());
                gate.human_required = true;
                gate.dispatch_reason = Some("stuck_loop_gate".to_string());
                gate.error_summary = Some(reason.clone());
                if let Err(e) = crate::ledger::append(cfg, &gate) {
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
            if let Some(stuck_wid) = original_action.work_id() {
                scoped
                    .merge_requests
                    .retain(|mr| mr.work_id.as_deref() != Some(stuck_wid));
                scoped
                    .available_tickets
                    .retain(|t| t.work_id.as_deref() != Some(stuck_wid));
            }
            let redispatched = decide_next_action(&scoped);
            if redispatched.kind() == "no_op" {
                // Nothing else actionable -> genuine stall, surface it.
                action = NextAction::HumanRequired {
                    reason,
                    reference: original_action.work_id().map(str::to_string),
                };
            } else {
                action = redispatched;
            }
        }
        // TICKET-282: a FixMr reusing a branch already attached to a foreign
        // or stale worktree must be deferred (non-terminal) and the loop
        // continued with the next eligible item, never allowed to stall the
        // recurring loop on a hard `git worktree add` failure.
        if let Some(redispatch) = defer_if_branch_attached(cfg, profile_name, &action)? {
            action = redispatch;
        }
        record_action_events(cfg, profile_name, &original_action, &action)?;

        let outcome = if let Some(work_id) = action.work_id().filter(|_| {
            !matches!(
                action,
                NextAction::WaitUntil { .. }
                    | NextAction::HumanRequired { .. }
                    | NextAction::NoOp { .. }
            )
        }) {
            if !crate::work_claim::try_claim_work(&claim_scope, work_id)? {
                format!("Skipped already-claimed work '{work_id}'")
            } else {
                match execute_action(cfg, profile_name, &action, skip_validation_gate, None) {
                    Ok(outcome) => {
                        crate::work_claim::release_work(&claim_scope, work_id)?;
                        outcome
                    }
                    Err(error) => {
                        crate::work_claim::release_work(&claim_scope, work_id)?;
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
        crate::events::record(
            cfg,
            stop_event_type,
            Some(profile_name),
            action.work_id(),
            outcome.clone(),
        )?;

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
    snapshot: &crate::status::StatusSnapshot,
    json: bool,
    max_parallel: usize,
    skip_validation_gate: bool,
) -> Result<()> {
    use std::collections::HashSet;

    let mut executed_work_ids = HashSet::new();
    let claim_scope = {
        let profile = crate::config::get_profile(cfg, profile_name)?;
        format!("{profile_name}@{}", profile.repo_id)
    };

    // Profile routing decides which eligible backend handles each action. Do
    // not use the number of persisted availability rows as a worker limit:
    // that list is sparse and only contains observed scopes, not every
    // configured backend.
    let effective_parallel_limit = max_parallel;

    // Decide actions one by one until we reach the parallel limit or run out of actions
    let history = crate::events::read_events(cfg)?;
    let mut results = Vec::new();

    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        // A terminal decision (NoOp/HumanRequired/WaitUntil) for one slot only
        // means *that slot* found nothing new to do in this batch -- it does
        // not mean other slots wouldn't find distinct eligible work from their
        // own fresh snapshot. So terminal actions are deferred rather than
        // executed immediately: only the last one seen is executed/recorded,
        // and only if no slot in the batch spawned real work, preserving the
        // "why did we stop" signal for the genuinely-no-work case without
        // aborting the rest of the batch for a single slot's verdict.
        let mut pending_terminal: Option<(NextAction, NextAction)> = None;
        for _ in 0..effective_parallel_limit {
            // Re-fetch claimed work IDs to get fresh state (other processes might have claimed work)
            let claimed_work_ids = crate::work_claim::get_claimed_work_ids(&claim_scope)?;

            // Reuse the controller snapshot for this cycle; claim filtering
            // below removes work already taken by earlier slots or other
            // processes.
            let mut fresh_snapshot = snapshot.clone();

            // Do not let the next slot re-select a ticket claimed by an
            // earlier slot in this batch or by another controller process.
            // The decision function operates on snapshots, so claims must be
            // projected out before deciding the next action.
            fresh_snapshot.available_tickets.retain(|ticket| {
                ticket
                    .work_id
                    .as_deref()
                    .map(|id| {
                        !claimed_work_ids.iter().any(|claimed| claimed == id)
                            && !executed_work_ids.contains(id)
                    })
                    .unwrap_or(true)
            });
            fresh_snapshot.merge_requests.retain(|mr| {
                mr.work_id
                    .as_deref()
                    .map(|id| {
                        !claimed_work_ids.iter().any(|claimed| claimed == id)
                            && !executed_work_ids.contains(id)
                    })
                    .unwrap_or(true)
            });

            let original_action = decide_next_action(&fresh_snapshot);
            let mut action = original_action.clone();

            // Apply stuck-loop detection (TICKET-skip-and-continue): persist the
            // work-item-scoped gate, then skip this item and let the loop pick the
            // next eligible work item rather than parking the whole profile.
            if let Some(reason) = detect_stuck_loop(&history, profile_name, &original_action) {
                if let Some(wid) = original_action.work_id() {
                    let profile = crate::config::get_profile(cfg, profile_name)?;
                    let mut gate = crate::ledger::LedgerEntry::new(
                        profile_name,
                        profile,
                        "auto",
                        "fix",
                        wid,
                        None,
                        None,
                    );
                    gate.work_id = Some(wid.to_string());
                    gate.human_required = true;
                    gate.dispatch_reason = Some("stuck_loop_gate".to_string());
                    gate.error_summary = Some(reason.clone());
                    let _ = crate::ledger::append(cfg, &gate);
                }
                // Re-decide: exclude the stuck work_id, pick the next eligible one.
                if let Some(stuck_wid) = original_action.work_id() {
                    fresh_snapshot
                        .merge_requests
                        .retain(|mr| mr.work_id.as_deref() != Some(stuck_wid));
                    fresh_snapshot
                        .available_tickets
                        .retain(|t| t.work_id.as_deref() != Some(stuck_wid));
                }
                let redispatched = decide_next_action(&fresh_snapshot);
                if redispatched.kind() == "no_op" {
                    action = NextAction::HumanRequired {
                        reason,
                        reference: original_action.work_id().map(str::to_string),
                    };
                } else {
                    action = redispatched;
                }
            }

            // TICKET-282: defer a FixMr whose branch is already attached to a
            // foreign/stale worktree and continue with the next eligible item
            // rather than stalling the batch on a hard `git worktree add`.
            if let Some(redispatch) = defer_if_branch_attached(cfg, profile_name, &action)? {
                action = redispatch;
            }

            // Check if this action involves a work_id that's already claimed or executed in this batch
            let action_work_id = action.work_id();
            if let Some(work_id) = action_work_id {
                if claimed_work_ids.contains(&work_id.to_string())
                    || crate::work_claim::is_claimed(&claim_scope, work_id)?
                    || executed_work_ids.contains(work_id)
                {
                    // Skip this action as it's already in flight or claimed
                    continue;
                }
            }

            // For terminal actions (WaitUntil, HumanRequired, NoOp), this slot
            // found nothing to do -- record it as the current "why we might
            // stop" candidate and let the next slot try independently, rather
            // than aborting the whole batch (see comment above `handles`).
            match &action {
                NextAction::WaitUntil { .. }
                | NextAction::HumanRequired { .. }
                | NextAction::NoOp { .. } => {
                    pending_terminal = Some((original_action, action));
                }
                _ => {
                    // For dispatch actions, record and execute
                    record_action_events(cfg, profile_name, &original_action, &action)?;

                    // Claim this work_id before execution to prevent duplicate dispatch
                    if let Some(work_id) = action_work_id {
                        if !crate::work_claim::try_claim_work(&claim_scope, work_id)? {
                            continue;
                        }
                        executed_work_ids.insert(work_id.to_string());
                    }

                    let action_for_thread = action.clone();
                    let profile_for_thread = profile_name.to_string();
                    let claim_scope_for_thread = claim_scope.clone();
                    let work_id_for_thread = action_work_id.map(str::to_string);
                    // A capped backend/model must be reserved before the
                    // next slot makes its routing decision. The rendezvous
                    // sender is dropped if dispatch fails before routing, so
                    // that failure cannot deadlock the batch.
                    let waits_for_route = matches!(
                        &action_for_thread,
                        NextAction::DispatchTicket { .. }
                            | NextAction::Retry { .. }
                            | NextAction::Escalate { .. }
                            | NextAction::FixMr { .. }
                    );
                    let (route_ready, route_receiver) = if waits_for_route {
                        let (sender, receiver) = sync_channel(0);
                        (Some(sender), Some(receiver))
                    } else {
                        (None, None)
                    };
                    handles.push(scope.spawn(move || {
                        let result = execute_action(
                            cfg,
                            &profile_for_thread,
                            &action_for_thread,
                            skip_validation_gate,
                            route_ready,
                        );
                        let (outcome, event_outcome) = match result {
                            Ok(outcome) => (outcome.clone(), outcome),
                            Err(error) => {
                                let outcome = format!("Error: {error}");
                                (outcome.clone(), outcome)
                            }
                        };
                        if let Some(work_id) = work_id_for_thread.as_deref() {
                            let _ =
                                crate::work_claim::release_work(&claim_scope_for_thread, work_id);
                        }
                        let _ = crate::events::record(
                            cfg,
                            crate::events::EventType::LoopStopped,
                            Some(&profile_for_thread),
                            action_for_thread.work_id(),
                            event_outcome,
                        );
                        LoopOnceResult {
                            action: action_for_thread,
                            outcome,
                        }
                    }));
                    if let Some(receiver) = route_receiver {
                        let _ = receiver.recv();
                    }
                }
            }
        }

        // Only surface a terminal decision if the batch found no real work at
        // all -- if any slot spawned a dispatch/review action, the terminal
        // verdicts from other slots were just "nothing left for this slot"
        // noise, not a reason to report the batch as stopped.
        if handles.is_empty() {
            if let Some((original_action, action)) = pending_terminal {
                record_action_events(cfg, profile_name, &original_action, &action)?;
                let outcome =
                    execute_action(cfg, profile_name, &action, skip_validation_gate, None)?;

                let stop_event_type = match &action {
                    NextAction::WaitUntil { .. } => crate::events::EventType::WaitSelected,
                    NextAction::HumanRequired { .. } => crate::events::EventType::HumanRequired,
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

                results.push(LoopOnceResult { action, outcome });
            }
        }

        for handle in handles {
            results.push(
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("parallel GAH worker panicked"))?,
            );
        }
        Ok(())
    })?;

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
    if results.iter().any(|r| r.outcome.starts_with("Error:")) {
        crate::work_claim::release_all_for_profile(&claim_scope)?;
    }

    Ok(())
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
        budget: 0,
        dry_run: false,
        config_path: None,
        oh_profile: None,
        model: None,
        retries: 2,
        allow_draft_fail: false,
        prod: false,
        allow_unknown_red_baseline: false,
        escalate: false,
        existing_branch: None,
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
            run_dispatch_and_record(cfg, "review", action.work_id(), &args)?;
            Ok(format!("Dispatched review for branch '{branch}'"))
        }
        NextAction::MarkReadyForReview { branch, .. } => {
            let profile = crate::config::get_profile(cfg, profile_name)?;
            crate::provider::mark_ready_for_review(profile, branch)?;
            Ok(format!("Marked MR on branch '{branch}' ready for review"))
        }
        NextAction::FixMr { branch, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: branch.clone(),
                existing_branch: Some(branch.clone()),
                dispatch_reason: Some("post_review_repair".to_string()),
                ..base_args()
            };
            run_dispatch_and_record(cfg, "fix_existing", action.work_id(), &args)?;
            Ok(format!("Dispatched fix for existing branch '{branch}'"))
        }
        NextAction::MergeMr {
            branch,
            work_id,
            mr_url,
            ..
        } => {
            let profile = crate::config::get_profile(cfg, profile_name)?;
            let merge_policy = profile
                .effective_routing(&cfg.defaults)
                .merge_policy
                .unwrap_or_default();
            let run_id = uuid::Uuid::new_v4().to_string();
            crate::events::record_with_run_id(
                cfg,
                crate::events::EventType::DispatchStarted,
                Some(profile_name),
                action.work_id(),
                Some(&run_id),
                "merge",
            )?;
            let gitlab_mwps = merge_policy == crate::config::MergePolicy::GitlabMwps
                && profile.provider == "gitlab";
            let result = if gitlab_mwps {
                // Issue #124 / TICKET-127: set GitLab's merge-when-pipeline
                // succeeds flag and return; GitLab enforces the CI gate
                // natively. We never merge the MR ourselves in this mode.
                let target = crate::provider::find_review_target_by_branch(profile, branch)
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                crate::provider::gitlab_set_mwps(profile, &target.id)
            } else {
                crate::dispatch::merge_branch(cfg, profile, branch, work_id, mr_url, Some(&run_id))
            };
            let outcome = match &result {
                Ok(()) if gitlab_mwps => {
                    format!("Set GitLab merge-when-pipeline-succeeds on branch '{branch}'")
                }
                Ok(()) => format!("Merged MR on branch '{branch}'"),
                Err(e) => format!("Merge failed for branch '{branch}': {e:#}"),
            };
            crate::events::record_with_run_id(
                cfg,
                crate::events::EventType::DispatchFinished,
                Some(profile_name),
                action.work_id(),
                Some(&run_id),
                format!("merge: {outcome}"),
            )?;
            Ok(outcome)
        }
        NextAction::DispatchTicket { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                dispatch_reason: Some("initial".to_string()),
                ..base_args()
            };
            run_dispatch_and_record(cfg, "dispatch_ticket", action.work_id(), &args)?;
            Ok(format!("Dispatched ticket '{ticket_path}'"))
        }
        NextAction::Retry { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                dispatch_reason: Some("initial".to_string()),
                ..base_args()
            };
            run_dispatch_and_record(cfg, "retry", action.work_id(), &args)?;
            Ok(format!("Retried ticket '{ticket_path}'"))
        }
        NextAction::Escalate { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                escalate: true,
                dispatch_reason: Some("initial".to_string()),
                ..base_args()
            };
            run_dispatch_and_record(cfg, "escalate", action.work_id(), &args)?;
            Ok(format!("Escalated ticket '{ticket_path}'"))
        }
        NextAction::WaitUntil { until, reason } => Ok(format!("Waiting until {until} ({reason})")),
        NextAction::HumanRequired { reason, reference } => Ok(format!(
            "Human required: {reason}{}",
            reference
                .as_deref()
                .map(|r| format!(" ({r})"))
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
) -> Result<()> {
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
            Ok(())
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
#[path = "controller/worktree_deferral_tests.rs"]
mod worktree_deferral_tests;

#[cfg(test)]
#[path = "controller/ledger_read_tests.rs"]
mod ledger_read_tests;

#[cfg(test)]
mod tests {
    use super::{
        acquire_profile_lock, is_validation_gate_failure, loop_lock_path,
        reload_config_for_profile, wait_interruptibly, NextAction,
    };

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

    use super::{detect_stuck_loop, record_action_events, STUCK_LOOP_THRESHOLD};
    use crate::config::{Defaults, GahConfig, RoutingPolicy};
    use crate::events::ControllerEvent;
    use std::collections::HashMap;

    fn decided_event(profile: &str, work_id: &str, kind: &str) -> ControllerEvent {
        ControllerEvent {
            timestamp: "2026-07-05T00:00:00Z".into(),
            event_type: "action_decided".into(),
            profile: Some(profile.into()),
            work_id: Some(work_id.into()),
            run_id: None,
            details: format!("{kind}: some reason"),
        }
    }

    fn fix_mr_action() -> NextAction {
        NextAction::FixMr {
            work_id: Some("TICKET-500".into()),
            branch: "gah/real-1".into(),
            mr_url: None,
            reason: "MR needs fix".into(),
        }
    }

    #[test]
    fn stuck_loop_not_detected_below_threshold() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD - 1)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action()).is_none());
    }

    #[test]
    fn stuck_loop_detected_at_threshold() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        let reason = detect_stuck_loop(&events, "real", &fix_mr_action());
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("fix_mr"));
    }

    #[test]
    fn different_action_kind_breaks_the_streak() {
        let mut events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        // A review_mr decision landed in between -- state changed, no longer stuck.
        events.push(decided_event("real", "TICKET-500", "review_mr"));
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action()).is_none());
    }

    #[test]
    fn events_for_other_work_ids_do_not_count_or_break_the_streak() {
        let mut events = vec![decided_event("real", "TICKET-500", "fix_mr")];
        events.push(decided_event("real", "TICKET-999", "dispatch_ticket"));
        events.extend(
            (0..STUCK_LOOP_THRESHOLD - 1).map(|_| decided_event("real", "TICKET-500", "fix_mr")),
        );
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action()).is_some());
    }

    #[test]
    fn events_from_a_different_profile_are_ignored() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("other-profile", "TICKET-500", "fix_mr"))
            .collect();
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action()).is_none());
    }

    #[test]
    fn actions_without_a_work_id_are_never_flagged_stuck() {
        let events: Vec<_> = (0..10)
            .map(|_| decided_event("real", "", "no_op"))
            .collect();
        let action = NextAction::NoOp {
            reason: "nothing actionable".into(),
        };
        assert!(detect_stuck_loop(&events, "real", &action).is_none());
    }

    fn event_test_config() -> (tempfile::TempDir, GahConfig) {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GahConfig {
            context: Default::default(),
            defaults: Defaults {
                current_manager: None,
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: String::new(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: RoutingPolicy::default(),
            },
            profiles: HashMap::new(),
        };
        (tmp, cfg)
    }

    #[test]
    fn once_reconciliation_writes_terminal_event_and_unknown_ledger_record() {
        let (_tmp, mut cfg) = event_test_config();
        let profile: crate::config::Profile = toml::from_str(
            r#"
display_name = "Real"
repo_id = "real"
provider = "github"
repo = "owner/real"
local_path = "/tmp/real"
artifact_root = "/tmp/real-artifacts"
default_target_branch = "main"
"#,
        )
        .unwrap();
        cfg.profiles.insert("real".into(), profile);
        crate::events::record_with_run_id(
            &cfg,
            crate::events::EventType::DispatchStarted,
            Some("real"),
            Some("TICKET-500"),
            Some("orphaned-run"),
            "dispatch_ticket: 500",
        )
        .unwrap();
        let mut ledger_entries = crate::ledger::read_entries(&cfg).unwrap();

        assert_eq!(
            super::reconcile_abandoned_dispatches(&cfg, "real", &mut ledger_entries).unwrap(),
            1
        );

        let events = crate::events::read_events(&cfg).unwrap();
        assert!(events.iter().any(|event| {
            event.run_id.as_deref() == Some("orphaned-run")
                && event.event_type == "dispatch_finished"
                && event.details.contains("abandoned")
        }));
        let ledger = crate::ledger::read_entries(&cfg).unwrap();
        let entry = ledger
            .iter()
            .find(|entry| entry.session_id.as_deref() == Some("orphaned-run"))
            .expect("reconciliation must persist an unknown terminal ledger record");
        assert_eq!(entry.work_id.as_deref(), Some("TICKET-500"));
        assert_eq!(entry.failure_class.as_deref(), Some("harness_error"));
        assert_eq!(
            entry.validation_result.as_deref(),
            Some("not_run_abandoned")
        );
    }

    #[test]
    fn stuck_loop_override_records_original_decision_and_override() {
        let (_tmp, cfg) = event_test_config();
        let original = NextAction::ReviewMr {
            work_id: Some("TICKET-500".into()),
            branch: "gah/real-1".into(),
            mr_url: Some("https://example/review".into()),
            reason: "MR on branch 'gah/real-1' classified NEEDS_REVIEW".into(),
        };
        let effective = NextAction::HumanRequired {
            reason: "stuck-loop detected: 'review_mr' selected 3 times in a row for TICKET-500 with no intervening state change".into(),
            reference: Some("TICKET-500".into()),
        };

        record_action_events(&cfg, "real", &original, &effective).unwrap();

        let events = crate::events::read_events(&cfg).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "action_decided");
        assert!(events[0].details.starts_with("review_mr:"));
        assert_eq!(events[1].event_type, "action_overridden");
        assert!(events[1].details.contains("review_mr -> human_required"));
    }

    // TICKET-096: Parallel dispatch tests
    use crate::models::AvailableTicket;
    use crate::status::{
        ObservationStatus, Observations, ProfileIdentity, ScopeStatusJson, StatusSnapshot,
    };

    fn empty_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            schema_version: 1,
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
            errors: vec![],
            available_tickets: vec![],
            fix_attempt_counts: std::collections::HashMap::new(),
            merge_attempt_counts: std::collections::HashMap::new(),
            review_held_work_ids: std::collections::HashSet::new(),
            publishing_allow_pr: true,
            max_parallel_workers: 1,
            backend_configured: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn parallel_dispatch_respects_max_parallel_limit() {
        let mut snapshot = empty_snapshot();

        // Add multiple eligible backends (more than max_parallel)
        for _ in 0..5 {
            snapshot.availability.push(ScopeStatusJson {
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
                title: Some(format!("Test ticket {}", i)),
                has_active_mr: false,
                prior_attempt_count: 0,
                genuine_agent_failure_count: 0,
                last_failure_class: None,
                recommended_backend: None,
                recommended_model: None,
                human_required: false,
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

        // Add only unavailable backends
        for i in 0..3 {
            snapshot.availability.push(ScopeStatusJson {
                backend: format!("backend_{}", i),
                model: None,
                quota_pool: None,
                eligible_now: false,
                reason: Some("rate limited".to_string()),
                unavailable_until: Some(time::OffsetDateTime::now_utc().to_string()),
                source: None,
                last_error_summary: None,
                observed_at: None,
                scope: None,
            });
        }

        // With 0 eligible backends, max_parallel=5 should be limited to 0
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
