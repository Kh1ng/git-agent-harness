use crate::config::{self, GahConfig};
use crate::ledger::{self, LedgerEntry};
use crate::notifications::{notify_event, NotifyEvent};
use crate::usage_attribution::{aggregate_attempt_usage, usage_has_observation};
use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;
use std::time::Instant;

mod attempts;
mod claims;
mod command;
mod dependencies;
mod dry_run;
mod environment;
mod error;
mod identity;
mod issues;
mod metrics;
mod mutation_policy;
mod prompts;
mod publish;
mod repair_context;
mod repo_inspection;
mod review;
#[cfg(test)]
mod test_util;
mod text;
mod validation;
mod workflows;

pub use self::review::policy::review_budget_exhausted_error;
pub(crate) use self::text::utf8_safe_prefix;

pub use self::attempts::review_preflight;
pub(crate) use self::attempts::routing_runtime_state_from_entries;

use self::claims::check_duplicate_work;
pub(crate) use self::claims::duplicate_work_error;
pub(crate) use self::claims::scan_available_tickets_with_dependencies;
#[allow(unused_imports)]
pub use self::claims::{merge_branch, scan_available_tickets};
pub use self::validation::{self_check_validation_gate, ValidationGateError};

/// A parallel sibling reached routing after another worker reserved the only
/// available backend/model slot. This is typed capacity contention, not a
/// failed backend execution; the controller should close the run as deferred
/// and retry it on a later iteration without alarming the operator.
pub fn capacity_deferred_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<crate::routing::RouteError>()
            .is_some_and(crate::routing::RouteError::is_capacity_deferral)
    })
}

fn should_notify_dispatch_failure(error: &anyhow::Error) -> bool {
    review_budget_exhausted_error(error).is_none() && !capacity_deferred_error(error)
}

fn is_policy_approval_gate(entry: &LedgerEntry) -> bool {
    entry.human_required
        && entry.human_required_reason_code.as_deref()
            == Some(crate::controller::HumanRequiredReason::PolicyApproval.as_str())
        && entry.failure_class.as_deref()
            == Some(crate::ledger::FailureClass::HumanBlocked.as_str())
}

fn ensure_terminal_failure_attribution(
    failure_class: &mut Option<String>,
    failure_stage: &mut Option<String>,
) {
    failure_class.get_or_insert_with(|| {
        crate::ledger::FailureClass::HarnessError
            .as_str()
            .to_string()
    });
    failure_stage.get_or_insert_with(|| crate::ledger::FailureStage::Dispatch.as_str().to_string());
}

pub(super) const MIN_DISPATCH_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

pub struct DispatchArgs {
    pub profile: String,
    pub mode: String,
    pub backend: String,
    pub target: String,
    pub branch: Option<String>,
    pub mr: Option<String>,
    pub current_branch: bool,
    /// Reserved for future per-run cost/turn budget enforcement; not yet read.
    #[allow(dead_code)]
    pub budget: u32,
    pub dry_run: bool,
    /// Already consumed by the caller to load `cfg`; kept on the struct for CLI plumbing symmetry.
    #[allow(dead_code)]
    pub config_path: Option<String>,
    pub oh_profile: Option<String>,
    pub model: Option<String>,
    pub retries: u32,
    pub allow_draft_fail: bool,
    pub prod: bool,
    pub issue_intake_override: bool,
    /// TICKET-111: proceed despite a baseline validation failure that the
    /// classifier could not attribute to harness/environment/expected-red
    /// (`BaselineDisposition::UnknownRed`). Named for exactly what it
    /// overrides, not a generic bypass.
    pub allow_unknown_red_baseline: bool,
    /// TICKET-079/089: seeds the *initial* route decision as if the prior
    /// attempt were a genuine agent-capability failure, activating the same
    /// cost-aware escalation-to-a-stronger-model logic TICKET-089 already
    /// applies mid-retry-loop -- reused here so `NextAction::Escalate`
    /// doesn't need a second escalation mechanism.
    pub escalate: bool,
    /// TICKET-118: for FixMr action, reuse an existing branch instead of creating a new one.
    #[allow(dead_code)]
    pub existing_branch: Option<String>,
    /// TICKET-073: deliberately bypass the fresh-worktree self-verification of
    /// a profile's `validation_commands`. Intended only for recovering from a
    /// known-broken config after the operator has acknowledged the failure.
    pub skip_validation_gate: bool,
    /// Distinguishes dispatch purpose for ledger persistence: `initial`,
    /// `post_review_repair`, `review`, or `stuck_loop_gate`.  The retry cap
    /// counts only `post_review_repair` entries.
    #[allow(dead_code)]
    pub dispatch_reason: Option<String>,
    /// Controller-provided work identity, especially important for reviews
    /// that do not resolve a ticket file during dispatch.
    pub work_id: Option<String>,
    /// Controller-assigned identity shared by start/finish events and the
    /// resulting ledger entry. Direct CLI dispatches generate one in `run`.
    pub run_id: Option<String>,
    /// Parallel-controller rendezvous: sent only after the selected coding
    /// route has reserved its backend/model slot. This prevents a sibling
    /// from choosing the same capped route before the first worker starts.
    pub route_ready: Option<SyncSender<()>>,
}

pub fn run(cfg: &GahConfig, args: &DispatchArgs) -> Result<()> {
    let profile = config::get_profile(cfg, &args.profile)?;
    environment::export_profile_env(profile, args.prod);

    println!("Profile: {}", profile.display_name);
    println!("Repo:    {}", profile.repo);
    println!("Branch:  {}", profile.default_target_branch);
    println!("Mode:    {}", args.mode);
    println!("Backend: {}", args.backend);
    println!();

    if args.dry_run {
        return dry_run::dry_run(cfg, profile, args);
    }

    // TICKET-073: verify the dispatch gate itself (validation_commands) against
    // a fresh worktree before spending any backend budget. Skips entirely when
    // the commands are unchanged since the last successful self-check (fast
    // path, hash compare only); otherwise spins up one fresh worktree and runs
    // the commands once. A failed self-check bails with a distinct error and is
    // NOT conflated with the dispatched ticket's own outcome.
    self_check_validation_gate(profile, cfg, args.skip_validation_gate)?;

    if args.mode == "improve" || args.mode == "fix" || args.mode == "experiment" {
        if let Some(work_id) = check_duplicate_work(cfg, profile, args)? {
            // Parallel workers: claim this work_id immediately, before any
            // backend work runs, so a concurrent `gah loop`/`gah dispatch`
            // process sees it right away rather than only after this
            // attempt finishes (minutes to hours later).
            let claim = LedgerEntry::new_claim(&args.profile, profile, &work_id);
            if let Err(e) = ledger::append(cfg, &claim) {
                eprintln!("warning: failed to append claim ledger entry: {e:#}");
            }
        }
    }

    let ts = args
        .run_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let session_dir = PathBuf::from(&profile.artifact_root)
        .join("sessions")
        .join(&ts);
    let mut ledger = LedgerEntry::new(
        &args.profile,
        profile,
        &args.backend,
        &args.mode,
        &args.target,
        Some(ts.clone()),
        Some(&session_dir),
    );
    ledger.work_id = args.work_id.clone();
    ledger.dispatch_reason = args.dispatch_reason.clone();
    let started = Instant::now();
    fs::create_dir_all(&session_dir)?;
    println!("Session: {}", session_dir.display());

    let result = match args.mode.as_str() {
        "improve" | "fix" => workflows::run_improve(cfg, profile, args, &session_dir, &mut ledger),
        "pm" => workflows::run_pm(cfg, profile, args, &session_dir, &mut ledger),
        "review" => workflows::run_review(cfg, profile, args, &session_dir, &mut ledger),
        "experiment" => workflows::run_experiment(cfg, profile, args, &session_dir, &mut ledger),
        other => anyhow::bail!("unknown mode: {}", other),
    };
    ledger.duration_seconds = Some(started.elapsed().as_secs_f64());
    if !usage_has_observation(&ledger.usage) {
        ledger.usage = aggregate_attempt_usage(&ledger.attempts);
    }
    if let Err(err) = &result {
        // Every terminal dispatch error must be attributable. More precise
        // workflow boundaries set their own class/stage; this fallback marks
        // uncaught orchestration/plumbing errors as harness-owned rather than
        // sending an operationally useless class=unknown/stage=unknown alert.
        ensure_terminal_failure_attribution(&mut ledger.failure_class, &mut ledger.failure_stage);
        ledger.error_summary = Some(error::summarize_error(err));
    }
    let policy_approval_gate = is_policy_approval_gate(&ledger);
    let mut new_policy_approval_transition = true;
    let append_result = if policy_approval_gate {
        crate::ledger::append_human_gate_if_transition(cfg, &ledger).map(|appended| {
            new_policy_approval_transition = appended;
        })
    } else {
        crate::ledger::append(cfg, &ledger).map(|_| ())
    };
    if let Err(err) = append_result {
        eprintln!("warning: failed to append ledger entry: {:#}", err);
    }
    if result
        .as_ref()
        .err()
        .is_some_and(should_notify_dispatch_failure)
        && (!policy_approval_gate || new_policy_approval_transition)
    {
        notify_event(
            cfg,
            profile,
            NotifyEvent::DispatchFailed {
                failure_class: ledger.failure_class.as_deref().unwrap_or("unknown"),
                failure_stage: ledger.failure_stage.as_deref(),
                // Live-observed: a review dispatch that fails before
                // resolving its target has no work_id (review targets a
                // branch/MR, not a ticket) -- fall back to the branch so
                // the notification says something more useful than
                // "work_id=unknown" for a failure a human can't trace back
                // to anything.
                work_id: ledger
                    .work_id
                    .as_deref()
                    .or(ledger.branch.as_deref())
                    .unwrap_or("unknown"),
                attempt_count: ledger.attempts_started,
                error_summary: ledger.error_summary.as_deref(),
                mr_url: ledger.mr_url.as_deref().or(ledger.branch.as_deref()),
            },
        );
    }
    result
}

#[cfg(test)]
mod tests;
