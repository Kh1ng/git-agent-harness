use crate::config::{self, GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::notifications::{notify_event, NotifyEvent};
use crate::routing::{self, RouteDecision, RouteRequest, TaskRoutingContext};
use crate::usage_attribution::{aggregate_attempt_usage, usage_has_observation};
use crate::{runner, worktree};
use anyhow::{bail, Context, Result};
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
mod review;
mod text;
mod validation;
mod workflows;

#[cfg(test)]
mod test_util;

use self::issues::{
    issue_is_auto_dispatch_blocked, list_open_issues, parse_ticket_metadata,
    parse_ticket_metadata_from_issue, ticket_number_prefix,
};

pub use self::review::policy::review_budget_exhausted_error;

pub use self::attempts::review_preflight;
use self::attempts::{ensure_bin, routing_runtime_state};

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
        "review" => workflows::run_review(cfg, profile, args, &session_dir, &mut ledger),
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

#[cfg(test)]
mod tests {
    use super::apply_diff_stats;
    use super::issues::TicketMetadata;
    use super::publish::{
        build_fix_or_improve_mr_body, build_metadata_rich_mr_body, build_mr_title,
        build_standard_mr_body, render_review_comment, MrRenderContext,
    };
    use super::test_util::{init_repo, profile};
    use crate::ledger::LedgerEntry;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

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
