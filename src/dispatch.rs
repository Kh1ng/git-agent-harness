use crate::config::{self, CandidateConfig, GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::notifications::{notify_event, NotifyEvent};
use crate::routing::{self, RouteDecision, RouteRequest, TaskRoutingContext};
use crate::usage_attribution::{
    aggregate_attempt_usage, normalize_attempt_usage, usage_has_observation, UsageAttribution,
};
use crate::{provider, runner, worktree};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::SyncSender;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

mod attempts;
mod claims;
mod issues;
mod prompts;
mod publish;
mod text;
mod validation;
mod workflows;

#[cfg(test)]
mod test_util;

use self::issues::{
    issue_is_auto_dispatch_blocked, list_open_issues, parse_ticket_metadata,
    parse_ticket_metadata_from_issue, ticket_number_prefix, TicketMetadata,
};
use self::text::{extract_first_json_object, normalize_match};

pub use self::attempts::review_preflight;
use self::attempts::{
    apply_route_to_ledger, decide_route, ensure_bin, mark_backend_unavailable_from_output,
    mark_shutdown_cancelled, review_usage, route_identity, routing_runtime_state,
};

use self::publish::{render_review_comment, review_labels};

use self::claims::check_duplicate_work;
pub(crate) use self::claims::duplicate_work_error;
pub use self::claims::{merge_branch, scan_available_tickets};
pub use self::validation::{self_check_validation_gate, ValidationGateError};

pub(super) const MIN_DISPATCH_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// UTF-8 safe suffix: returns the last up to `max_bytes` of `s`,
/// adjusting the start index forward to a valid character boundary.
/// Result length is guaranteed <= max_bytes.
/// Never panics on valid UTF-8 input.
fn utf8_safe_suffix(s: &str, max_bytes: usize) -> &str {
    if s.is_empty() || max_bytes == 0 {
        return "";
    }
    let byte_start = s.len().saturating_sub(max_bytes);
    // Ensure we start at a valid character boundary
    // If byte_start is not a boundary, find the next boundary after it
    // This guarantees result.len() <= max_bytes
    let safe_start = if !s.is_char_boundary(byte_start) {
        s.char_indices()
            .find(|(i, _)| *i >= byte_start)
            .map(|(i, _)| i)
            .unwrap_or(s.len())
    } else {
        byte_start
    };
    &s[safe_start..]
}

/// UTF-8 safe prefix: returns the first up to `max_bytes` of `s`,
/// adjusting the end index backward to a valid character boundary.
/// Result length is guaranteed <= max_bytes.
/// Never panics on valid UTF-8 input.
pub(crate) fn utf8_safe_prefix(s: &str, max_bytes: usize) -> &str {
    if s.is_empty() || max_bytes == 0 {
        return "";
    }
    let byte_end = s.len().min(max_bytes);
    // Ensure we end at a valid character boundary
    // If byte_end is not a boundary, find the previous boundary before it
    // This guarantees result.len() <= max_bytes
    let safe_end = if !s.is_char_boundary(byte_end) {
        s.char_indices()
            .take_while(|(i, _)| *i < byte_end)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0)
    } else {
        byte_end
    };
    &s[..safe_end]
}

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
    /// Require explicit --prod flag to load production env_file_prod.
    /// Without this flag, only env_file (dev) is loaded.
    pub prod: bool,
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

/// Typed, terminal refusal used when a ticket has exhausted its configured
/// review budget. Keeping this distinct from backend failures lets the
/// controller close the run cleanly and makes the operator-visible event
/// stream explain that no reviewer was launched and no extra quota was spent.
#[derive(Debug)]
pub struct ReviewBudgetExhausted {
    reason: String,
}

impl ReviewBudgetExhausted {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ReviewBudgetExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for ReviewBudgetExhausted {}

pub fn review_budget_exhausted_error(err: &anyhow::Error) -> Option<&ReviewBudgetExhausted> {
    err.downcast_ref::<ReviewBudgetExhausted>()
}

pub fn run(cfg: &GahConfig, args: &DispatchArgs) -> Result<()> {
    let profile = config::get_profile(cfg, &args.profile)?;
    export_profile_env(profile, args.prod);

    println!("Profile: {}", profile.display_name);
    println!("Repo:    {}", profile.repo);
    println!("Branch:  {}", profile.default_target_branch);
    println!("Mode:    {}", args.mode);
    println!("Backend: {}", args.backend);
    println!();

    if args.dry_run {
        return dry_run(cfg, profile, args);
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
        "review" => review(cfg, profile, args, &session_dir, &mut ledger),
        "experiment" => workflows::run_experiment(cfg, profile, args, &session_dir, &mut ledger),
        other => anyhow::bail!("unknown mode: {}", other),
    };
    ledger.duration_seconds = Some(started.elapsed().as_secs_f64());
    if !usage_has_observation(&ledger.usage) {
        ledger.usage = aggregate_attempt_usage(&ledger.attempts);
    }
    if let Err(err) = &result {
        ledger.error_summary = Some(summarize_error(err));
    }
    if let Err(err) = crate::ledger::append(cfg, &ledger) {
        eprintln!("warning: failed to append ledger entry: {:#}", err);
    }
    if result.is_err()
        && result
            .as_ref()
            .err()
            .and_then(review_budget_exhausted_error)
            .is_none()
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

/// Exports `profile.env_file` (or `env_file_prod` with `--prod`) into the
/// real process environment, as early as possible.
///
/// `profile.pat()` and other provider.rs calls (GitLab/GitHub API lookups
/// made by the harness itself -- MR creation, review-target resolution,
/// posting comments) read GITLAB_PAT/GITHUB_TOKEN etc. via `std::env::var`
/// directly, and those calls can happen before any backend is spawned.
/// Loading the env file into a `Vec<(String, String)>` for a spawned
/// child's environment (done later, per mode, for the backend process
/// itself) never reaches these in-process calls -- confirmed live: a
/// review dispatch failed 3 layers downstream with a git refspec error
/// because GITLAB_PAT was never actually in this process's environment.
fn export_profile_env(profile: &Profile, prod: bool) {
    let resolved_env = if prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    if resolved_env.is_empty() {
        return;
    }
    for (key, value) in runner::load_env_file(resolved_env) {
        std::env::set_var(key, value);
    }
}

pub(super) fn command_output(bin: &str, args: &[&str], cwd: &Path) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("{} {}", bin, args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "{} {}: {}",
            bin,
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Check profile policy before provisioning any worktree.
/// If a policy_path is set, the requested action must be allowed or dispatch
/// hard-fails before any mutations occur.
fn enforce_policy(profile: &Profile, action: &str) -> Result<()> {
    let Some(policy_path) = &profile.policy_path else {
        return Ok(()); // no policy file = trust the user
    };
    let text = std::fs::read_to_string(policy_path)
        .with_context(|| format!("reading policy file: {}", policy_path))?;
    let cfg: crate::models::PolicyConfig =
        toml::from_str(&text).with_context(|| format!("parsing policy file: {}", policy_path))?;
    let repo = cfg.repo;
    let allowed = match repo.trust_mode.as_str() {
        "read_only" => false,
        "draft_pr_allowed" => match action {
            "open-draft-pr" => {
                repo.allow_provider_mutation && repo.allow_push && repo.allow_draft_pr
            }
            "edit-issue" => repo.allow_issue_write,
            "git-push" => repo.allow_push,
            "git-push-prod" => repo.allow_project_write,
            _ => false,
        },
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        anyhow::bail!(
            "POLICY BLOCKED: trust_mode={:?} does not allow action={:?}.              Set allow_push/allow_draft_pr/allow_project_write in {} or              pass --override-policy if you know what you're doing.",
            repo.trust_mode, action, policy_path
        )
    }
}

fn review(
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
    let diff_bundle = prepare_review_diff(repo, profile, &target)?;
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
        ledger.set_failure(
            crate::ledger::FailureClass::HumanBlocked,
            crate::ledger::FailureStage::Review,
        );
        ledger.validation_result = Some("review_budget_exhausted".into());
        ledger.human_required = true;
        ledger.error_summary = Some(block.reason.clone());
        apply_route_to_ledger(ledger, &route);
        notify_event(
            cfg,
            profile,
            NotifyEvent::HumanRequired {
                reason: "review budget exhausted",
                reference: target.mr_url.as_deref(),
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
        let attempt = runner::run_review_backend(
            profile,
            &route.effective_backend,
            repo,
            &prompt,
            &attempt_session,
            route.effective_model.as_deref(),
            &env_vars,
        );
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
                            rerouted.effective_backend, route.effective_backend, parsed.kind
                        );
                        route = rerouted;
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
            ledger.confidence_impact = Some(verdict.confidence.clone());
            ledger.review_verdict = Some(verdict.verdict.clone());
            ledger.review_confidence = Some(verdict.confidence.clone());
            ledger.reviewer_backend = Some(route.effective_backend.clone());
            ledger.reviewer_model = route.effective_model.clone();
            ledger.reviewer_tier = Some(reviewer_tier.as_str().to_string());
            ledger.review_gate_reason = verdict.safety_gate_reason.clone();
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
                notify_event(
                    cfg,
                    profile,
                    NotifyEvent::HumanRequired {
                        reason: "review verdict requires human attention",
                        reference: mr_url.as_deref(),
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
            mark_shutdown_cancelled(ledger, crate::ledger::FailureStage::Review, Some(-signal));
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

fn dry_run(cfg: &GahConfig, profile: &Profile, args: &DispatchArgs) -> Result<()> {
    println!("DRY RUN — no mutations will be performed\n");
    println!("## What would happen\n");
    let ts = timestamp();
    let branch = format!("gah/{}-{}", profile.repo_id, ts);
    let session_dir = PathBuf::from(&profile.artifact_root)
        .join("sessions")
        .join(&ts);
    println!("Session dir:  {}", session_dir.display());
    println!("New branch:   {}", branch);
    println!("From:         origin/{}", profile.default_target_branch);
    println!(
        "Worktree:     {}/{}",
        cfg.defaults.worktree_base,
        branch.replace('/', "-")
    );
    match args.mode.as_str() {
        "improve" | "fix" => {
            let route = dry_run_route(cfg, profile, &args.mode, args);
            if let Some(name) = args.oh_profile.as_deref() {
                println!(
                    "OH profile:   {} (~/.openhands/profiles/{}.json)",
                    name, name
                );
                if let Some(m) = &args.model {
                    println!("Model override: {}", m);
                }
            } else if route.is_none() {
                let cloud = args.backend == "cloud-coder";
                let default_model = cfg.defaults.llm_model(cloud);
                let model_name = args.model.as_deref().unwrap_or(&default_model);
                println!("LLM model:    {}", model_name);
                println!("LLM base:     {}", cfg.defaults.llm_base_url());
            }
            println!("Backend:      {}", args.backend);
            if let Some(route) = &route {
                println!(
                    "Effective:    {}/{}",
                    route.effective_backend,
                    route.effective_model.as_deref().unwrap_or("default")
                );
                println!("Routing:      {}", route.routing_reason);
                if let Some(summary) = route
                    .routing_diagnostics
                    .as_ref()
                    .and_then(|diagnostics| diagnostics.human_summary.as_deref())
                {
                    println!("Route detail: {}", summary);
                }
            }
            println!("Retries:      {}", args.retries);
            println!("Allow draft fail: {}", args.allow_draft_fail);
            println!("Prod env:         {}", args.prod);
            if !profile.validation_commands.is_empty() {
                println!("Validation:");
                for cmd in &profile.validation_commands {
                    println!("  $ {}", cmd);
                }
            }
            if !args.target.is_empty() {
                let task_type = if Path::new(&args.target)
                    .extension()
                    .is_some_and(|e| e == "json")
                {
                    "candidate JSON"
                } else {
                    "task string"
                };
                println!("Task source:  {} ({})", args.target, task_type);
            }
            println!(
                "\nSteps: fetch → worktree → {} → [validate → retry]* → commit → push → draft MR",
                route.as_ref().map(|r| r.effective_backend.as_str()).unwrap_or(args.backend.as_str())
            );
        }
        "pm" => {
            if args.target.is_empty() {
                println!("Steps: git log → test count → CI check → write pm-report.md")
            } else {
                let route = dry_run_route(cfg, profile, "pm", args);
                println!("Backend:      {}", args.backend);
                if let Some(route) = &route {
                    println!(
                        "Effective:    {}/{}",
                        route.effective_backend,
                        route.effective_model.as_deref().unwrap_or("default")
                    );
                    println!("Routing:      {}", route.routing_reason);
                }
                println!(
                    "Steps: collect manager memory/MRs/tickets/repo state → {} backend → structured PM plan → validated tickets in docs/tickets/",
                    route.as_ref().map(|r| r.effective_backend.as_str()).unwrap_or(args.backend.as_str())
                )
            }
        }
        "review" => {
            let route = dry_run_route(cfg, profile, "review", args);
            println!("Backend:      {}", args.backend);
            if let Some(route) = &route {
                println!(
                    "Effective:    {}/{}",
                    route.effective_backend,
                    route.effective_model.as_deref().unwrap_or("default")
                );
                println!("Routing:      {}", route.routing_reason);
            }
            if let Some(mr) = args.mr.as_deref() {
                println!("Review MR:    {}", mr);
            }
            if let Some(branch) = args.branch.as_deref() {
                println!("Source branch: {}", branch);
            }
            if args.current_branch {
                println!("Source branch: current branch");
            }
            println!("Steps: fetch target/source refs → explicit diff → bundle → routed review");
        }
        "experiment" => println!(
            "Steps: worktree → {} backend (research prompt) → collect artifacts → LLM judge → commit → draft MR",
            args.backend
        ),
        other => println!("mode '{}': not yet implemented", other),
    }
    println!("\n## Safety\n- No pushes, no MRs, no provider calls (dry run)");
    Ok(())
}

/// Build the task prompt for the agent.
#[allow(clippy::too_many_arguments)]
fn enforce_context_budget(
    cfg: &GahConfig,
    _profile: &Profile,
    profile_name: &str,
    backend: &str,
    phase: &str,
    fresh_context: bool,
    prompt: &str,
    session_dir: &Path,
    run_id: Option<&str>,
    ledger: &mut LedgerEntry,
) -> Result<String> {
    let context_cfg = cfg.context.effective(profile_name, backend);
    let build = match crate::context::enforce(prompt, &context_cfg) {
        Ok(build) => build,
        Err(err) => {
            ledger.set_failure(
                crate::ledger::FailureClass::ContextLimitExceeded,
                crate::ledger::FailureStage::AgentRun,
            );
            ledger.context_phase = Some(phase.to_string());
            ledger.context_estimated_tokens_before = Some(crate::context::estimate_tokens(prompt));
            ledger.context_estimated_tokens_after = None;
            ledger.context_compacted = true;
            return Err(err);
        }
    };
    ledger.context_phase = Some(phase.to_string());
    ledger.context_estimated_tokens_before = Some(build.estimated_tokens_before_reduction);
    ledger.context_estimated_tokens_after = Some(build.estimated_tokens_after_reduction);
    ledger.context_compacted = build.compacted;
    let _ = fs::write(
        session_dir.join("context-built.json"),
        serde_json::to_vec_pretty(&build)?,
    );
    let details = serde_json::json!({
        "phase": phase,
        "backend": backend,
        "estimated_tokens_before_reduction": build.estimated_tokens_before_reduction,
        "estimated_tokens_after_reduction": build.estimated_tokens_after_reduction,
        "soft_limit_tokens": context_cfg.soft_limit_tokens,
        "hard_limit_tokens": context_cfg.hard_limit_tokens,
        "compacted": build.compacted,
        "fresh_context": fresh_context,
        "largest_sections": build.largest_sections,
        "sources": build.sources,
    });
    let _ = crate::events::record_with_run_id(
        cfg,
        crate::events::EventType::ContextBuilt,
        Some(profile_name),
        ledger.work_id.as_deref(),
        run_id,
        details.to_string(),
    );
    Ok(build.prompt)
}

#[cfg(test)]
mod tests {
    use super::publish::{
        build_fix_or_improve_mr_body, build_metadata_rich_mr_body, build_mr_title,
        build_standard_mr_body, MrRenderContext,
    };
    use super::test_util::{gah_config, gah_config_with_ledger, init_repo, profile};
    use super::{
        apply_diff_stats, check_review_budget, derive_reviewer_tier, next_escalatory_reviewer,
        parse_review_verdict, parse_review_verdict_with_context, render_review_comment,
        review_escalation_reason, review_labels, reviewer_dedup_class, ReviewDiffBundle,
        ReviewGateContext, ReviewerTier, RouteDecision, TicketMetadata,
    };
    use crate::config::{CandidateConfig, Profile, RoutingPolicy};
    use crate::ledger::LedgerEntry;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    fn review_ledger_entry(
        profile_name: &str,
        prof: &Profile,
        branch: &str,
        verdict: &str,
        confidence: &str,
    ) -> LedgerEntry {
        let mut entry = LedgerEntry::new(profile_name, prof, "vibe", "review", "test", None, None);
        entry.branch = Some(branch.to_string());
        entry.validation_result = Some(verdict.to_string());
        entry.confidence_impact = Some(confidence.to_string());
        entry
    }

    fn paid_route_decision() -> RouteDecision {
        let mut route = route_decision("api-reviewer", Some("api-model"), false);
        route.routing_diagnostics = Some(crate::ledger::RoutingDiagnostics {
            selected_cost_class: Some("paid".into()),
            ..Default::default()
        });
        route
    }

    #[test]
    fn review_budget_counts_review_cycles_across_ticket_id_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(
            tmp.path(),
            RoutingPolicy {
                max_review_cycles_per_ticket: Some(2),
                ..RoutingPolicy::default()
            },
        );
        let prof = profile(tmp.path());
        for work_id in ["TICKET-42", "#42"] {
            let mut entry = review_ledger_entry("test", &prof, "gah/42", "NEEDS_FIX", "high");
            entry.work_id = Some(work_id.into());
            crate::ledger::append(&cfg, &entry).unwrap();
        }

        let block = check_review_budget(
            &cfg,
            &prof,
            "test",
            Some("#42"),
            &route_decision("vibe", Some("reviewer"), false),
        )
        .unwrap()
        .expect("two completed review cycles must block a third");
        assert!(block.reason.contains("2/2 review cycles"));
    }

    #[test]
    fn skipped_duplicate_reviews_do_not_consume_the_cycle_budget() {
        // Regression: a duplicate-review short-circuit (#109) launches no
        // reviewer and must not be indistinguishable from a real cycle when
        // counted by the review budget (#113) -- otherwise a ticket that is
        // re-observed several times without any new commits could exhaust its
        // budget purely from free, already-skipped reviews.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(
            tmp.path(),
            RoutingPolicy {
                max_review_cycles_per_ticket: Some(2),
                ..RoutingPolicy::default()
            },
        );
        let prof = profile(tmp.path());
        let mut real = review_ledger_entry("test", &prof, "gah/44", "NEEDS_FIX", "high");
        real.work_id = Some("#44".into());
        crate::ledger::append(&cfg, &real).unwrap();
        for _ in 0..5 {
            let mut skipped =
                review_ledger_entry("test", &prof, "gah/44", "skipped_duplicate_review", "high");
            skipped.work_id = Some("#44".into());
            crate::ledger::append(&cfg, &skipped).unwrap();
        }

        let block = check_review_budget(
            &cfg,
            &prof,
            "test",
            Some("#44"),
            &route_decision("vibe", Some("reviewer"), false),
        )
        .unwrap();
        assert!(
            block.is_none(),
            "five free skipped-duplicate reviews must not exhaust a 2-cycle budget"
        );
    }

    #[test]
    fn paid_review_budget_only_blocks_explicitly_paid_route() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(
            tmp.path(),
            RoutingPolicy {
                max_review_cycles_per_ticket: Some(3),
                max_paid_reviews_per_ticket: Some(1),
                ..RoutingPolicy::default()
            },
        );
        let prof = profile(tmp.path());
        let mut entry = review_ledger_entry("test", &prof, "gah/43", "APPROVE", "high");
        entry.work_id = Some("#43".into());
        entry.usage.usage_classification = Some("api_key_backed".into());
        crate::ledger::append(&cfg, &entry).unwrap();

        let paid = check_review_budget(&cfg, &prof, "test", Some("#43"), &paid_route_decision())
            .unwrap()
            .expect("paid cap must block another configured paid reviewer");
        assert!(paid.reason.contains("1/1 API-backed reviews"));

        let quota = check_review_budget(
            &cfg,
            &prof,
            "test",
            Some("#43"),
            &route_decision("agy", Some("sonnet"), false),
        )
        .unwrap();
        assert!(quota.is_none(), "paid history must not block a quota route");
    }

    #[test]
    fn review_budget_fails_open_without_ticket_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        assert!(
            check_review_budget(&cfg, &prof, "test", None, &paid_route_decision(),)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn review_escalation_reason_none_when_no_prior_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            None
        );
    }

    #[test]
    fn review_escalation_reason_none_with_single_needs_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "NEEDS_FIX", "high"),
        )
        .unwrap();
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            None
        );
    }

    #[test]
    fn human_review_starts_the_bounded_second_opinion_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "HUMAN_REVIEW", "high"),
        )
        .unwrap();

        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            Some("human_review")
        );
    }

    #[test]
    fn escalation_uses_each_configured_backend_model_once_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let mut prof = profile(tmp.path());
        prof.routing.escalatory_reviewers = vec![
            CandidateConfig {
                backend: "claude".into(),
                model: Some("sonnet".into()),
                ..Default::default()
            },
            CandidateConfig {
                backend: "opencode".into(),
                model: Some("nous-portal/z-ai/glm-5.2".into()),
                ..Default::default()
            },
        ];
        let mut prior = review_ledger_entry("test", &prof, "gah/branch-1", "HUMAN_REVIEW", "high");
        prior.effective_backend = "agy".into();
        prior.effective_model = Some("Claude Sonnet 4.6 (Thinking)".into());
        crate::ledger::append(&cfg, &prior).unwrap();

        let first = next_escalatory_reviewer(&cfg, &prof, "test", "gah/branch-1", None)
            .expect("first second opinion");
        assert_eq!(
            (first.backend.as_str(), first.model.as_deref()),
            ("claude", Some("sonnet"))
        );

        let second = next_escalatory_reviewer(
            &cfg,
            &prof,
            "test",
            "gah/branch-1",
            Some(("claude", Some("sonnet"))),
        )
        .expect("second second opinion");
        assert_eq!(
            (second.backend.as_str(), second.model.as_deref()),
            ("opencode", Some("nous-portal/z-ai/glm-5.2"))
        );

        let mut claude = review_ledger_entry("test", &prof, "gah/branch-1", "HUMAN_REVIEW", "high");
        claude.effective_backend = "claude".into();
        claude.effective_model = Some("sonnet".into());
        crate::ledger::append(&cfg, &claude).unwrap();
        assert!(next_escalatory_reviewer(
            &cfg,
            &prof,
            "test",
            "gah/branch-1",
            Some(("opencode", Some("nous-portal/z-ai/glm-5.2"))),
        )
        .is_none());
    }

    #[test]
    fn escalation_recognizes_codex_config_default_model_as_tried() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let mut prof = profile(tmp.path());
        prof.codex_args = vec!["--model".into(), "gpt-5-codex".into()];
        prof.routing.escalatory_reviewers = vec![
            CandidateConfig {
                backend: "codex".into(),
                model: None,
                ..Default::default()
            },
            CandidateConfig {
                backend: "opencode".into(),
                model: Some("nous-portal/z-ai/glm-5.2".into()),
                ..Default::default()
            },
        ];

        let first = next_escalatory_reviewer(&cfg, &prof, "test", "gah/branch-1", None)
            .expect("first second opinion");
        assert_eq!(
            (first.backend.as_str(), first.model.as_deref()),
            ("codex", None)
        );

        // The ledger records whatever model routing actually backfilled for
        // codex (its config-file default), not the unset config value.
        let mut prior = review_ledger_entry("test", &prof, "gah/branch-1", "HUMAN_REVIEW", "high");
        prior.effective_backend = "codex".into();
        prior.effective_model = Some("gpt-5-codex".into());
        crate::ledger::append(&cfg, &prior).unwrap();

        let second = next_escalatory_reviewer(
            &cfg,
            &prof,
            "test",
            "gah/branch-1",
            Some(("codex", Some("gpt-5-codex"))),
        )
        .expect("codex must be recognized as already tried, advancing the chain");
        assert_eq!(
            (second.backend.as_str(), second.model.as_deref()),
            ("opencode", Some("nous-portal/z-ai/glm-5.2"))
        );
    }

    #[test]
    fn review_escalation_reason_repeated_failure_on_two_consecutive_needs_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "NEEDS_FIX", "high"),
        )
        .unwrap();
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "REJECT", "high"),
        )
        .unwrap();
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            Some("repeated_needs_fix")
        );
    }

    #[test]
    fn review_escalation_reason_none_when_needs_fix_not_consecutive_at_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "NEEDS_FIX", "high"),
        )
        .unwrap();
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "APPROVE", "high"),
        )
        .unwrap();
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            None
        );
    }

    #[test]
    fn review_escalation_reason_low_confidence_on_most_recent_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "APPROVE", "high"),
        )
        .unwrap();
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "APPROVE", "low"),
        )
        .unwrap();
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            Some("low_confidence")
        );
    }

    #[test]
    fn review_escalation_reason_none_with_medium_confidence_and_no_repeated_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "APPROVE", "medium"),
        )
        .unwrap();
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            None
        );
    }

    #[test]
    fn review_escalation_reason_ignores_other_branch_and_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/other-branch", "NEEDS_FIX", "high"),
        )
        .unwrap();
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/other-branch", "REJECT", "high"),
        )
        .unwrap();
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("other-profile", &prof, "gah/branch-1", "NEEDS_FIX", "high"),
        )
        .unwrap();
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("other-profile", &prof, "gah/branch-1", "REJECT", "high"),
        )
        .unwrap();
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            None
        );
    }

    #[test]
    fn review_escalation_reason_respects_configured_fix_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(
            tmp.path(),
            RoutingPolicy {
                max_fix_attempts_per_mr: Some(3),
                ..RoutingPolicy::default()
            },
        );
        let prof = profile(tmp.path());
        for _ in 0..2 {
            crate::ledger::append(
                &cfg,
                &review_ledger_entry("test", &prof, "gah/branch-1", "NEEDS_FIX", "high"),
            )
            .unwrap();
        }
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            None
        );
        crate::ledger::append(
            &cfg,
            &review_ledger_entry("test", &prof, "gah/branch-1", "REJECT", "high"),
        )
        .unwrap();
        assert_eq!(
            review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
            Some("repeated_needs_fix")
        );
    }

    fn route_decision(backend: &str, model: Option<&str>, fallback_used: bool) -> RouteDecision {
        RouteDecision {
            requested_backend: backend.to_string(),
            effective_backend: backend.to_string(),
            requested_model: model.map(str::to_string),
            effective_model: model.map(str::to_string),
            effective_quota_pool: None,
            routing_reason: "test".to_string(),
            fallback_used,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        }
    }

    #[test]
    fn reviewer_tier_strong_when_backend_and_model_match_strong_config() {
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.routing.strong_review_backend = Some("claude".into());
        prof.routing.strong_review_model = Some("sonnet".into());
        let cfg = gah_config(RoutingPolicy::default());

        let route = route_decision("claude", Some("sonnet"), false);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &route),
            ReviewerTier::Strong
        );
    }

    #[test]
    fn reviewer_tier_weak_when_backend_matches_legacy_weak_config() {
        // Issue #233: the legacy single `weak_review_*` entry still feeds
        // routing backfill, but it must not grant the auto-merge-eligible
        // escalatory tier.
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.routing.weak_review_backend = Some("codex".into());
        let cfg = gah_config(RoutingPolicy::default());

        let route = route_decision("codex", None, true);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &route),
            ReviewerTier::Weak
        );
    }

    #[test]
    fn reviewer_tier_escalatory_for_explicit_escalatory_reviewers_list_entry() {
        // Issue #233: an explicitly declared escalatory reviewer is the only
        // path to the auto-merge-eligible tier.
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        let candidate = |backend: &str, model: &str| crate::config::CandidateConfig {
            backend: backend.into(),
            model: Some(model.into()),
            ..Default::default()
        };
        prof.routing.escalatory_reviewers = vec![
            candidate("claude", "claude-sonnet-4"),
            candidate("kimi", "kimi-k2"),
            candidate("glm", "glm-4.7"),
        ];
        prof.routing.weak_review_backend = Some("claude".into());
        prof.routing.weak_review_model = Some("claude-sonnet-4".into());
        let cfg = gah_config(RoutingPolicy::default());

        let route = route_decision("claude", Some("claude-sonnet-4"), true);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &route),
            ReviewerTier::Escalatory
        );
    }

    #[test]
    fn reviewer_tier_routine_reviewer_is_strong() {
        // Issue #123: ROUTINE_REVIEWER is the single STRONG first-line reviewer.
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.routing.routine_reviewer = Some(crate::config::CandidateConfig {
            backend: "vibe".into(),
            model: Some("mistral-medium-3.5".into()),
            ..Default::default()
        });
        let cfg = gah_config(RoutingPolicy::default());

        let route = route_decision("vibe", Some("mistral-medium-3.5"), true);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &route),
            ReviewerTier::Strong
        );
    }

    #[test]
    fn reviewer_tier_standard_when_neither_strong_nor_weak_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let prof = profile(tmp.path());
        let cfg = gah_config(RoutingPolicy::default());

        let route = route_decision("claude", Some("haiku"), false);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &route),
            ReviewerTier::Standard
        );
    }

    #[test]
    fn reviewer_tier_strong_for_any_review_candidates_entry_not_just_the_exact_strong_config() {
        // Regression: found live -- strong_review_backend/model is a single
        // hardcoded pair that must be manually kept in sync with
        // review_candidates. Falling back from agy to agy-second (or
        // claude) for the exact same Sonnet-class reviewer silently
        // downgraded reviewer_tier to "standard", even though
        // review_candidates explicitly lists all three as the operator's
        // own declared strong-reviewer pool.
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.routing.strong_review_backend = Some("agy".into());
        prof.routing.strong_review_model = Some("Claude Sonnet 4.6 (Thinking)".into());
        let candidate = |backend: &str, model: &str| crate::config::CandidateConfig {
            backend: backend.into(),
            model: Some(model.into()),
            quota_pool: None,
            priority: 0,
            included_in_quota: false,
            marginal_cost_usd: None,
            quota_usage_percent: None,
            quota_days_remaining: None,
            requires_approval: false,
        };
        prof.routing.review_candidates = Some(vec![
            candidate("agy", "Claude Sonnet 4.6 (Thinking)"),
            candidate("agy-second", "Claude Sonnet 4.6 (Thinking)"),
            candidate("claude", "claude-sonnet-4"),
        ]);
        let cfg = gah_config(RoutingPolicy::default());

        let via_agy_second =
            route_decision("agy-second", Some("Claude Sonnet 4.6 (Thinking)"), true);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &via_agy_second),
            ReviewerTier::Strong
        );
        let via_claude = route_decision("claude", Some("claude-sonnet-4"), true);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &via_claude),
            ReviewerTier::Strong
        );
    }

    #[test]
    fn reviewer_tier_falls_back_to_defaults_routing_when_profile_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let prof = profile(tmp.path());
        let defaults_routing = RoutingPolicy {
            strong_review_backend: Some("claude".into()),
            ..Default::default()
        };
        let cfg = gah_config(defaults_routing);

        let route = route_decision("claude", None, false);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &route),
            ReviewerTier::Strong
        );
    }

    #[test]
    fn weak_needs_fix_uses_repair_budget_before_human_escalation() {
        // Weak review remains visible and cannot auto-approve, but a concrete
        // NEEDS_FIX result must flow into the configured repair budget.
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.routing.weak_review_backend = Some("codex".into());
        let cfg = gah_config(RoutingPolicy::default());
        let route = route_decision("codex", None, true);
        assert_eq!(
            derive_reviewer_tier(&cfg, &prof, &route),
            ReviewerTier::Weak
        );

        let json = r#"{"verdict":"NEEDS_FIX","confidence":"high","human_required":false,"blocking_findings":["src/lib.rs: missing guard"],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:src/lib.rs"]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let verdict = parse_review_verdict(json, &route, &usage, ReviewerTier::Weak).unwrap();

        assert_eq!(
            verdict.verdict, "NEEDS_FIX",
            "verdict text is never rewritten"
        );
        assert_eq!(verdict.reviewer_tier.as_deref(), Some("weak"));
        assert!(!verdict.human_required);
        assert_eq!(verdict.confidence, "medium");
        assert_eq!(review_labels(&verdict), vec!["gah-needs-fix"]);
    }

    #[test]
    fn approve_from_weak_tier_still_requires_human_review() {
        let route = route_decision("codex", None, true);
        let json = r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:src/lib.rs"]}"#;
        let verdict = parse_review_verdict(
            json,
            &route,
            &crate::ledger::LedgerUsage::default(),
            ReviewerTier::Weak,
        )
        .unwrap();
        assert!(verdict.human_required);
        assert_eq!(verdict.confidence, "medium");
        assert_eq!(
            review_labels(&verdict),
            vec!["gah-review-weak", "gah-human-review"]
        );
    }

    #[test]
    fn provisional_human_review_is_labeled_for_escalation_not_handoff() {
        let route = route_decision("agy", Some("Claude Sonnet 4.6 (Thinking)"), false);
        let json = r#"{"verdict":"HUMAN_REVIEW","confidence":"high","human_required":true,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":[]}"#;
        let mut verdict = parse_review_verdict(
            json,
            &route,
            &crate::ledger::LedgerUsage::default(),
            ReviewerTier::Strong,
        )
        .unwrap();

        // This is exactly the state after the next configured reviewer was
        // found. It must remain controller-actionable without a human alert.
        verdict.human_required = false;
        assert_eq!(review_labels(&verdict), vec!["gah-review-escalating"]);
    }

    #[test]
    fn escalatory_dedup_identity_keeps_distinct_second_opinions() {
        let claude = route_decision("claude", Some("sonnet"), false);
        let glm = route_decision("opencode", Some("nous-portal/z-ai/glm-5.2"), false);
        assert_ne!(
            reviewer_dedup_class(ReviewerTier::Escalatory, &claude),
            reviewer_dedup_class(ReviewerTier::Escalatory, &glm),
        );
    }

    #[test]
    fn reject_from_weak_tier_uses_repair_budget_before_human_escalation() {
        let route = route_decision("codex", None, true);
        let json = r#"{"verdict":"REJECT","confidence":"high","human_required":false,"blocking_findings":["src/lib.rs: invalid state transition"],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:src/lib.rs"]}"#;
        let verdict = parse_review_verdict(
            json,
            &route,
            &crate::ledger::LedgerUsage::default(),
            ReviewerTier::Weak,
        )
        .unwrap();
        assert!(!verdict.human_required);
        assert_eq!(verdict.confidence, "medium");
        assert_eq!(review_labels(&verdict), vec!["gah-needs-fix"]);
    }

    #[test]
    fn grounded_approve_from_strong_tier_is_not_forced_to_human_review() {
        let json = r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:src/internal.rs","ci:passed"]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/internal.rs\n".to_string(),
                diff: "+fn internal_only() {}\n".to_string(),
            },
            Some("passed"),
        );
        let verdict =
            parse_review_verdict_with_context(json, &route, &usage, ReviewerTier::Strong, &context)
                .unwrap();

        assert_eq!(verdict.reviewer_tier.as_deref(), Some("strong"));
        assert!(!verdict.human_required);
        assert_eq!(verdict.confidence, "high");
        assert_eq!(review_labels(&verdict), vec!["gah-ready-for-human"]);
    }

    #[test]
    fn approve_without_evidence_is_forced_to_human_review() {
        let json = r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);

        let verdict = parse_review_verdict(json, &route, &usage, ReviewerTier::Strong).unwrap();

        assert_eq!(verdict.verdict, "HUMAN_REVIEW");
        assert!(verdict.human_required);
        assert_eq!(
            verdict.safety_gate_reason.as_deref(),
            Some("APPROVE omitted required concrete review evidence")
        );
    }

    #[test]
    fn contract_surface_change_is_held_even_when_reviewer_paraphrases_or_omits_it() {
        // Regression for PR #284: the gate must inspect the actual changed
        // contract surface, not depend on the reviewer spelling out a
        // particular "schema-breaking" phrase in its findings.
        let json = r#"{
            "verdict":"APPROVE",
            "confidence":"high",
            "human_required":false,
            "blocking_findings":[],
            "non_blocking_findings":[],
            "risk_notes":[],
            "evidence":["file:src/telemetry/records.rs", "ci:passed"]
        }"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("agy", Some("Claude Sonnet"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/telemetry/records.rs\n".to_string(),
                diff: "-    pub attempts_started: u32,\n+    pub attempts_started: Option<u32>,\n"
                    .to_string(),
            },
            Some("passed"),
        );

        let verdict =
            parse_review_verdict_with_context(json, &route, &usage, ReviewerTier::Strong, &context)
                .unwrap();

        assert_eq!(verdict.verdict, "HUMAN_REVIEW");
        assert!(verdict.human_required);
        assert!(verdict
            .safety_gate_reason
            .as_deref()
            .unwrap_or_default()
            .contains("contract surface"));
    }

    #[test]
    fn versioned_contract_change_with_compatibility_evidence_can_be_approved() {
        let json = r#"{
            "verdict":"APPROVE",
            "confidence":"high",
            "human_required":false,
            "blocking_findings":[],
            "non_blocking_findings":[],
            "risk_notes":[],
            "evidence":["file:src/telemetry/records.rs", "ci:passed"],
            "compatibility_evidence":["file:src/telemetry/records.rs", "mechanism:schema-version"]
        }"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("agy", Some("Claude Sonnet"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/telemetry/records.rs\n".to_string(),
                diff: "-pub const SCHEMA_VERSION: u32 = 3;\n+pub const SCHEMA_VERSION: u32 = 4;\n"
                    .to_string(),
            },
            Some("passed"),
        );

        let verdict =
            parse_review_verdict_with_context(json, &route, &usage, ReviewerTier::Strong, &context)
                .unwrap();

        assert_eq!(verdict.verdict, "APPROVE");
        assert!(!verdict.human_required);
        assert!(verdict.safety_gate_reason.is_none());
    }

    #[test]
    fn production_approval_requires_exact_changed_file_and_control_plane_ci() {
        let json = r#"{"verdict":"Approve","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:not-in-diff.rs","ci:passed"]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/dispatch.rs\n".to_string(),
                diff: "+fn hardened_review() {}\n".to_string(),
            },
            Some("passed"),
        );

        let verdict =
            parse_review_verdict_with_context(json, &route, &usage, ReviewerTier::Strong, &context)
                .unwrap();

        assert_eq!(verdict.verdict, "HUMAN_REVIEW");
        assert!(verdict
            .safety_gate_reason
            .as_deref()
            .unwrap_or_default()
            .contains("not grounded"));
    }

    #[test]
    fn production_approval_does_not_require_ci_before_review() {
        let json = r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:src/internal.rs"]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/internal.rs\n".to_string(),
                diff: "+fn internal_only() {}\n".to_string(),
            },
            Some("pending"),
        );

        let verdict =
            parse_review_verdict_with_context(json, &route, &usage, ReviewerTier::Strong, &context)
                .unwrap();

        assert_eq!(verdict.verdict, "APPROVE");
        assert!(!verdict.human_required);
    }

    #[test]
    fn production_approval_cannot_falsely_claim_ci_passed_before_ci_finishes() {
        let json = r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["file:src/internal.rs","ci:passed"]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/internal.rs\n".to_string(),
                diff: "+fn internal_only() {}\n".to_string(),
            },
            Some("pending"),
        );

        let verdict =
            parse_review_verdict_with_context(json, &route, &usage, ReviewerTier::Strong, &context)
                .unwrap();

        assert_eq!(verdict.verdict, "HUMAN_REVIEW");
        assert!(verdict
            .safety_gate_reason
            .as_deref()
            .unwrap_or_default()
            .contains("claimed passed CI"));
    }

    #[test]
    fn production_approval_with_prose_is_held_to_prevent_hidden_findings() {
        let review_text = "Found a worrying edge case.\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src/dispatch.rs\",\"ci:passed\"]}";
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/dispatch.rs\n".to_string(),
                diff: "+fn hardened_review() {}\n".to_string(),
            },
            Some("passed"),
        );

        let verdict = parse_review_verdict_with_context(
            review_text,
            &route,
            &usage,
            ReviewerTier::Strong,
            &context,
        )
        .unwrap();

        assert_eq!(verdict.verdict, "HUMAN_REVIEW");
        assert!(verdict
            .safety_gate_reason
            .as_deref()
            .unwrap_or_default()
            .contains("substantive prose"));
    }

    #[test]
    fn inert_review_notes_header_does_not_hide_or_block_a_structured_approval() {
        let review_text = "Review notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src/dispatch.rs\"]}";
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/dispatch.rs\n".to_string(),
                diff: "+fn hardened_review() {}\n".to_string(),
            },
            Some("pending"),
        );

        let verdict = parse_review_verdict_with_context(
            review_text,
            &route,
            &usage,
            ReviewerTier::Strong,
            &context,
        )
        .unwrap();

        assert_eq!(verdict.verdict, "APPROVE");
    }

    #[test]
    fn agy_execution_trace_does_not_hide_or_block_a_structured_approval() {
        // Live `agy --print` emits this execution-plan trace before the final
        // response. It is transport metadata rather than a review finding.
        let review_text = "I will inspect the diff.\nI will run the focused tests.\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src/dispatch.rs\"]}";
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("agy", Some("Claude Sonnet 4.8 (Thinking)"), false);
        let context = ReviewGateContext::from_diff_bundle(
            &ReviewDiffBundle {
                files: "src/dispatch.rs\n".to_string(),
                diff: "+fn hardened_review() {}\n".to_string(),
            },
            Some("pending"),
        );

        let verdict = parse_review_verdict_with_context(
            review_text,
            &route,
            &usage,
            ReviewerTier::Strong,
            &context,
        )
        .unwrap();

        assert_eq!(verdict.verdict, "APPROVE");
        assert!(!verdict.human_required);
    }

    #[test]
    fn approve_with_blocking_findings_is_forced_to_human_review() {
        let json = r#"{
            "verdict":"APPROVE",
            "confidence":"high",
            "human_required":false,
            "blocking_findings":["data loss on retry"],
            "non_blocking_findings":[],
            "risk_notes":[],
            "evidence":["reproduced in a unit test"]
        }"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("agy", Some("Claude Sonnet"), false);

        let verdict = parse_review_verdict(json, &route, &usage, ReviewerTier::Strong).unwrap();

        assert_eq!(verdict.verdict, "HUMAN_REVIEW");
        assert!(verdict.human_required);
        assert_eq!(
            verdict.safety_gate_reason.as_deref(),
            Some("APPROVE contradicted non-empty blocking_findings")
        );
    }

    #[test]
    fn low_confidence_approve_forces_human_review_regardless_of_tier() {
        // Low self-reported CONFIDENCE (the reviewer's own uncertainty) is a
        // separate signal from reviewer TIER (who reviewed) -- even a
        // strong-tier reviewer returning APPROVE with confidence:"low" must
        // still get human eyes.
        let json = r#"{"verdict":"APPROVE","confidence":"low","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["cargo test passed"]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);
        let verdict = parse_review_verdict(json, &route, &usage, ReviewerTier::Strong).unwrap();

        assert_eq!(verdict.reviewer_tier.as_deref(), Some("strong"));
        assert!(verdict.human_required);
        assert_eq!(
            review_labels(&verdict),
            vec!["gah-review-weak", "gah-human-review"]
        );
    }

    #[test]
    fn parse_review_verdict_handles_vibe_json_output() {
        // Test parsing of actual Vibe CLI output format
        // Vibe with --output text returns just the content, which should be a ReviewVerdict JSON object
        let vibe_json_output = r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[],"evidence":["vibe inspected the diff"]}"#;

        let route = crate::routing::RouteDecision {
            requested_backend: "vibe".to_string(),
            effective_backend: "vibe".to_string(),
            requested_model: Some("mistral-medium-3.5".to_string()),
            effective_model: Some("mistral-medium-3.5".to_string()),
            effective_quota_pool: None,
            routing_reason: "test".to_string(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        };
        let usage = crate::ledger::LedgerUsage::default();

        let verdict =
            parse_review_verdict(vibe_json_output, &route, &usage, ReviewerTier::Standard).unwrap();

        assert_eq!(verdict.verdict, "APPROVE");
        assert_eq!(verdict.confidence, "high");
        assert!(!verdict.human_required);
        assert_eq!(verdict.blocking_findings, Vec::<String>::new());
        assert_eq!(verdict.non_blocking_findings, Vec::<String>::new());
        assert_eq!(verdict.risk_notes, Vec::<String>::new());
        assert_eq!(verdict.reviewer_backend.as_deref(), Some("vibe"));
        assert_eq!(verdict.effective_backend.as_deref(), Some("vibe"));
        assert_eq!(
            verdict.effective_model.as_deref(),
            Some("mistral-medium-3.5")
        );
    }

    #[test]
    fn parse_review_verdict_fails_on_vibe_malformed_json() {
        // Test that malformed JSON from Vibe fails gracefully
        let malformed_output = r#"This is not valid JSON from Vibe"#;

        let route = crate::routing::RouteDecision {
            requested_backend: "vibe".to_string(),
            effective_backend: "vibe".to_string(),
            requested_model: None,
            effective_model: None,
            effective_quota_pool: None,
            routing_reason: "test".to_string(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        };
        let usage = crate::ledger::LedgerUsage::default();

        let result = parse_review_verdict(malformed_output, &route, &usage, ReviewerTier::Standard);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("reviewer did not return verdict JSON"));
    }

    #[test]
    fn parse_review_verdict_fails_on_vibe_empty_output() {
        // Test that empty output from Vibe fails gracefully
        let empty_output = "";

        let route = crate::routing::RouteDecision {
            requested_backend: "vibe".to_string(),
            effective_backend: "vibe".to_string(),
            requested_model: None,
            effective_model: None,
            effective_quota_pool: None,
            routing_reason: "test".to_string(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        };
        let usage = crate::ledger::LedgerUsage::default();

        let result = parse_review_verdict(empty_output, &route, &usage, ReviewerTier::Standard);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("reviewer did not return verdict JSON"));
    }

    #[test]
    fn parse_review_verdict_skips_incidental_empty_braces_in_prose() {
        // Regression (TICKET-177 / live repro): reviewer prose discusses a
        // regex literal containing a bare `{}` format-string placeholder
        // BEFORE the real JSON verdict block. The old first-match brace
        // scanner grabbed the incidental `{}` (a structurally valid but
        // empty JSON object) and failed to deserialize into ReviewVerdict.
        let review_text = r##"## Review Notes

### Correctness

Found an issue: `find_header_u64` uses `r#"(?i)"?{}\b"?\s*[:=]\s*"?([0-9]+)"?"#`
which lacks a leading boundary check.

## JSON Summary

```json
{
  "verdict": "NEEDS_FIX",
  "confidence": "high",
  "human_required": false,
  "blocking_findings": ["regex lacks leading boundary assertion"],
  "non_blocking_findings": [],
  "risk_notes": []
}
```
"##;

        let route = crate::routing::RouteDecision {
            requested_backend: "vibe".to_string(),
            effective_backend: "vibe".to_string(),
            requested_model: Some("mistral-medium-3.5".to_string()),
            effective_model: Some("mistral-medium-3.5".to_string()),
            effective_quota_pool: None,
            routing_reason: "test".to_string(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        };
        let usage = crate::ledger::LedgerUsage::default();

        let verdict =
            parse_review_verdict(review_text, &route, &usage, ReviewerTier::Standard).unwrap();

        assert_eq!(verdict.verdict, "NEEDS_FIX");
        assert_eq!(verdict.confidence, "high");
        assert_eq!(
            verdict.blocking_findings,
            vec!["regex lacks leading boundary assertion".to_string()]
        );
    }

    // Issue #95: a tombstone entry (mode="clear_attempts") resets the
    // prior_attempt_count and genuine_agent_failure_count for its work_id.

    // Parallel workers: a fresh claim marks a ticket has_active_claim,
    // excluding it from re-selection; a real completion entry after the
    // claim resolves it, and a stale claim stops blocking on its own.

    // Issue #95: entries after a tombstone DO count.

    // Issue #95: infra failures don't count toward genuine_agent_failure_count

    #[test]
    fn mr_title_uses_ticket_context_and_preserves_draft_fail_prefix() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-058".into()),
            work_id: Some("TICKET-058".into()),
            title: Some("Descriptive MR Titles".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };
        assert_eq!(
            build_mr_title("fix", "real", false, Some(&ticket)),
            "[GAH] Fix: TICKET-058 Descriptive MR Titles"
        );
        assert_eq!(
            build_mr_title("fix", "real", true, Some(&ticket)),
            "[GAH][DRAFT-FAIL] Fix: TICKET-058 Descriptive MR Titles"
        );
    }

    #[test]
    fn mr_title_uses_native_issue_identity_without_ticket_alias() {
        let ticket = TicketMetadata {
            ticket_id: Some("#319".into()),
            work_id: Some("#319".into()),
            title: Some("Use native issue numbers".into()),
            issue_number: Some("319".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        assert_eq!(
            build_mr_title("fix", "real", false, Some(&ticket)),
            "[GAH] Fix: #319 Use native issue numbers"
        );
    }

    #[test]
    fn render_review_comment_includes_non_blocking_findings_and_risk_notes() {
        // Regression: a verdict with zero blocking_findings (e.g. a
        // low-confidence APPROVE) still carries real substance in these two
        // fields. The posted PR comment was silently dropping both, leaving
        // reviewers with nothing but a bare verdict/confidence line and no
        // actual feedback.
        let verdict: crate::models::ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE","confidence":"low","human_required":true,
                "blocking_findings":[],
                "non_blocking_findings":["missing test coverage on one path"],
                "risk_notes":["new module coupling"]}"#,
        )
        .unwrap();
        let comment = render_review_comment(&verdict, Path::new("/tmp/session"));
        assert!(comment.contains("Non-blocking findings:"));
        assert!(comment.contains("missing test coverage on one path"));
        assert!(comment.contains("Risk notes:"));
        assert!(comment.contains("new module coupling"));
    }

    #[test]
    fn render_review_comment_prints_gate_reason_once() {
        let mut verdict: crate::models::ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"HUMAN_REVIEW","confidence":"high","human_required":true,
                "blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#,
        )
        .unwrap();
        verdict.safety_gate_reason = Some("APPROVE omitted grounded evidence".into());

        let comment = render_review_comment(&verdict, Path::new("/tmp/session"));
        assert_eq!(
            comment.matches("APPROVE omitted grounded evidence").count(),
            1
        );
    }

    // published_review_verdict_strips_internal_tier and
    // render_review_comment_publishes_approve_not_internal_tier used to pin
    // that the internal APPROVE_STRONG/APPROVE_WEAK routing tier never leaked
    // into human-facing text. Now that the verdict vocabulary has no
    // internal-only tier at all (verdict is always one of
    // APPROVE/NEEDS_FIX/REJECT/HUMAN_REVIEW), that property holds by
    // construction and there is nothing left to regress -- deleted rather
    // than kept as tests asserting an invariant that can no longer break.

    #[test]
    fn apply_diff_stats_reports_zero_before_commit_but_correct_after() {
        // Regression: diff_stats compares origin/<target> against HEAD, so
        // calling apply_diff_stats while real changes are still uncommitted
        // working-tree modifications (HEAD hasn't moved) always reports
        // "0 file(s) changed, +0, -0" -- this is exactly the bug that put
        // that false summary into real MR bodies. dispatch.rs's real call
        // sites now run this after the commit; this test pins why order
        // matters by exercising both states directly.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        let initial_sha = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        // Fake an "origin/main" ref without a real remote, matching how
        // diff_stats/changed_files/has_changes all resolve their comparison
        // point in real dispatch runs.
        Command::new("git")
            .args(["update-ref", "refs/remotes/origin/main", &initial_sha])
            .current_dir(repo)
            .output()
            .unwrap();

        fs::write(repo.join("new_file.txt"), "line one\nline two\n").unwrap();

        let mut prof = profile(repo);
        prof.local_path = repo.display().to_string();
        let mut ledger = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);

        // Before commit: real change exists in the working tree, but HEAD
        // hasn't moved, so the origin/main...HEAD comparison sees nothing.
        apply_diff_stats(&mut ledger, repo, "main");
        assert_eq!(ledger.files_changed, Some(0));

        Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(repo)
            .output()
            .unwrap();

        // After commit: HEAD has moved, so the comparison now sees the
        // real change -- this is what dispatch.rs's real call sites rely on.
        apply_diff_stats(&mut ledger, repo, "main");
        assert_eq!(ledger.files_changed, Some(1));
        assert_eq!(ledger.insertions, Some(2));
        assert_eq!(ledger.deletions, Some(0));
    }

    #[test]
    fn mr_title_missing_metadata_fallback() {
        // Without ticket metadata, it should fall back to mode + repo_id
        let title = build_mr_title("fix", "real", false, None);
        assert_eq!(title, "[GAH] Fix: real");

        let title_draft = build_mr_title("fix", "real", true, None);
        assert_eq!(title_draft, "[GAH][DRAFT-FAIL] Fix: real");
    }

    #[test]
    fn mr_title_suggested_mr_title_used() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-093".into()),
            work_id: Some("TICKET-093".into()),
            title: Some("Heading Title".into()),
            suggested_mr_title: Some(
                "Derive PR titles from authoritative structured work metadata".into(),
            ),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        // When suggested_mr_title is present and authoritative, use it with the ID
        let title = build_mr_title("fix", "real", false, Some(&ticket));
        assert_eq!(
            title,
            "[GAH] Fix: TICKET-093 Derive PR titles from authoritative structured work metadata"
        );
    }

    #[test]
    fn mr_title_graceful_truncation() {
        let long_title = "a".repeat(300);
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-093".into()),
            work_id: Some("TICKET-093".into()),
            title: Some(long_title),
            is_authoritative: true,
            ..TicketMetadata::default()
        };

        let title = build_mr_title("fix", "real", false, Some(&ticket));
        assert!(title.len() <= 255);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn metadata_rich_mr_body_includes_structured_sections() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-094".into()),
            work_id: Some("TICKET-094".into()),
            title: Some("Authoritative PR Description".into()),
            summary: Some("Authoritative PR Description".into()),
            problem: Some("The old MR body only showed a minimal template.".into()),
            goal: Some("Generate PR descriptions from structured metadata.".into()),
            acceptance_criteria: vec![
                "Description includes structured sections".into(),
                "Legacy fallback remains available".into(),
            ],
            constraints: vec!["Do not dump raw prompts".into()],
            source: Some("docs/tickets/TICKET-094-authoritative-pr-description.md".into()),
            is_authoritative: true,
            ..TicketMetadata::default()
        };
        let tmp = tempfile::tempdir().unwrap();
        let mut ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );
        ledger.validation_result = Some("passed".into());
        ledger.files_changed = Some(2);
        ledger.insertions = Some(14);
        ledger.deletions = Some(3);
        ledger.attempts_started = Some(2);
        ledger.attempts_completed = Some(2);
        ledger.fallback_used = true;

        let validation_commands = vec!["cargo test".into(), "cargo fmt --check".into()];
        let backend_summary = "Fixed the PR description to include reasoning.";
        let ctx = MrRenderContext {
            backend: "codex",
            model: "gpt-5.4",
            branch: "gah/repo-123",
            target_branch: "main",
            validation_commands: &validation_commands,
            ledger: &ledger,
            backend_summary,
        };
        let body = build_fix_or_improve_mr_body("fix", Some(&ticket), &ctx, true);

        assert!(body.contains("## Work Item"));
        assert!(body.contains("ID: `TICKET-094`"));
        assert!(body.contains("## Problem"));
        assert!(body.contains("The old MR body only showed a minimal template."));
        assert!(body.contains("## Goal"));
        assert!(body.contains("## Acceptance Criteria"));
        assert!(body.contains("- Description includes structured sections"));
        assert!(body.contains("## Constraints"));
        assert!(body.contains("- Do not dump raw prompts"));
        assert!(body.contains("## What changed and why"));
        assert!(body.contains("Fixed the PR description to include reasoning."));
        assert!(body.contains("## Validation"));
        assert!(body.contains("Outcome: passed"));
        assert!(body.contains("- `cargo test`"));
        assert!(body.contains("## Backend / Model"));
        assert!(body.contains("## Attempts"));
        assert!(body.contains("Fallback used: yes"));
        assert!(body.contains("## Source"));
        assert!(body.contains("docs/tickets/TICKET-094-authoritative-pr-description.md"));
        assert!(!body.contains("## Changes"));
        assert!(!body.contains("## Branch"));
        assert!(!body.contains("## Failure / Stop State"));
    }

    #[test]
    fn metadata_poor_mr_body_falls_back_to_legacy_template() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );

        let validation_commands = Vec::new();
        let backend_summary = "Fixed the issue.";
        let ctx = MrRenderContext {
            backend: "codex",
            model: "gpt-5.4",
            branch: "gah/repo-123",
            target_branch: "main",
            validation_commands: &validation_commands,
            ledger: &ledger,
            backend_summary,
        };
        let body = build_fix_or_improve_mr_body("fix", None, &ctx, true);

        assert!(body.contains("## GAH fix mode"));
        assert!(body.contains("Ticket: n/a"));
        assert!(body.contains("Validation passed: true"));
        assert!(!body.contains("## Work Item"));
    }

    #[test]
    fn metadata_rich_mr_body_includes_closes_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );

        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-72".to_string()),
            work_id: Some("TICKET-72".to_string()),
            title: Some("Test Issue".to_string()),
            issue_number: Some("72".to_string()),
            ..TicketMetadata::default()
        };

        let ctx = MrRenderContext {
            backend: "test",
            model: "test-model",
            branch: "gah/test-123",
            target_branch: "main",
            validation_commands: &[],
            ledger: &ledger,
            backend_summary: "Test summary",
        };

        let body = build_metadata_rich_mr_body("fix", &ticket, &ctx);

        // Verify that the Closes directive is included
        assert!(
            body.contains("Closes #72"),
            "MR body should contain 'Closes #72'"
        );

        // Verify it's not at the very beginning or end (should be after Work Item section)
        assert!(
            !body.starts_with("Closes #72"),
            "Closes directive should not be at the start"
        );
    }

    #[test]
    fn standard_mr_body_includes_closes_directive() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-72".to_string()),
            work_id: Some("TICKET-72".to_string()),
            title: Some("Test Issue".to_string()),
            issue_number: Some("72".to_string()),
            ..TicketMetadata::default()
        };

        let body = build_standard_mr_body(
            "fix",
            Some(&ticket),
            "test",
            "test-model",
            "branch",
            "main",
            true,
            "Test summary",
        );

        // Verify that the Closes directive is included
        assert!(
            body.contains("Closes #72"),
            "Standard MR body should contain 'Closes #72'"
        );
    }

    #[test]
    fn mr_body_no_closes_directive_without_issue_number() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-72".to_string()),
            work_id: Some("TICKET-72".to_string()),
            title: Some("Test Issue".to_string()),
            issue_number: None, // No issue number
            ..TicketMetadata::default()
        };

        let body = build_standard_mr_body(
            "fix",
            Some(&ticket),
            "test",
            "test-model",
            "branch",
            "main",
            true,
            "Test summary",
        );

        // Verify that the Closes directive is NOT included when there's no issue number
        assert!(
            !body.contains("Closes #"),
            "Standard MR body should not contain Closes directive without issue number"
        );
    }

    #[test]
    fn metadata_rich_mr_body_no_closes_directive_without_issue_number() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );

        let ticket = TicketMetadata {
            ticket_id: None,
            work_id: None,
            title: Some("Test Issue".to_string()),
            issue_number: None, // No issue number
            ..TicketMetadata::default()
        };

        let ctx = MrRenderContext {
            backend: "test",
            model: "test-model",
            branch: "gah/test-123",
            target_branch: "main",
            validation_commands: &[],
            ledger: &ledger,
            backend_summary: "Test summary",
        };

        let body = build_metadata_rich_mr_body("fix", &ticket, &ctx);

        // Verify that the Closes directive is NOT included when there's no issue number
        assert!(
            !body.contains("Closes #"),
            "MR body should not contain Closes directive without issue number"
        );
    }
}

fn git_output(args: &[&str], cwd: &Path) -> Result<String> {
    worktree::git(args, cwd)
}

fn apply_diff_stats(ledger: &mut LedgerEntry, wt: &Path, target_branch: &str) {
    if let Ok(stats) = worktree::diff_stats(wt, target_branch) {
        ledger.files_changed = Some(stats.files_changed);
        ledger.insertions = Some(stats.insertions);
        ledger.deletions = Some(stats.deletions);
    }
}

fn summarize_error(err: &anyhow::Error) -> String {
    let text = format!("{:#}", err).replace('\n', " ");
    if text.len() > 500 {
        let safe_text = utf8_safe_prefix(&text, 497).to_string();
        format!("{safe_text}...")
    } else {
        text
    }
}

fn dry_run_route(
    cfg: &GahConfig,
    profile: &Profile,
    mode: &str,
    args: &DispatchArgs,
) -> Option<RouteDecision> {
    let ticket_meta = if matches!(mode, "improve" | "fix") && !args.target.is_empty() {
        parse_ticket_metadata(Path::new(&args.target))
            .ok()
            .flatten()
    } else {
        None
    };
    let mut dry_ledger = LedgerEntry::new(
        &args.profile,
        profile,
        &args.backend,
        mode,
        &args.target,
        None,
        None,
    );
    dry_ledger.work_id = ticket_meta
        .as_ref()
        .and_then(|meta| meta.work_id.clone().or_else(|| meta.ticket_id.clone()));
    let runtime = routing_runtime_state(cfg, &dry_ledger).unwrap_or_default();
    routing::decide_for_task_with_state(
        &cfg.defaults,
        profile,
        RouteRequest {
            last_failure_class: None,
            mode,
            requested_backend: config::canonical_backend_name(&args.backend),
            requested_model: args.model.as_deref(),
            recommended_backend: ticket_meta
                .as_ref()
                .and_then(|m| m.recommended_backend.as_deref()),
            recommended_model: ticket_meta
                .as_ref()
                .and_then(|m| m.recommended_model.as_deref()),
            session_id: None,
            usage_summary: None,
        },
        TaskRoutingContext {
            task_class: ticket_meta
                .as_ref()
                .and_then(|meta| meta.task_class.as_deref()),
            difficulty: ticket_meta
                .as_ref()
                .and_then(|meta| meta.difficulty.as_deref()),
            risk: ticket_meta.as_ref().and_then(|meta| meta.risk.as_deref()),
        },
        &runtime,
    )
    .ok()
}

fn resolve_review_target(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
) -> Result<ReviewTarget> {
    if let Some(mr) = args.mr.as_deref() {
        let mr_target = provider::find_review_target_by_mr(profile, mr)?;
        return Ok(ReviewTarget {
            mr_id: Some(mr_target.id),
            mr_url: Some(mr_target.url),
            mr_title: mr_target.title,
            mr_body: mr_target.body,
            ci_status: mr_target.ci_status,
            source_sha: mr_target.source_sha,
            target_sha: mr_target.target_sha,
            source_branch: mr_target.source_branch.clone(),
            target_branch: fallback_target_branch(
                &profile.default_target_branch,
                Some(&mr_target.target_branch),
            ),
            prior_state: lookup_review_state_by_branch(
                cfg,
                &args.profile,
                &mr_target.source_branch,
            ),
        });
    }

    if let Some(branch) = args.branch.as_deref() {
        return review_target_from_branch(profile, branch);
    }

    if !args.target.is_empty() {
        let target_path = Path::new(&args.target);
        if let Some(ticket) = parse_ticket_metadata(target_path)? {
            if let Some(state) =
                lookup_review_state(cfg, profile, &args.profile, &args.target, &ticket)
            {
                return Ok(state);
            }
        } else {
            return review_target_from_branch(profile, &args.target);
        }
    }

    if args.current_branch {
        let repo = Path::new(&profile.local_path);
        let branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"], repo)?;
        return review_target_from_branch(profile, &branch);
    }

    anyhow::bail!(
        "review target required: pass --mr, --branch, a ticket path in --target, or --current-branch"
    )
}

fn review_target_from_branch(profile: &Profile, branch: &str) -> Result<ReviewTarget> {
    match provider::find_review_target_by_branch(profile, branch) {
        Ok(mr_target) => Ok(ReviewTarget {
            mr_id: Some(mr_target.id),
            mr_url: Some(mr_target.url),
            source_branch: if mr_target.source_branch.is_empty() {
                branch.to_string()
            } else {
                mr_target.source_branch
            },
            target_branch: fallback_target_branch(
                &profile.default_target_branch,
                Some(&mr_target.target_branch),
            ),
            mr_title: mr_target.title,
            mr_body: mr_target.body,
            ci_status: mr_target.ci_status,
            source_sha: mr_target.source_sha,
            target_sha: mr_target.target_sha,
            prior_state: None,
        }),
        Err(_) => Ok(ReviewTarget {
            mr_id: None,
            mr_url: None,
            mr_title: None,
            mr_body: None,
            ci_status: None,
            source_sha: None,
            target_sha: None,
            source_branch: branch.to_string(),
            target_branch: profile.default_target_branch.clone(),
            prior_state: None,
        }),
    }
}

fn fallback_target_branch(default_branch: &str, provider_target: Option<&str>) -> String {
    provider_target
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_branch)
        .to_string()
}

fn lookup_review_state(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    target: &str,
    ticket: &TicketMetadata,
) -> Option<ReviewTarget> {
    let entries = ledger::read_entries(cfg).ok()?;
    let ticket_id = ticket.ticket_id.as_deref();
    let ticket_title = ticket.title.as_deref().map(normalize_match);
    entries
        .into_iter()
        .rev()
        .find(|entry| {
            entry.profile == profile_name
                && matches!(entry.mode.as_str(), "fix" | "improve")
                && entry.branch.is_some()
                && entry.error_summary.is_none()
                && (entry.target_summary.as_deref() == Some(target)
                    || ticket_id
                        .map(|id| entry.target_summary.as_deref().unwrap_or("").contains(id))
                        .unwrap_or(false)
                    || ticket_title
                        .as_ref()
                        .map(|title| {
                            normalize_match(entry.target_summary.as_deref().unwrap_or(""))
                                .contains(title)
                        })
                        .unwrap_or(false))
        })
        .map(|entry| ReviewTarget {
            mr_id: entry
                .mr_url
                .as_deref()
                .and_then(|url| url.rsplit('/').next())
                .map(str::to_string),
            mr_url: entry.mr_url.clone(),
            mr_title: None,
            mr_body: None,
            ci_status: None,
            source_sha: None,
            target_sha: None,
            source_branch: entry.branch.clone().unwrap_or_default(),
            target_branch: profile.default_target_branch.clone(),
            prior_state: Some(render_prior_ledger_state(&entry)),
        })
}

fn lookup_review_state_by_branch(
    cfg: &GahConfig,
    profile_name: &str,
    branch: &str,
) -> Option<String> {
    let entries = ledger::read_entries(cfg).ok()?;
    entries
        .into_iter()
        .rev()
        .find(|entry| {
            entry.profile == profile_name
                && matches!(entry.mode.as_str(), "fix" | "improve")
                && entry.branch.as_deref() == Some(branch)
        })
        .map(|entry| render_prior_ledger_state(&entry))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewBudgetBlock {
    reason: String,
}

/// Return a deterministic ticket-scoped review budget block before a reviewer
/// is launched. A cycle is a prior review dispatch that consumed a real
/// reviewer call; it includes failed and timed-out reviews because those can
/// still consume quota, but excludes both a prior budget refusal and a
/// duplicate-review short-circuit (same source SHA/tier already reviewed),
/// since neither launched a reviewer. Paid usage is counted only from an
/// explicit recorded `api_key_backed` classification, never inferred from a
/// provider name or silently from unknown data. The paid cap applies only
/// when routing has explicitly selected a candidate configured as paid;
/// quota-backed, local, and unknown-cost routes remain eligible until the
/// cycle cap is reached.
fn check_review_budget(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    work_id: Option<&str>,
    route: &RouteDecision,
) -> Result<Option<ReviewBudgetBlock>> {
    // Direct branch/MR reviews without a controller-provided ticket identity
    // cannot be attributed safely to a per-ticket budget. Fail open rather
    // than accidentally merging unrelated branches into one accounting bucket.
    let Some(work_id) = work_id.filter(|id| !id.trim().is_empty()) else {
        return Ok(None);
    };
    let routing = profile.effective_routing(&cfg.defaults);
    let entries = ledger::entries_for_work_id(cfg, work_id)?;
    let reviews: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.mode == "review"
                && !matches!(
                    entry.validation_result.as_deref(),
                    Some("review_budget_exhausted") | Some("skipped_duplicate_review")
                )
        })
        .collect();

    let cycle_count = reviews.len() as u32;
    let cycle_cap = routing.max_review_cycles_per_ticket();
    if cycle_count >= cycle_cap {
        return Ok(Some(ReviewBudgetBlock {
            reason: format!(
                "review budget exhausted for {work_id}: {cycle_count}/{cycle_cap} review cycles used"
            ),
        }));
    }

    let selected_paid = route
        .routing_diagnostics
        .as_ref()
        .and_then(|diagnostics| diagnostics.selected_cost_class.as_deref())
        == Some("paid");
    if selected_paid {
        let paid_count = reviews
            .iter()
            .filter(|entry| entry.usage.usage_classification.as_deref() == Some("api_key_backed"))
            .count() as u32;
        let paid_cap = routing.max_paid_reviews_per_ticket();
        if paid_count >= paid_cap {
            return Ok(Some(ReviewBudgetBlock {
                reason: format!(
                    "paid review budget exhausted for {work_id}: {paid_count}/{paid_cap} API-backed reviews used"
                ),
            }));
        }
    }

    Ok(None)
}

/// The routine reviewer (`review_backend`, e.g. Vibe/Mistral) is fast and
/// cheap but was never meant to be the last word on a genuinely hard or
/// repeatedly-failing review. The repeated-failure trigger follows the
/// configured post-review repair budget; adds an
/// immediate-escalate path for a reviewer that itself reported low
/// confidence, since forcing 2 low-confidence rubber stamps before getting
/// a second opinion defeats the point of tracking confidence at all.
///
/// Reads `validation_result`/`confidence_impact` off this branch's own
/// `mode == "review"` entries -- NOT `review_verdict`/`review_confidence`.
/// Those two fields are written by `backfill_review_verdict` (ledger.rs,
/// TICKET-125) onto the *implementation* (fix/improve) entry instead, by
/// design (see `backfill_review_verdict_attributes_to_implementation_entry_not_reviewer`).
/// A review dispatch's own entry never carries a `review_verdict`, so
/// checking that field here would make this permanently a no-op; the
/// verdict/confidence a review entry actually records about itself live in
/// `validation_result`/`confidence_impact` (set directly in `review()`).
fn review_escalation_reason(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    branch: &str,
) -> Option<&'static str> {
    let repeated_failure_threshold = profile
        .effective_routing(&cfg.defaults)
        .max_fix_attempts_per_mr() as usize;

    let entries = ledger::read_entries(cfg).ok()?;
    let recent: Vec<&LedgerEntry> = entries
        .iter()
        .rev()
        .filter(|e| {
            e.profile == profile_name && e.mode == "review" && e.branch.as_deref() == Some(branch)
        })
        .take(repeated_failure_threshold)
        .collect();

    // A real HUMAN_REVIEW verdict and a deterministic evidence-gate hold both
    // use this persisted result. Neither is a reason to abandon automation
    // while a configured second-opinion reviewer remains.
    if recent
        .first()
        .is_some_and(|e| e.validation_result.as_deref() == Some("HUMAN_REVIEW"))
    {
        return Some("human_review");
    }

    if recent
        .first()
        .is_some_and(|e| e.confidence_impact.as_deref() == Some("low"))
    {
        return Some("low_confidence");
    }

    if recent.len() == repeated_failure_threshold
        && recent.iter().all(|e| {
            matches!(
                e.validation_result.as_deref(),
                Some("NEEDS_FIX") | Some("REJECT")
            )
        })
    {
        return Some("repeated_needs_fix");
    }

    None
}

/// Select the next unused reviewer from the explicitly ordered escalation
/// chain. The identity includes both backend instance and model: AGY account
/// 1, AGY account 2, and a paid gateway must remain independently observable
/// and independently eligible for a second opinion.
fn next_escalatory_reviewer(
    cfg: &GahConfig,
    profile: &Profile,
    profile_name: &str,
    branch: &str,
    current: Option<(&str, Option<&str>)>,
) -> Option<CandidateConfig> {
    let mut attempted: HashSet<(String, Option<String>)> = ledger::read_entries(cfg)
        .ok()?
        .into_iter()
        .filter(|entry| {
            entry.profile == profile_name
                && entry.mode == "review"
                && entry.branch.as_deref() == Some(branch)
                && entry.validation_result.as_deref() != Some("skipped_duplicate_review")
        })
        .map(|entry| (entry.effective_backend, entry.effective_model))
        .collect();
    if let Some((backend, model)) = current {
        attempted.insert((backend.to_string(), model.map(str::to_string)));
    }

    profile
        .effective_routing(&cfg.defaults)
        .effective_escalatory_reviewers()
        .into_iter()
        .find(|candidate| {
            // A candidate left without an explicit model is recorded in the
            // ledger under whatever effective model routing backfilled for it
            // (e.g. codex's config-file default, mirroring routing.rs's own
            // decide_route backfill) -- compare against that, not the raw
            // config value, or a once-tried backfilled candidate looks
            // perpetually untried and the chain never advances past it.
            let effective_model = if candidate.backend == "codex" && candidate.model.is_none() {
                crate::runner::extract_model_from_args(&profile.codex_args)
            } else {
                candidate.model.clone()
            };
            !attempted.contains(&(candidate.backend.clone(), effective_model))
        })
}

/// Review deduplication normally works at the authority-tier level. An
/// ordered escalation chain deliberately contains several distinct second
/// opinions, so each escalatory backend/model pair gets one review of a
/// source commit rather than the first escalatory reviewer suppressing every
/// later one.
fn reviewer_dedup_class(tier: ReviewerTier, route: &RouteDecision) -> String {
    match tier {
        ReviewerTier::Escalatory => format!(
            "escalatory:{}/{}",
            route.effective_backend,
            route.effective_model.as_deref().unwrap_or("default")
        ),
        _ => tier.as_str().to_string(),
    }
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
    ledger.error_summary = Some(message.clone());
    notify_event(
        cfg,
        profile,
        NotifyEvent::HumanRequired {
            reason: "review escalation exhausted",
            reference: target.mr_url.as_deref(),
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

fn render_prior_ledger_state(entry: &LedgerEntry) -> String {
    format!(
        "Mode: {}\nRequested backend/model: {} / {}\nEffective backend/model: {} / {}\nValidation result: {}\nMR: {}\nSession: {}",
        entry.mode,
        entry.requested_backend,
        entry.requested_model.as_deref().unwrap_or("unknown"),
        entry.effective_backend,
        entry.effective_model.as_deref().unwrap_or("unknown"),
        entry.validation_result.as_deref().unwrap_or("unknown"),
        entry.mr_url.as_deref().unwrap_or("n/a"),
        entry.session_dir.as_deref().unwrap_or("n/a"),
    )
}

fn prepare_review_diff(
    repo: &Path,
    _profile: &Profile,
    target: &ReviewTarget,
) -> Result<ReviewDiffBundle> {
    git_output(&["fetch", "-q", "origin", "--prune"], repo)?;
    git_output(
        &[
            "fetch",
            "-q",
            "origin",
            &format!(
                "{}:refs/remotes/origin/{}",
                target.target_branch, target.target_branch
            ),
        ],
        repo,
    )?;
    git_output(
        &[
            "fetch",
            "-q",
            "origin",
            &format!(
                "{}:refs/remotes/origin/{}",
                target.source_branch, target.source_branch
            ),
        ],
        repo,
    )?;

    let target_ref = format!("origin/{}", target.target_branch);
    let source_ref = format!("origin/{}", target.source_branch);
    let diff = git_output(&["diff", &format!("{target_ref}...{source_ref}")], repo)?;
    let files = git_output(
        &[
            "diff",
            "--name-only",
            &format!("{target_ref}...{source_ref}"),
        ],
        repo,
    )?;
    if diff.trim().is_empty() {
        anyhow::bail!(empty_review_diff_diagnostics(
            repo,
            target,
            &target_ref,
            &source_ref
        ));
    }
    Ok(ReviewDiffBundle { diff, files })
}

fn empty_review_diff_diagnostics(
    repo: &Path,
    target: &ReviewTarget,
    target_ref: &str,
    source_ref: &str,
) -> String {
    let current_branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"], repo)
        .unwrap_or_else(|e| format!("(error: {e:#})"));
    let target_sha =
        git_output(&["rev-parse", target_ref], repo).unwrap_or_else(|e| format!("(error: {e:#})"));
    let source_sha =
        git_output(&["rev-parse", source_ref], repo).unwrap_or_else(|e| format!("(error: {e:#})"));
    let diff_stat = git_output(
        &["diff", "--stat", &format!("{target_ref}...{source_ref}")],
        repo,
    )
    .unwrap_or_else(|e| format!("(error: {e:#})"));
    format!(
        "empty review diff\nprofile.local_path: {}\ncurrent branch: {}\nsource branch: {}\ntarget branch: {}\nfetched refs: {}, {}\ngit rev-parse target: {}\ngit rev-parse source: {}\ngit diff --stat:\n{}\nsuggestion: fetch the source branch or pass --branch/--mr for the open review target explicitly",
        repo.display(),
        current_branch,
        target.source_branch,
        target.target_branch,
        source_ref,
        target_ref,
        target_sha,
        source_sha,
        diff_stat,
    )
}

#[derive(Debug, Clone)]
struct ReviewTarget {
    mr_id: Option<String>,
    mr_url: Option<String>,
    mr_title: Option<String>,
    mr_body: Option<String>,
    ci_status: Option<String>,
    source_sha: Option<String>,
    target_sha: Option<String>,
    source_branch: String,
    target_branch: String,
    prior_state: Option<String>,
}

#[derive(Debug, Clone)]
struct ReviewDiffBundle {
    diff: String,
    files: String,
}

/// Facts supplied by the control plane, not the reviewer. An approval must
/// cite these exact facts; free-form reviewer claims alone never make a change
/// safe to merge.
#[derive(Debug, Clone, Default)]
struct ReviewGateContext {
    changed_files: Vec<String>,
    ci_passed: bool,
    contract_files: Vec<String>,
    compatibility_mechanisms: Vec<&'static str>,
    enforce_grounding: bool,
}

impl ReviewGateContext {
    fn from_diff_bundle(bundle: &ReviewDiffBundle, ci_status: Option<&str>) -> Self {
        let changed_files: Vec<String> = bundle
            .files
            .lines()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect();
        let diff_lower = bundle.diff.to_ascii_lowercase();
        let public_api_change = bundle.diff.lines().any(|line| {
            let line = line.trim_start_matches(['+', '-']);
            line.trim_start().starts_with("pub struct ")
                || line.trim_start().starts_with("pub enum ")
                || line.trim_start().starts_with("pub type ")
                || line.trim_start().starts_with("pub fn ")
        });
        let contract_files: Vec<String> = changed_files
            .iter()
            .filter(|path| {
                path.starts_with("packages/contracts/")
                    || path.starts_with("src/telemetry/")
                    || path == &"src/ledger.rs"
                    || path.starts_with("migrations/")
                    || path.contains("/api/")
                    || path.starts_with("apps/server/src/")
                    || (public_api_change && path.starts_with("src/"))
            })
            .cloned()
            .collect();
        let mut compatibility_mechanisms = Vec::new();
        if diff_lower.contains("schema_version") {
            compatibility_mechanisms.push("schema-version");
        }
        if diff_lower.contains("serde(default)") {
            compatibility_mechanisms.push("backward-compatible-default");
        }
        if diff_lower.contains("migrat") {
            compatibility_mechanisms.push("migration");
        }

        Self {
            changed_files,
            ci_passed: ci_status.is_some_and(|status| {
                matches!(
                    status.trim().to_ascii_lowercase().as_str(),
                    "passed" | "success" | "green"
                )
            }),
            contract_files,
            compatibility_mechanisms,
            enforce_grounding: true,
        }
    }

    fn has_contract_surface_change(&self) -> bool {
        !self.contract_files.is_empty()
    }

    fn evidence_is_grounded(&self, evidence: &[String]) -> bool {
        evidence.iter().any(|item| {
            let Some(path) = item.trim().strip_prefix("file:") else {
                return false;
            };
            self.changed_files
                .iter()
                .any(|candidate| candidate == path.trim())
        })
    }

    fn falsely_claims_passed_ci(&self, evidence: &[String]) -> bool {
        !self.ci_passed
            && evidence
                .iter()
                .any(|item| item.trim().eq_ignore_ascii_case("ci:passed"))
    }

    fn compatibility_is_grounded(&self, evidence: &[String]) -> bool {
        evidence.iter().any(|item| {
            let Some(path) = item.trim().strip_prefix("file:") else {
                return false;
            };
            self.contract_files
                .iter()
                .any(|candidate| candidate == path.trim())
        }) && evidence.iter().any(|item| {
            let Some(mechanism) = item.trim().strip_prefix("mechanism:") else {
                return false;
            };
            self.compatibility_mechanisms
                .iter()
                .any(|candidate| candidate == &mechanism.trim())
        })
    }
}

#[allow(clippy::too_many_arguments)]
/// TICKET-108: reviewer authority (who is reviewing) kept as a dimension
/// separate from review outcome (verdict/confidence, what they said).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewerTier {
    Strong,
    Standard,
    Weak,
    /// Issue #123: an escalatory reviewer (a more-capable model from the
    /// ESCALATORY_REVIEW list) the pipeline escalated to and continued with.
    /// Auto-merge eligible like `Strong`, but recorded distinctly so the
    /// cascade origin is observable.
    Escalatory,
}

impl ReviewerTier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Standard => "standard",
            Self::Weak => "weak",
            Self::Escalatory => "escalatory",
        }
    }
}

/// Derived from which configured routing field actually selected this
/// backend/model, not from anything the reviewer says about itself -- a
/// weak reviewer cannot self-promote by returning confident-sounding text
/// (TICKET-108's core requirement).
fn derive_reviewer_tier(cfg: &GahConfig, profile: &Profile, route: &RouteDecision) -> ReviewerTier {
    let effective_model = route.effective_model.as_deref();
    let selected = |backend_cfg: Option<&str>, model_cfg: Option<&str>| -> bool {
        backend_cfg.is_some_and(|b| b == route.effective_backend)
            && (model_cfg.is_none() || model_cfg == effective_model)
    };
    let routine = profile
        .routing
        .effective_routine_reviewer()
        .or_else(|| cfg.defaults.routing.effective_routine_reviewer());
    let escalatory = profile
        .routing
        .escalatory_reviewers
        .iter()
        .cloned()
        .chain(cfg.defaults.routing.escalatory_reviewers.clone())
        .collect::<Vec<_>>();

    // Issue #233: tier classification must only honor explicitly declared
    // escalatory reviewers. The legacy weak-review keys still feed routing
    // backfill via `effective_escalatory_reviewers()`, but they do not imply
    // the auto-merge-eligible escalatory tier.
    for esc in &escalatory {
        if selected(Some(esc.backend.as_str()), esc.model.as_deref()) {
            // Check if this escalatory reviewer is actually a legacy weak review configuration
            // Legacy weak review configs should be treated as Weak tier, not Escalatory
            let is_legacy_weak_config = profile.routing.escalatory_reviewers.is_empty()
                && profile.routing.weak_review_backend.as_deref() == Some(esc.backend.as_str())
                && profile.routing.weak_review_model.as_deref() == esc.model.as_deref();

            if is_legacy_weak_config {
                return ReviewerTier::Weak;
            }
            return ReviewerTier::Escalatory;
        }
    }
    // Routine reviewer is the STRONG first-line authority.
    if let Some(routine) = &routine {
        if selected(Some(routine.backend.as_str()), routine.model.as_deref()) {
            return ReviewerTier::Strong;
        }
    }
    let strong_backend = profile.routing.strong_review_backend.as_deref().or(cfg
        .defaults
        .routing
        .strong_review_backend
        .as_deref());
    let strong_model = profile.routing.strong_review_model.as_deref().or(cfg
        .defaults
        .routing
        .strong_review_model
        .as_deref());
    let weak_backend = profile.routing.weak_review_backend.as_deref().or(cfg
        .defaults
        .routing
        .weak_review_backend
        .as_deref());
    let weak_model = profile.routing.weak_review_model.as_deref().or(cfg
        .defaults
        .routing
        .weak_review_model
        .as_deref());

    if selected(weak_backend, weak_model) {
        return ReviewerTier::Weak;
    }
    if selected(strong_backend, strong_model) {
        return ReviewerTier::Strong;
    }
    // review_candidates is the operator's actual declared pool of reviewers
    // they consider trustworthy (agy/agy-second/claude serving the same
    // Sonnet-class model are routinely interchangeable fallbacks for each
    // other, not different capability tiers). Requiring strong_review_backend/
    // model to be manually kept in sync with every review_candidates entry
    // is exactly the kind of drift that already produced two real bugs
    // tonight (gah's own strong_review_backend pointed at codex-mini; here,
    // falling back from agy to agy-second/claude silently downgraded a
    // Sonnet reviewer to "standard" tier). Any candidate not already
    // classified weak above is strong.
    let candidates = profile.routing.review_candidates.as_ref().or(cfg
        .defaults
        .routing
        .review_candidates
        .as_ref());
    if let Some(candidates) = candidates {
        let in_candidates = candidates.iter().any(|c| {
            c.backend == route.effective_backend
                && (c.model.is_none() || c.model.as_deref() == effective_model)
        });
        if in_candidates {
            return ReviewerTier::Strong;
        }
    }
    ReviewerTier::Standard
}

#[cfg(test)]
fn parse_review_verdict(
    review_text: &str,
    route: &RouteDecision,
    parsed_usage: &crate::ledger::LedgerUsage,
    tier: ReviewerTier,
) -> Result<crate::models::ReviewVerdict> {
    parse_review_verdict_with_context(
        review_text,
        route,
        parsed_usage,
        tier,
        &ReviewGateContext::default(),
    )
}

fn parse_review_verdict_with_context(
    review_text: &str,
    route: &RouteDecision,
    parsed_usage: &crate::ledger::LedgerUsage,
    tier: ReviewerTier,
    gate_context: &ReviewGateContext,
) -> Result<crate::models::ReviewVerdict> {
    let json = extract_first_json_object(review_text)
        .ok_or_else(|| anyhow::anyhow!("reviewer did not return verdict JSON"))?;
    let mut verdict = serde_json::from_str::<crate::models::ReviewVerdict>(&json)?;
    enforce_review_evidence_gate(
        &mut verdict,
        review_text,
        &route.effective_backend,
        gate_context,
    );
    // Reviewer identity (tier) and review outcome (verdict text/confidence)
    // are separate dimensions -- the verdict text itself is never rewritten
    // based on who reviewed it (see review_labels for how tier affects
    // labeling instead).
    if tier == ReviewerTier::Weak && verdict.confidence == "high" {
        // Weak approval is deliberately not auto-merge authority. A weak
        // reviewer finding a defect is actionable input for the normal
        // post-review repair budget and must not skip straight to a human.
        verdict.confidence = "medium".into();
    }
    if tier == ReviewerTier::Weak && verdict.verdict == "APPROVE" {
        verdict.human_required = true;
    }
    if verdict.verdict == "HUMAN_REVIEW"
        || (verdict.verdict == "APPROVE" && verdict.confidence == "low")
    {
        verdict.human_required = true;
    }
    verdict.reviewer_tier = Some(tier.as_str().to_string());
    verdict.reviewer_backend = Some(route.effective_backend.clone());
    verdict.reviewer_model = route.effective_model.clone();
    verdict.requested_backend = Some(route.requested_backend.clone());
    verdict.effective_backend = Some(route.effective_backend.clone());
    verdict.requested_model = route.requested_model.clone();
    verdict.effective_model = route.effective_model.clone();
    verdict.fallback_used = Some(route.fallback_used);
    verdict.usage_source = parsed_usage.usage_source.clone();
    verdict.input_tokens = parsed_usage.input_tokens;
    verdict.output_tokens = parsed_usage.output_tokens;
    verdict.total_tokens = parsed_usage.total_tokens;
    verdict.estimated_cost_usd = parsed_usage.estimated_cost_usd;
    verdict.actual_cost_usd = parsed_usage.actual_cost_usd;
    Ok(verdict)
}

/// A reviewer is advisory; merge safety is deterministic. In particular, an
/// LLM must not be able to write an apparent APPROVE while its own structured
/// findings describe a blocking or unversioned contract change (the exact
/// failure observed in PR #284). The normalized verdict remains visible in
/// the review artifact, ledger, and status payload.
fn enforce_review_evidence_gate(
    verdict: &mut crate::models::ReviewVerdict,
    review_text: &str,
    reviewer_backend: &str,
    gate_context: &ReviewGateContext,
) {
    if verdict.verdict != "APPROVE" {
        return;
    }

    let reason = if !verdict.blocking_findings.is_empty() {
        Some("APPROVE contradicted non-empty blocking_findings".to_string())
    } else if review_text_has_substantive_prose(review_text, reviewer_backend) {
        Some(
            "APPROVE included substantive prose; every finding must be represented in the review JSON"
                .to_string(),
        )
    } else if verdict.evidence.is_empty() {
        Some("APPROVE omitted required concrete review evidence".to_string())
    } else if gate_context.enforce_grounding
        && gate_context.falsely_claims_passed_ci(&verdict.evidence)
    {
        Some("APPROVE claimed passed CI while the control plane did not report it".to_string())
    } else if gate_context.enforce_grounding
        && !gate_context.evidence_is_grounded(&verdict.evidence)
    {
        Some(
            "APPROVE evidence was not grounded in an exact changed file from the control plane"
                .to_string(),
        )
    } else if gate_context.has_contract_surface_change()
        && (gate_context.compatibility_mechanisms.is_empty()
            || !gate_context.compatibility_is_grounded(&verdict.compatibility_evidence))
    {
        Some(
            "APPROVE changed a contract surface without a control-plane-verifiable compatibility mechanism and evidence"
                .to_string(),
        )
    } else {
        None
    };

    let Some(reason) = reason else {
        return;
    };

    verdict.verdict = "HUMAN_REVIEW".to_string();
    verdict.human_required = true;
    verdict.safety_gate_reason = Some(reason);
}

fn review_text_has_substantive_prose(review_text: &str, reviewer_backend: &str) -> bool {
    let Some(json) = extract_first_json_object(review_text) else {
        return true;
    };
    let Some(start) = review_text.find(&json) else {
        return true;
    };
    let mut residue = String::with_capacity(review_text.len().saturating_sub(json.len()));
    residue.push_str(&review_text[..start]);
    residue.push_str(&review_text[start + json.len()..]);
    let agy_transport_trace = matches!(reviewer_backend, "agy" | "agy-second");
    residue.lines().map(str::trim).any(|line| {
        // `agy --print` writes its execution-plan trace to stdout before
        // the final answer. Those uniform "I will ..." lines are runner
        // transport metadata, not reviewer prose. Preserve fail-closed
        // behavior for every other line, including AGY's final prose.
        let inert = line.is_empty()
            || (agy_transport_trace && line.starts_with("I will "))
            || matches!(
                line.to_ascii_lowercase().trim_end_matches(':').trim(),
                "review notes" | "## review notes" | "### review notes" | "```json" | "```"
            );
        !inert
    })
}

fn count_test_files(profile: &Profile, root: &Path) -> usize {
    let patterns = if profile.test_file_patterns.is_empty() {
        vec![
            "test_*.py".to_string(),
            "*_test.py".to_string(),
            "*.test.ts".to_string(),
            "*.test.js".to_string(),
            "*.spec.ts".to_string(),
            "*.spec.js".to_string(),
            "*_test.rs".to_string(),
            "tests/*.rs".to_string(),
            "*_test.go".to_string(),
            "*Test.java".to_string(),
            "*_spec.rb".to_string(),
            "*Tests.cs".to_string(),
        ]
    } else {
        profile.test_file_patterns.clone()
    };
    count_files_matching(root, root, &|name: &str| {
        patterns.iter().any(|pat| {
            let re = format!(
                "^{}$",
                pat.replace(".", r"\.").replace("*", ".*").replace("?", ".")
            );
            regex::Regex::new(&re)
                .map(|r| r.is_match(name))
                .unwrap_or(false)
        })
    })
}

fn count_files_matching(root: &Path, dir: &Path, pred: &dyn Fn(&str) -> bool) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches!(
                name,
                "target" | ".git" | "node_modules" | "__pycache__" | ".venv"
            ) {
                count += count_files_matching(root, &path, pred);
            }
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if pred(&rel.to_string_lossy()) {
                count += 1;
            }
        }
    }
    count
}

fn which(cmd: &str) -> Option<String> {
    Command::new("which")
        .arg(cmd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{seconds}-{}", uuid::Uuid::new_v4().simple())
}
