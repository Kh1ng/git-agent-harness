use crate::config::{self, GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::models::CandidateArtifact;
use crate::models::{AvailableTicket, PmPlan, WorkMetadata};
use crate::notifications::{notify_event, NotifyEvent};
use crate::provider::provider_command;
use crate::routing::{self, RouteDecision, RouteError, RouteRequest};
use crate::{provider, runner, usage, worktree};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

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
fn utf8_safe_prefix(s: &str, max_bytes: usize) -> &str {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationFailureProgress {
    Changed,
    UnchangedFromBaseline,
    UnchangedFromPreviousAttempt,
    UnchangedFromBaselineAndPreviousAttempt,
}

impl ValidationFailureProgress {
    fn unchanged_from_baseline(self) -> bool {
        matches!(
            self,
            Self::UnchangedFromBaseline | Self::UnchangedFromBaselineAndPreviousAttempt
        )
    }

    fn unchanged_from_previous_attempt(self) -> bool {
        matches!(
            self,
            Self::UnchangedFromPreviousAttempt | Self::UnchangedFromBaselineAndPreviousAttempt
        )
    }
}

fn validation_failure_no_progress_reason(
    progress: ValidationFailureProgress,
) -> Option<&'static str> {
    match progress {
        ValidationFailureProgress::Changed => None,
        ValidationFailureProgress::UnchangedFromBaseline => Some(
            "validation failure identical to the pristine-tree baseline — the agent's changes never affected this error. Fix the validation command or environment, not the ticket.",
        ),
        ValidationFailureProgress::UnchangedFromPreviousAttempt => Some(
            "validation failure identical to the previous attempt — the agent made no progress on the failing check.",
        ),
        ValidationFailureProgress::UnchangedFromBaselineAndPreviousAttempt => Some(
            "validation failure identical to both the pristine-tree baseline and the previous attempt — the agent made no progress and never affected the original error.",
        ),
    }
}

fn is_ledger_entry_stale(entry: &LedgerEntry) -> bool {
    let entry_time = if let Ok(parsed) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
        parsed
    } else if let Ok(secs) = entry.timestamp.parse::<i64>() {
        if let Ok(dt) = OffsetDateTime::from_unix_timestamp(secs) {
            dt
        } else {
            return false;
        }
    } else {
        return false;
    };
    let now = OffsetDateTime::now_utc();
    now - entry_time > time::Duration::days(14)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateWorkError {
    pub work_id: String,
    pub branch: Option<String>,
    pub mr_url: Option<String>,
}

impl fmt::Display for DuplicateWorkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Refusing dispatch: active open PR already exists for work ID '{}'",
            self.work_id
        )?;
        if let Some(url) = self.mr_url.as_deref() {
            write!(f, " ({url})")?;
        } else if let Some(branch) = self.branch.as_deref() {
            write!(f, " (branch {branch})")?;
        }
        Ok(())
    }
}

impl std::error::Error for DuplicateWorkError {}

pub(crate) fn duplicate_work_error(err: &anyhow::Error) -> Option<&DuplicateWorkError> {
    err.downcast_ref::<DuplicateWorkError>()
}

fn check_duplicate_work(cfg: &GahConfig, profile: &Profile, args: &DispatchArgs) -> Result<()> {
    let target = if args.target.is_empty() {
        if args.mode == "improve" || args.mode == "fix" {
            let default = PathBuf::from(&profile.artifact_root)
                .join("candidates")
                .join("latest.json");
            if default.exists() {
                default.to_string_lossy().into_owned()
            } else {
                args.target.clone()
            }
        } else {
            args.target.clone()
        }
    } else {
        args.target.clone()
    };

    if target.is_empty() {
        return Ok(());
    }

    let p = Path::new(&target);
    let work_id = if p.extension().is_some_and(|e| e == "json") && p.exists() {
        if let Ok(text) = fs::read_to_string(p) {
            if let Ok(artifact) = serde_json::from_str::<CandidateArtifact>(&text) {
                artifact.candidates.first().map(|c| c.candidate_id.clone())
            } else {
                None
            }
        } else {
            None
        }
    } else if p.extension().is_some_and(|e| e == "md") && p.exists() {
        if let Ok(Some(ticket)) = parse_ticket_metadata(p) {
            ticket.work_id.clone().or(ticket.ticket_id.clone())
        } else {
            None
        }
    } else {
        None
    };

    let Some(work_id) = work_id else {
        return Ok(());
    };

    let matching_entries = match crate::ledger::entries_for_work_id(cfg, &work_id) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("warning: failed to read ledger entries: {:#}", e);
            return Ok(());
        }
    };

    if matching_entries.is_empty() {
        return Ok(());
    }

    // Try to fetch MRs/PRs from provider
    let mrs = crate::sync::fetch_mrs(profile).unwrap_or_default();

    for entry in matching_entries {
        if is_ledger_entry_stale(&entry) {
            continue;
        }

        // Check if there is a matching MR
        let matching_mr = mrs.iter().find(|mr| {
            if let Some(ref entry_branch) = entry.branch {
                if mr.branch == *entry_branch {
                    return true;
                }
            }
            if let Some(ref entry_mr_url) = entry.mr_url {
                if mr.url.as_ref() == Some(entry_mr_url) {
                    return true;
                }
            }
            false
        });

        if let Some(mr) = matching_mr {
            let class = crate::sync::classify(mr);
            if class == "MERGED" {
                continue;
            }
            if class == "CLOSED_UNMERGED" {
                continue;
            }
            if class == "STALE" {
                continue;
            }
            // Otherwise, it's an active open PR -> Block
            return Err(anyhow::Error::new(DuplicateWorkError {
                work_id: work_id.clone(),
                branch: Some(mr.branch.clone()),
                mr_url: mr.url.clone(),
            }));
        }

        // If no matching MR is found, check if branch exists
        let repo_path = Path::new(&profile.local_path);
        if let Some(ref branch_name) = entry.branch {
            if command_output("git", &["rev-parse", "--verify", branch_name], repo_path).is_ok() {
                println!(
                    "Warning: active branch '{}' may already own work for work ID '{}'",
                    branch_name, work_id
                );
            }
        }
    }

    Ok(())
}

/// TICKET-078: observation feed for `decide_next_action` -- one entry per
/// ticket file in `docs/tickets/`. Reuses exactly the same active-MR
/// matching logic as `check_duplicate_work` (branch/mr_url against
/// already-fetched `all_mrs`, MERGED/CLOSED_UNMERGED/STALE = not active)
/// so the duplicate guard and the controller's view of "is this ticket
/// available" can never diverge into two different answers.
/// Looks up a ticket's ledger history by work_id. Returns `None` when the
/// ticket should be dropped from the candidate list entirely (a merged MR
/// anywhere in its history means it's done -- prior failed attempts before
/// that merge shouldn't count toward AUTO_RETRY_CAP and trigger
/// HumanRequired on a completed ticket).
fn ledger_lookup_for_ticket(
    work_id: Option<&str>,
    profile: &Profile,
    all_mrs: &[crate::sync::SyncMr],
    ledger_entries_by_work_id: &crate::ledger::LedgerEntriesByWorkId,
) -> Option<(usize, usize, Option<String>, bool, bool)> {
    let Some(wid) = work_id else {
        return Some((0, 0, None, false, false));
    };
    let entries = ledger_entries_by_work_id.get(wid);
    let mut count = 0usize;
    let mut agent_failure_count = 0usize;
    let mut last_failure_class = None;
    let mut has_active_mr = false;
    let mut has_merged_mr = false;
    // TICKET-human-required-scoping: effective human_required for this work
    // item is the most recent of its own (non-stale, repo-scoped) ledger
    // entries that carry a `human_required` flag. This is the canonical,
    // work-item-scoped derivation -- it must NOT read the single most-recent
    // profile-wide entry, which is what previously froze the whole profile.
    let mut human_required = false;
    for e in entries.into_iter().flatten() {
        // The ledger is a single global file shared by every profile
        // (Defaults::ledger_path, not per-profile), and work_id is
        // just a heading-derived string like "TICKET-090" with no
        // repo namespace -- two unrelated repos (or even two ticket
        // files in the same repo) can legitimately share that exact
        // string. Scope to this profile's own repo so another
        // repo's history can't poison this one's retry count.
        if e.repo_id != profile.repo_id {
            continue;
        }
        if is_ledger_entry_stale(e) {
            continue;
        }
        // Issue #95: tombstone entry from `gah clear-attempts`. When
        // encountered, reset all running counters -- only entries AFTER
        // the latest tombstone count. The tombstone itself is not counted
        // as an attempt.
        if e.mode == "clear_attempts" {
            count = 0;
            agent_failure_count = 0;
            last_failure_class = None;
            has_active_mr = false;
            human_required = false;
            continue;
        }
        count += 1;
        // Issue #95: only genuine agent failures count toward the retry
        // cap. Infra-class failures (backend_error, environment_error,
        // harness_error, unknown) still record in the ledger and appear
        // in status, but do not permanently consume the cap.
        if let Some(ref fc) = e.failure_class {
            if crate::controller::is_genuine_agent_failure(fc) {
                agent_failure_count += 1;
            }
        }
        last_failure_class = e.failure_class.clone().or(last_failure_class);
        if e.human_required {
            human_required = true;
        }
        let matching_mr = all_mrs.iter().find(|mr| {
            e.branch.as_deref().is_some_and(|b| b == mr.branch)
                || (e.mr_url.is_some() && e.mr_url.as_deref() == mr.url.as_deref())
        });
        if let Some(mr) = matching_mr {
            let class = crate::sync::classify(mr);
            if class == "MERGED" {
                has_merged_mr = true;
            } else if !matches!(class, "CLOSED_UNMERGED" | "STALE") {
                has_active_mr = true;
            }
        }
    }
    if has_merged_mr {
        return None;
    }
    Some((
        count,
        agent_failure_count,
        last_failure_class,
        has_active_mr,
        human_required,
    ))
}

/// Leading `TICKET-<digits>` numeric id out of a work_id string, e.g.
/// `"TICKET-101-fail-closed-version-drift"` -> `"101"`. Issue-derived
/// work_ids carry the rest of the title, not just the bare number, so
/// comparisons against `docs/tickets/closed/` filenames must key on this
/// numeric prefix rather than exact string equality.
fn ticket_number_prefix(work_id: &str) -> Option<&str> {
    let rest = work_id.strip_prefix("TICKET-")?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Ticket numbers (e.g. `"115"`) for ticket files archived to
/// `docs/tickets/closed/`. A ticket migrated to a native issue (#46) can be
/// resolved and archived locally while its corresponding issue stays open on
/// the tracker (closing the file doesn't close the issue) -- without this,
/// `scan_available_tickets` re-surfaces already-done work from the issue
/// side forever.
fn closed_ticket_numbers(profile: &Profile) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    let closed_dir = Path::new(&profile.local_path).join("docs/tickets/closed");
    if let Ok(read_dir) = fs::read_dir(&closed_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Some(number) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(ticket_number_prefix)
            {
                ids.insert(number.to_string());
            }
        }
    }
    ids
}

pub fn scan_available_tickets(
    profile: &Profile,
    all_mrs: &[crate::sync::SyncMr],
    ledger_entries_by_work_id: &crate::ledger::LedgerEntriesByWorkId,
) -> Vec<AvailableTicket> {
    let mut candidates = vec![];
    let closed_ids = closed_ticket_numbers(profile);

    let tickets_dir = Path::new(&profile.local_path).join("docs/tickets");
    if let Ok(read_dir) = fs::read_dir(&tickets_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(Some(meta)) = parse_ticket_metadata(&path) else {
                continue;
            };
            if !meta.is_authoritative {
                continue;
            }
            let work_id = meta.work_id.clone().or_else(|| meta.ticket_id.clone());
            let Some((
                prior_attempt_count,
                genuine_agent_failure_count,
                last_failure_class,
                has_active_mr,
                human_required,
            )) = ledger_lookup_for_ticket(
                work_id.as_deref(),
                profile,
                all_mrs,
                ledger_entries_by_work_id,
            )
            else {
                continue;
            };

            candidates.push(AvailableTicket {
                ticket_path: path.display().to_string(),
                work_id,
                title: meta.title.clone(),
                recommended_backend: meta.recommended_backend.clone(),
                recommended_model: meta.recommended_model.clone(),
                prior_attempt_count,
                genuine_agent_failure_count,
                last_failure_class,
                has_active_mr,
                human_required,
            });
        }
    }

    // Native issue tracker (GitHub/GitLab): the migration from docs/tickets
    // to real issues (TICKET-116/#46) only wired up manual `--target
    // <issue-number>` dispatch -- `gah loop`'s own automatic ticket
    // discovery never learned to look here, so a fully-migrated profile's
    // backlog was invisible to `decide_next_action` (it saw 0-1 leftover
    // docs/tickets files instead of the real 100+ open issues). ticket_path
    // is the bare issue number string -- DispatchTicket/Retry/Escalate pass
    // it straight through as `--target`, and `resolve_target_to_issue_or_string`
    // already treats a numeric target as an issue reference.
    for issue in list_open_issues(profile) {
        // Every issue gets a synthesized work_id (TICKET-<number>) even
        // without a TICKET- prefixed title, so is_authoritative is always
        // true here -- unlike docs/tickets files, there's no way for an
        // issue to opt out just by lacking metadata. A "blocked" or
        // "planning" label is the generic signal for "don't auto-dispatch
        // this" (an owner-blocked infra issue with no code fix available,
        // or a planning-only issue with no acceptance criteria yet) --
        // without it, gah loop would burn real dispatch cycles on issues
        // no agent can meaningfully act on before HumanRequired kicks in.
        if issue
            .labels
            .iter()
            .any(|l| matches!(l.to_lowercase().as_str(), "blocked" | "planning"))
        {
            continue;
        }
        let meta = parse_ticket_metadata_from_issue(&issue);
        if !meta.is_authoritative {
            continue;
        }
        let work_id = meta.work_id.clone();
        if work_id
            .as_deref()
            .and_then(ticket_number_prefix)
            .is_some_and(|n| closed_ids.contains(n))
        {
            continue;
        }
        let Some((
            prior_attempt_count,
            genuine_agent_failure_count,
            last_failure_class,
            has_active_mr,
            human_required,
        )) = ledger_lookup_for_ticket(
            work_id.as_deref(),
            profile,
            all_mrs,
            ledger_entries_by_work_id,
        )
        else {
            continue;
        };

        candidates.push(AvailableTicket {
            ticket_path: issue.number.clone(),
            work_id,
            title: meta.title.clone(),
            recommended_backend: meta.recommended_backend.clone(),
            recommended_model: meta.recommended_model.clone(),
            prior_attempt_count,
            genuine_agent_failure_count,
            last_failure_class,
            has_active_mr,
            human_required,
        });
    }

    candidates
}

/// TICKET-127: execute an auto-merge decided by `controller::decide_next_action`.
/// No LLM backend involved -- just the provider merge call, a ledger entry
/// (so `count_merge_attempts_per_branch` can cap retries), and a
/// notification. Merge failures are returned as `Err` for the caller to
/// report as a soft outcome, not a hard loop error.
pub fn merge_branch(
    cfg: &GahConfig,
    profile: &Profile,
    branch: &str,
    work_id: &Option<String>,
    mr_url: &Option<String>,
) -> Result<()> {
    let mut entry = LedgerEntry::new(
        &profile.repo_id,
        profile,
        "none",
        "merge",
        branch,
        None,
        None,
    );
    entry.branch = Some(branch.to_string());
    entry.work_id = work_id.clone();
    entry.mr_url = mr_url.clone();
    entry.attempts_started = 1;

    let result = provider::merge_mr(profile, branch);
    match &result {
        Ok(()) => {
            entry.attempts_completed = 1;
            notify_event(
                profile,
                NotifyEvent::MrMerged {
                    url: mr_url.as_deref().unwrap_or("unknown"),
                    work_id: work_id.as_deref().unwrap_or("unknown"),
                },
            );
        }
        Err(e) => {
            entry.failure_class = Some("merge_failed".to_string());
            entry.error_summary = Some(format!("{:#}", e));
        }
    }
    if let Err(ledger_err) = ledger::append(cfg, &entry) {
        eprintln!("warning: failed to append merge ledger entry: {ledger_err:#}");
    }
    result
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
        check_duplicate_work(cfg, profile, args)?;
    }

    let ts = timestamp();
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
    ledger.dispatch_reason = args.dispatch_reason.clone();
    let started = Instant::now();
    fs::create_dir_all(&session_dir)?;
    println!("Session: {}", session_dir.display());

    let result = match args.mode.as_str() {
        "improve" | "fix" => improve(cfg, profile, args, &session_dir, &mut ledger),
        "pm" => pm(cfg, profile, args, &session_dir, &mut ledger),
        "review" => review(cfg, profile, args, &session_dir, &mut ledger),
        "experiment" => experiment(cfg, profile, args, &session_dir, &mut ledger),
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
    if result.is_err() {
        notify_event(
            profile,
            NotifyEvent::DispatchFailed {
                failure_class: ledger.failure_class.as_deref().unwrap_or("unknown"),
                work_id: ledger.work_id.as_deref().unwrap_or("unknown"),
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

fn resolve_llm(
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

#[allow(clippy::too_many_arguments)]
fn run_backend(
    backend: &str,
    profile: &Profile,
    wt: &Path,
    task: &str,
    session_dir: &Path,
    llm: &runner::LlmConfig,
    effective_model: Option<&str>,
    env_path: Option<&str>,
) -> Result<runner::RunResult> {
    let mut env_vars = env_path.map(runner::load_env_file).unwrap_or_default();
    if backend == "agy-second" {
        if let Some(home) = profile.agy_second_home.as_deref().filter(|h| !h.is_empty()) {
            // Appended last so it overrides any HOME the env_file may have
            // set -- Command::env keeps the last value for a repeated key.
            env_vars.push(("HOME".to_string(), home.to_string()));
        }
    }
    match backend {
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
            profile.opencode_idle_timeout_seconds(),
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
    }
}

/// Run `auto_fix_commands` in the worktree, best-effort, right before
/// `validate()`. A formatter failing to run (missing binary, whatever) must
/// never block the dispatch -- it's a convenience, not a gate -- so every
/// failure is logged and swallowed rather than propagated.
fn run_auto_fix_commands(commands: &[String], wt: &Path) {
    for cmd_str in commands {
        if cmd_str.trim().is_empty() {
            continue;
        }
        match Command::new("sh")
            .args(["-c", cmd_str])
            .current_dir(wt)
            .output()
        {
            Ok(out) if !out.status.success() => {
                eprintln!(
                    "warning: auto_fix command '{cmd_str}' exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Err(e) => {
                eprintln!("warning: auto_fix command '{cmd_str}' failed to run: {e:#}");
            }
            Ok(_) => {}
        }
    }
}

/// Run validation_commands in the worktree. Returns Err(combined output) on first failure.
fn validate(commands: &[String], wt: &Path) -> Result<()> {
    for cmd_str in commands {
        if cmd_str.trim().is_empty() {
            continue;
        }
        println!("  Validating: {}", cmd_str);
        // Run through the shell: validation commands routinely use `cd x && y`,
        // pipes, and env vars, which Command::new(bin) cannot execute.
        let out = Command::new("sh")
            .args(["-c", cmd_str])
            .current_dir(wt)
            .output()
            .with_context(|| format!("failed to run '{}'", cmd_str))?;
        if !out.status.success() {
            bail!(
                "$ {}
{}{}",
                cmd_str,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
        }
    }
    Ok(())
}

/// TICKET-110: like `validate()`, but also surfaces the failing command's
/// exit code so baseline classification can key on POSIX shell conventions
/// (127/126) rather than string-matching stdout.
fn validate_with_exit_code(commands: &[String], wt: &Path) -> Result<(), (String, Option<i32>)> {
    for cmd_str in commands {
        if cmd_str.trim().is_empty() {
            continue;
        }
        println!("  Validating: {}", cmd_str);
        let out = Command::new("sh")
            .args(["-c", cmd_str])
            .current_dir(wt)
            .output()
            .map_err(|e| (format!("failed to run '{}': {:#}", cmd_str, e), None))?;
        if !out.status.success() {
            return Err((
                format!(
                    "$ {}\n{}{}",
                    cmd_str,
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr),
                ),
                out.status.code(),
            ));
        }
    }
    Ok(())
}

/// TICKET-073: verify a profile's `validation_commands` against a genuinely
/// fresh worktree before trusting the dispatch gate.
///
/// This is the "verify the gate itself works before trusting it" check. The
/// common case (commands unchanged since the last successful self-check) is a
/// pure hash compare against durable state and costs essentially nothing. Only
/// on a hash change (or no prior record, or a previously-failed check) does
/// this spin up a real isolated worktree from `default_target_branch`, run the
/// commands once, record pass/fail + the new hash, and clean the worktree up.
///
/// On success this returns `Ok(())` and may have written a new state record.
/// On a failed self-check it bails with a distinct, loud error — the failure
/// class is `FailureClass::ValidationGate`, deliberately *not*
/// `FailureClass::ValidationFailure`, so a broken config is never conflated
/// with the dispatched ticket's own outcome.
///
/// `skip` honours an explicit operator bypass (passed through
/// `--skip-validation-gate`); everything else is fail-closed.
pub fn self_check_validation_gate(profile: &Profile, cfg: &GahConfig, skip: bool) -> Result<()> {
    use crate::validation_check as vc;

    if skip {
        println!("[validation-gate] skipped by explicit --skip-validation-gate");
        return Ok(());
    }

    // Nothing to verify when the profile has no validation commands at all.
    if profile.validation_commands.is_empty() {
        return Ok(());
    }

    let hash = vc::hash_validation_commands(&profile.validation_commands);
    let state_path = vc::resolve_state_path();

    let state = vc::load_state(&state_path)
        .with_context(|| format!("loading validation-check state {}", state_path.display()))?;

    if !vc::should_recheck(&state, &profile.repo_id, &hash) {
        println!(
            "[validation-gate] commands unchanged (hash {}) — skipping fresh-worktree self-check",
            &hash[..hash.len().min(8)]
        );
        return Ok(());
    }

    println!(
        "[validation-gate] commands changed (hash {}) — verifying against a fresh worktree from '{}'...",
        &hash[..hash.len().min(8)],
        profile.default_target_branch
    );

    let repo = Path::new(&profile.local_path);
    let worktree_base = PathBuf::from(&cfg.defaults.worktree_base);

    // `worktree::create` errors out if the branch already exists. Use a
    // full-precision timestamp + random suffix so the branch name is truly
    // unique per run — the previous code truncated to 8 alphanumeric chars
    // from RFC3339, which collapsed to just the date (`20260709`) and
    // caused every same-day gate run after the first to fail.
    let ts = vc::now_rfc3339(OffsetDateTime::now_utc());
    let ts_compact: String = ts.chars().filter(|c| c.is_alphanumeric()).collect();
    let suffix = &ts_compact[..ts_compact.len().min(20)];
    let branch = format!(
        "gah/validation-gate-{}-{}",
        &hash[..hash.len().min(8)],
        suffix
    );

    let wt = worktree::create(
        repo,
        &profile.default_target_branch,
        &branch,
        &worktree_base,
    )?;
    let verified_at = vc::now_rfc3339(OffsetDateTime::now_utc());
    let result = validate(&profile.validation_commands, &wt);
    let ok = result.is_ok();

    // Always clean up, regardless of pass/fail — a leftover validation-gate
    // worktree AND branch is state noise that the next dispatch would trip
    // over. The branch must be deleted too: worktree::cleanup only removes
    // the worktree dir and prunes, leaving the branch ref behind.
    worktree::cleanup(&wt, repo);
    let _ = worktree::git_raw(&["branch", "-D", &branch], repo);

    vc::record_check(&state_path, &profile.repo_id, &hash, ok, &verified_at)
        .with_context(|| format!("recording validation-check result {}", state_path.display()))?;

    if let Err(text) = result {
        anyhow::bail!(
            "VALIDATION GATE FAILED — profile '{}' validation_commands did not pass on a \
             fresh worktree from '{}'. This is a broken gate config, NOT the dispatched \
             ticket's fault. Fix validation_commands (or run with --skip-validation-gate to \
             proceed anyway once you've acknowledged it).\n\n\
             Self-check recorded last_verified_ok=false (hash {}).\n\n\
             Failure output:\n{}",
            profile.repo_id,
            profile.default_target_branch,
            &hash[..hash.len().min(8)],
            text,
        );
    }

    println!(
        "[validation-gate] passed on fresh worktree — self-check recorded (hash {})",
        &hash[..hash.len().min(8)]
    );
    Ok(())
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
/// Issue #155: for AGY, also merges in the run-scoped cli.log delta
/// (quota/reset messages) -- the offset-scoped tail captured by runner,
/// NOT a fresh read of the whole cli.log, so a single attempt's usage is
/// never polluted by prior runs or concurrent appends.
fn attempt_usage(
    log_path: &str,
    agy_cli_log_delta: Option<&str>,
    backend: Option<&str>,
    transcript_path: Option<&str>,
    claude_path: Option<&str>,
) -> crate::ledger::LedgerUsage {
    let text = match fs::read_to_string(log_path) {
        Ok(t) => t,
        Err(_) => return crate::ledger::LedgerUsage::default(),
    };

    // Claude Code: prefer the structured session transcript for real
    // per-attempt token/cost usage (issue #153). Never scrape stdout text.
    if backend == Some("claude") {
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
                    return merged;
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
        return usage;
    }

    // Try codex exec --json parser first — handles JSONL output from
    // codex exec --json where the generic regex parser would find nothing.
    let mut usage = usage::parse_codex_exec_json(&text);
    if usage.usage_source.is_none() {
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
    usage
}

fn usage_has_observation(usage: &crate::ledger::LedgerUsage) -> bool {
    usage.usage_source.is_some()
        || usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.cache_read_tokens.is_some()
        || usage.cache_write_tokens.is_some()
        || usage.total_tokens.is_some()
        || usage.requests_count.is_some()
        || usage.estimated_cost_usd.is_some()
        || usage.actual_cost_usd.is_some()
        || usage.quota_window.is_some()
        || usage.quota_used_percent.is_some()
        || usage.quota_remaining_percent.is_some()
        || usage.quota_reset_at.is_some()
}

fn aggregate_attempt_usage(
    attempts: &[crate::ledger::AttemptRecord],
) -> crate::ledger::LedgerUsage {
    let mut aggregated = crate::ledger::LedgerUsage::default();
    let mut seen = false;
    for attempt in attempts {
        let usage = &attempt.usage;
        if !usage_has_observation(usage) {
            continue;
        }
        seen = true;
        aggregated.input_tokens =
            Some(aggregated.input_tokens.unwrap_or(0) + usage.input_tokens.unwrap_or(0));
        aggregated.output_tokens =
            Some(aggregated.output_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0));
        aggregated.cache_read_tokens =
            Some(aggregated.cache_read_tokens.unwrap_or(0) + usage.cache_read_tokens.unwrap_or(0));
        aggregated.cache_write_tokens = Some(
            aggregated.cache_write_tokens.unwrap_or(0) + usage.cache_write_tokens.unwrap_or(0),
        );
        aggregated.total_tokens =
            Some(aggregated.total_tokens.unwrap_or(0) + usage.total_tokens.unwrap_or(0));
        aggregated.requests_count =
            Some(aggregated.requests_count.unwrap_or(0) + usage.requests_count.unwrap_or(0));
        aggregated.estimated_cost_usd = Some(
            aggregated.estimated_cost_usd.unwrap_or(0.0) + usage.estimated_cost_usd.unwrap_or(0.0),
        );
        aggregated.actual_cost_usd =
            Some(aggregated.actual_cost_usd.unwrap_or(0.0) + usage.actual_cost_usd.unwrap_or(0.0));

        if aggregated.observed_at.as_deref() < usage.observed_at.as_deref() {
            aggregated.observed_at = usage.observed_at.clone();
            aggregated.quota_window = usage.quota_window.clone();
            aggregated.quota_used_percent = usage.quota_used_percent;
            aggregated.quota_remaining_percent = usage.quota_remaining_percent;
            aggregated.quota_reset_at = usage.quota_reset_at.clone();
        }
    }

    if !seen {
        return crate::ledger::LedgerUsage::default();
    }

    if aggregated.input_tokens == Some(0)
        && !attempts
            .iter()
            .any(|attempt| attempt.usage.input_tokens.is_some())
    {
        aggregated.input_tokens = None;
    }
    if aggregated.output_tokens == Some(0)
        && !attempts
            .iter()
            .any(|attempt| attempt.usage.output_tokens.is_some())
    {
        aggregated.output_tokens = None;
    }
    if aggregated.cache_read_tokens == Some(0)
        && !attempts
            .iter()
            .any(|attempt| attempt.usage.cache_read_tokens.is_some())
    {
        aggregated.cache_read_tokens = None;
    }
    if aggregated.cache_write_tokens == Some(0)
        && !attempts
            .iter()
            .any(|attempt| attempt.usage.cache_write_tokens.is_some())
    {
        aggregated.cache_write_tokens = None;
    }
    if aggregated.total_tokens == Some(0)
        && !attempts
            .iter()
            .any(|attempt| attempt.usage.total_tokens.is_some())
    {
        aggregated.total_tokens = None;
    }
    if aggregated.requests_count == Some(0)
        && !attempts
            .iter()
            .any(|attempt| attempt.usage.requests_count.is_some())
    {
        aggregated.requests_count = None;
    }
    if aggregated.estimated_cost_usd == Some(0.0)
        && !attempts
            .iter()
            .any(|attempt| attempt.usage.estimated_cost_usd.is_some())
    {
        aggregated.estimated_cost_usd = None;
    }
    if aggregated.actual_cost_usd == Some(0.0)
        && !attempts
            .iter()
            .any(|attempt| attempt.usage.actual_cost_usd.is_some())
    {
        aggregated.actual_cost_usd = None;
    }
    aggregated.usage_source = Some("attempt_aggregate".to_string());
    aggregated
}

fn preflight(profile: &Profile, backend: &str) -> Result<()> {
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

fn ensure_bin(bin: &str) -> Result<()> {
    if which(bin).is_some() {
        Ok(())
    } else {
        anyhow::bail!("required binary '{}' not found on PATH", bin);
    }
}

fn command_output(bin: &str, args: &[&str], cwd: &Path) -> Result<String> {
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

fn improve(
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
    let mut route = decide_route(cfg, profile, route_req.clone(), ledger)?;
    apply_route_to_ledger(ledger, &route);
    preflight(profile, &route.effective_backend)?;
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

    // TICKET-118: Handle existing branch for FixMr action
    let (branch, wt) = if let Some(ref existing_branch) = args.existing_branch {
        println!(
            "Creating worktree from existing branch '{}'...",
            existing_branch
        );
        let wt = worktree::create_existing(repo, existing_branch, &worktree_base)?;
        (existing_branch.clone(), wt)
    } else {
        println!(
            "Creating worktree from {}...",
            profile.default_target_branch
        );
        let wt = worktree::create(
            repo,
            &profile.default_target_branch,
            &branch,
            &worktree_base,
        )?;
        (branch, wt)
    };
    ledger.branch = Some(branch.clone());
    apply_authoritative_work_identity(ledger, ticket_meta.as_ref(), &branch);
    println!("Worktree: {}", wt.display());
    println!("Branch:   {}", branch);

    let mut base_task = build_task(profile, &wt, &args.mode, &target, issue_details.as_ref());

    // Baseline: run validation once on the pristine worktree BEFORE spending
    // tokens. A failure here is a config error or a pre-existing red repo —
    // either way the agent must know, and later failures can be compared
    // against it to tell "agent made no progress" from "agent broke it".
    let (baseline_failure, baseline_exit_code) = if profile.validation_commands.is_empty() {
        (None, None)
    } else {
        println!("Baseline validation on pristine worktree...");
        match validate_with_exit_code(&profile.validation_commands, &wt) {
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
    let mut backend_summary = String::new();
    for attempt in 0..max_attempts {
        println!(
            "\nAttempt {}/{}: running {} backend...",
            attempt + 1,
            max_attempts,
            route.effective_backend
        );
        let attempt_session = session_dir.join(format!("attempt-{}", attempt + 1));
        fs::create_dir_all(&attempt_session)?;
        ledger.attempts_started += 1;
        let attempt_start = std::time::Instant::now();

        let env_path = if !resolved_env.is_empty() {
            Some(resolved_env)
        } else {
            None
        };
        let result = run_backend(
            &route.effective_backend,
            profile,
            &wt,
            &task,
            &attempt_session,
            &llm,
            route.effective_model.as_deref(),
            env_path,
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
                    usage: crate::ledger::LedgerUsage::default(),
                });
                worktree::cleanup(&wt, repo);
                return Err(e);
            }
        };
        // The backend process launched and ran to an exit code, regardless
        // of what that code was — "completed" tracks whether the attempt
        // got a fair shot, not whether it succeeded.
        ledger.attempts_completed += 1;

        println!(
            "Backend finished: exit={} duration={:.0}s log={}",
            result.exit_code, result.duration_secs, result.log_path
        );
        ledger.backend_exit_code = Some(result.exit_code);

        // Extract backend summary from the tail of the log
        backend_summary = extract_backend_summary(&result.log_path);

        if result.exit_code != 0 {
            // The backend launched but exited nonzero — the backend itself
            // failed at its job, distinct from it never starting at all.
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::AgentRun,
            );
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt + 1,
                backend: route.effective_backend.clone(),
                effective_model: Some(llm.model.clone()),
                exit_code: Some(result.exit_code),
                validation_result: None,
                failure_class: Some(crate::ledger::FailureClass::BackendError.as_str().into()),
                failure_stage: Some(crate::ledger::FailureStage::AgentRun.as_str().into()),
                duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                diff_path: None,
                usage: attempt_usage(
                    &result.log_path,
                    result.agy_cli_log_delta.as_deref(),
                    Some(route.effective_backend.as_str()),
                    result.transcript_path.as_deref(),
                    Some(&claude_path),
                ),
            });
            let log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
            if attempt + 1 < max_attempts {
                if let Some(parsed) = mark_backend_unavailable_from_output(
                    &route.effective_backend,
                    route.effective_model.as_deref(),
                    route.effective_quota_pool.as_deref(),
                    &log_text,
                    &result.log_path,
                )? {
                    let rerouted = decide_route(cfg, profile, route_req.clone(), ledger)?;
                    let current_identity =
                        route_identity(&route.effective_backend, route.effective_model.as_deref());
                    let rerouted_identity = route_identity(
                        &rerouted.effective_backend,
                        rerouted.effective_model.as_deref(),
                    );
                    if rerouted_identity != current_identity {
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
                let _ = worktree::git(&["reset", "--hard", "HEAD"], &wt);
                let _ = worktree::git(&["clean", "-fd"], &wt);
                task = format!(
                    "{}\n\n## Previous attempt did not complete (attempt {}/{})\n\nThe backend exited with code {} before finishing (not a validation failure -- it errored, crashed, or was killed for producing no output). The worktree has been reset clean. Please try again.",
                    base_task,
                    attempt + 1,
                    max_attempts,
                    result.exit_code,
                );
                continue;
            }
            worktree::cleanup(&wt, repo);
            anyhow::bail!(
                "backend exited {} on attempt {}",
                result.exit_code,
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
                    Some(route.effective_backend.as_str()),
                    result.transcript_path.as_deref(),
                    Some(&claude_path),
                ),
            });
            break;
        }

        run_auto_fix_commands(&profile.auto_fix_commands, &wt);

        println!(
            "Running validation ({} commands)...",
            profile.validation_commands.len()
        );
        match validate(&profile.validation_commands, &wt) {
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
                        Some(route.effective_backend.as_str()),
                        result.transcript_path.as_deref(),
                        Some(&claude_path),
                    ),
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
                    // Wipe bad code so next attempt starts clean
                    let _ = worktree::git(&["reset", "--hard", "HEAD"], &wt);
                    let _ = worktree::git(&["clean", "-fd"], &wt);
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
                            Some(route.effective_backend.as_str()),
                            result.transcript_path.as_deref(),
                            Some(&claude_path),
                        ),
                    });
                    // Rebuild from the base task with only the latest failure —
                    // accumulating retry blocks confuses smaller models.
                    task = format!(
                        "{}\n\n## Previous attempt failed validation (attempt {}/{})\n\nYour previous attempt was discarded. The worktree is clean again.\nFix the following before completing the task:\n\n```\n{}\n```",
                        base_task,
                        attempt + 1,
                        max_attempts,
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
                    let rerouted = decide_route(cfg, profile, escalation_req, ledger)?;
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
                            Some(route.effective_backend.as_str()),
                            result.transcript_path.as_deref(),
                            Some(&claude_path),
                        ),
                    });
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
                            Some(route.effective_backend.as_str()),
                            result.transcript_path.as_deref(),
                            Some(&claude_path),
                        ),
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
                            Some(route.effective_backend.as_str()),
                            result.transcript_path.as_deref(),
                            Some(&claude_path),
                        ),
                    });
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

    let has_changes = worktree::has_changes(&wt, &profile.default_target_branch)?;
    if !has_changes {
        println!("No changes produced — nothing to push.");
        worktree::cleanup(&wt, repo);
        return Ok(());
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
        worktree::cleanup(&wt, repo);
        return Ok(());
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
    worktree::push_branch(&wt, &branch, &push_url, &push_pat)?;
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
        profile,
        NotifyEvent::MrCreated {
            url: &mr.url,
            work_id: ledger.work_id.as_deref().unwrap_or("unknown"),
            backend: &route.effective_backend,
            model: route.effective_model.as_deref().unwrap_or("unknown"),
        },
    );

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

fn experiment(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    let route = decide_route(
        cfg,
        profile,
        RouteRequest {
            last_failure_class: None,
            mode: "experiment",
            requested_backend: config::canonical_backend_name(&args.backend),
            requested_model: args.model.as_deref(),
            recommended_backend: None,
            recommended_model: None,
            session_id: session_dir.file_name().and_then(|s| s.to_str()),
            usage_summary: None,
        },
        ledger,
    )?;
    apply_route_to_ledger(ledger, &route);
    preflight(profile, &route.effective_backend)?;
    let llm = resolve_llm(
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
    let branch = format!("gah/exp-{}-{}", profile.repo_id, ts);
    let worktree_base = PathBuf::from(&cfg.defaults.worktree_base);
    let repo = Path::new(&profile.local_path);

    println!(
        "Creating worktree from {}...",
        profile.default_target_branch
    );
    let wt = worktree::create(
        repo,
        &profile.default_target_branch,
        &branch,
        &worktree_base,
    )?;
    ledger.branch = Some(branch.clone());
    println!("Worktree: {}", wt.display());
    println!("Branch:   {}", branch);

    let issue_details = resolve_target_to_issue_or_string(profile, &args.target)?;
    let task = build_task(
        profile,
        &wt,
        "experiment",
        &args.target,
        issue_details.as_ref(),
    );
    let attempt_dir = session_dir.join("attempt-1");
    fs::create_dir_all(&attempt_dir)?;

    let env_path = if !resolved_env.is_empty() {
        Some(resolved_env)
    } else {
        None
    };
    let result = match run_backend(
        &route.effective_backend,
        profile,
        &wt,
        &task,
        &attempt_dir,
        &llm,
        route.effective_model.as_deref(),
        env_path,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Backend error (continuing for judge evaluation): {:#}", e);
            let log_path = attempt_dir.join("backend-output.log");
            let _ = std::fs::write(&log_path, format!("Backend error: {:#}", e));
            runner::RunResult {
                exit_code: -1,
                duration_secs: 0.0,
                log_path: log_path.to_string_lossy().into_owned(),
                agy_cli_log_delta: None,
                transcript_path: None,
            }
        }
    };
    println!(
        "Backend finished: exit={} duration={:.0}s log={}",
        result.exit_code, result.duration_secs, result.log_path
    );
    ledger.backend_exit_code = Some(result.exit_code);

    // Extract backend summary from the tail of the log
    let backend_summary = extract_backend_summary(&result.log_path);

    // Collect research artifacts (notebooks, plots, CSVs, reports)
    let artifacts_dir = session_dir.join("artifacts");
    fs::create_dir_all(&artifacts_dir)?;
    let artifact_count = collect_artifacts(&wt, &artifacts_dir);
    println!("Artifacts collected: {}", artifact_count);

    // Judge whether the task was answered
    let log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
    let answered = judge_experiment(&args.target, &log_text, artifact_count);
    println!(
        "Judge: {}",
        if answered {
            "ANSWERED"
        } else {
            "PARTIAL/UNANSWERED"
        }
    );

    if !answered && artifact_count == 0 {
        println!(
            "No artifacts and judge says task not answered. \
             Check {} for agent output.",
            result.log_path
        );
        worktree::cleanup(&wt, repo);
        return Ok(());
    }

    let has_changes = worktree::has_changes(&wt, &profile.default_target_branch)?;
    if !has_changes {
        println!(
            "No code changes in worktree — artifacts saved to {}",
            artifacts_dir.display()
        );
        worktree::cleanup(&wt, repo);
        return Ok(());
    }
    ledger.validation_result = Some(if answered {
        "answered".into()
    } else {
        "partial".into()
    });
    // TICKET-128: honor the per-profile publishing policy (see fix/improve
    // mode for the full rationale). Experiments may still generate code and
    // artifacts; they just must not be published as an agent-authored MR.
    if !publishing_allows_publish(profile) {
        if profile.publishing.allow_commit_message_generation {
            if worktree::has_uncommitted_changes(&wt)? {
                ledger.commit_attempted = true;
                worktree::stage_all(&wt)?;
                worktree::ensure_staged(&wt)?;
                let commit_msg = format!("gah: experiment for {}", profile.repo_id);
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
        worktree::cleanup(&wt, repo);
        return Ok(());
    }

    println!("Changes detected. Committing and pushing...");
    let mut commit_msg = format!("gah: experiment for {}", profile.repo_id);
    if !backend_summary.is_empty() {
        commit_msg.push_str("\n\n");
        commit_msg.push_str(&backend_summary);
    }
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
    // Must run after the commit above -- see the fix mode call site for why.
    apply_diff_stats(ledger, &wt, &profile.default_target_branch);
    ledger.push_attempted = true;
    worktree::push_branch(&wt, &branch, &push_url, &push_pat)?;
    ledger.push_succeeded = true;

    let mr_ctx = ExperimentMrRenderContext {
        backend: &route.effective_backend,
        model: &llm.model,
        artifact_count,
        answered,
        backend_summary: &backend_summary,
    };
    let mr_body = build_experiment_mr_body(&mr_ctx);
    ledger.mr_attempted = true;
    let mr = provider::create_draft_mr(
        profile,
        &branch,
        &format!("[GAH][EXP] {}", profile.repo_id),
        &mr_body,
    )?;
    ledger.mr_created = true;
    ledger.mr_url = Some(mr.url.clone());
    println!("Draft MR: {}", mr.url);
    notify_event(
        profile,
        NotifyEvent::MrCreated {
            url: &mr.url,
            work_id: ledger.work_id.as_deref().unwrap_or("unknown"),
            backend: &route.effective_backend,
            model: &llm.model,
        },
    );

    worktree::cleanup(&wt, repo);
    Ok(())
}

/// Copy untracked artifact files (notebooks, plots, data exports) to out_dir.
fn collect_artifacts(wt: &Path, out: &Path) -> usize {
    const ARTIFACT_EXTS: &[&str] = &["ipynb", "html", "png", "jpg", "jpeg", "csv", "parquet"];

    // Intentionally omit --exclude-standard: ML repos gitignore *.csv, *.png,
    // *.parquet etc. to avoid committing large datasets. We want those files.
    let Ok(output) = Command::new("git")
        .args(["ls-files", "--others"])
        .current_dir(wt)
        .output()
    else {
        return 0;
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut count = 0;
    for line in text.lines() {
        let path = wt.join(line.trim());
        let is_artifact = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| ARTIFACT_EXTS.contains(&e))
            .unwrap_or(false);
        if is_artifact {
            if let Some(name) = path.file_name() {
                let _ = fs::copy(&path, out.join(name));
                count += 1;
            }
        }
    }
    count
}

/// Ask an LLM judge whether the agent answered the task.
/// Always calls Claude regardless of artifact count — a blank file is not success.
/// Falls back to artifact presence only when `claude` is unavailable.
fn judge_experiment(task: &str, log: &str, artifact_count: usize) -> bool {
    let artifact_note = if artifact_count > 0 {
        format!("{} output file(s) were produced.", artifact_count)
    } else {
        "No output files were produced.".to_string()
    };
    let prompt = format!(
        "You are evaluating whether an AI agent meaningfully answered a research task.\n\n\
         Task:\n{}\n\n\
         Agent output (last 3000 chars):\n{}\n\n\
         {}\n\n\
         Did the agent produce a substantive, non-trivial answer to the task? \
         An empty file, a stub, or a generic error message does not count. \
         Reply with only YES or NO.",
        utf8_safe_prefix(task, 500),
        utf8_safe_suffix(log, 3000),
        artifact_note,
    );
    Command::new("claude")
        .args(["-p", &prompt])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .to_uppercase()
                .contains("YES")
        })
        // claude unavailable: fall back to artifact presence as a weak signal
        .unwrap_or(artifact_count > 0)
}

fn pm(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    let repo = Path::new(&profile.local_path);

    // Without a target: static repo snapshot (context for the agent, not a dispatch)
    if args.target.is_empty() {
        let log = git_output(&["log", "--oneline", "-20"], repo).unwrap_or_default();
        let test_count = count_test_files(profile, repo);
        let has_ci = repo.join(".github/workflows").exists()
            || repo.join(".gitlab-ci.yml").exists()
            || repo.join(".ci").exists();
        let readme = repo.join("README.md");
        let readme_text = if readme.exists() {
            let s = fs::read_to_string(&readme).unwrap_or_default();
            utf8_safe_prefix(&s, 2000).to_string()
        } else {
            String::from("(no README)")
        };
        let report = format!(
            "# PM Report: {}\n\nRepo: {}\nBranch: {}\nTest files: {}\nCI configured: {}\n\n\
             ## Recent commits\n```\n{}\n```\n\n## README\n{}\n",
            profile.display_name,
            profile.repo,
            profile.default_target_branch,
            test_count,
            has_ci,
            log,
            readme_text,
        );
        let out_path = session_dir.join("pm-report.md");
        fs::write(&out_path, &report)?;
        println!("{}", report);
        println!("Written: {}", out_path.display());
        ledger.validation_result = Some("not_run".into());
        return Ok(());
    }

    // With a target: dispatch an LLM to produce a structured ticket plan.
    let preflight_ctx = collect_pm_preflight(profile, repo)?;
    let route_req = RouteRequest {
        mode: "pm",
        requested_backend: config::canonical_backend_name(&args.backend),
        requested_model: args.model.as_deref(),
        recommended_backend: None,
        recommended_model: None,
        session_id: session_dir.file_name().and_then(|s| s.to_str()),
        usage_summary: None,
        last_failure_class: None,
    };
    let mut plan_route = decide_route(cfg, profile, route_req.clone(), ledger)?;
    apply_route_to_ledger(ledger, &plan_route);
    preflight(profile, &plan_route.effective_backend)?;
    let mut llm = resolve_llm(
        cfg,
        args,
        profile.oh_profile.as_deref(),
        plan_route.effective_model.as_deref(),
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

    let task = build_pm_plan_task(profile, &preflight_ctx, &args.target)?;

    let mut attempted_routes = HashSet::new();
    let log_text = loop {
        let attempt_index = attempted_routes.len() + 1;
        let attempt_dir = session_dir.join(format!("pm-run-{attempt_index}"));
        fs::create_dir_all(&attempt_dir)?;
        fs::write(attempt_dir.join("task.md"), &task)?;

        let result = run_backend(
            &plan_route.effective_backend,
            profile,
            repo,
            &task,
            &attempt_dir,
            &llm,
            plan_route.effective_model.as_deref(),
            None,
        )?;
        println!(
            "PM backend finished: exit={} duration={:.0}s log={}",
            result.exit_code, result.duration_secs, result.log_path
        );
        ledger.backend_exit_code = Some(result.exit_code);
        ledger.validation_result = Some("not_run".into());

        let log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
        if result.exit_code == 0 {
            break log_text;
        }

        ledger.set_failure(
            crate::ledger::FailureClass::BackendError,
            crate::ledger::FailureStage::AgentRun,
        );
        let route_key = route_identity(
            &plan_route.effective_backend,
            plan_route.effective_model.as_deref(),
        );
        attempted_routes.insert(route_key);
        let parsed = mark_backend_unavailable_from_output(
            &plan_route.effective_backend,
            plan_route.effective_model.as_deref(),
            plan_route.effective_quota_pool.as_deref(),
            &log_text,
            &result.log_path,
        )?;
        let Some(parsed) = parsed else {
            anyhow::bail!(
                "PM backend exited {} with no recognized quota/rate-limit signal in {}",
                result.exit_code,
                result.log_path
            );
        };

        let rerouted = decide_route(cfg, profile, route_req.clone(), ledger)?;
        let rerouted_key = route_identity(
            &rerouted.effective_backend,
            rerouted.effective_model.as_deref(),
        );
        if attempted_routes.contains(&rerouted_key) {
            anyhow::bail!(
                "PM backend {} became unavailable ({:?}) and no new eligible backend remained",
                plan_route.effective_backend,
                parsed.kind
            );
        }
        println!(
            "PM rerouting: {} -> {} ({:?})",
            plan_route.effective_backend, rerouted.effective_backend, parsed.kind
        );
        plan_route = rerouted;
        apply_route_to_ledger(ledger, &plan_route);
        preflight(profile, &plan_route.effective_backend)?;
        llm = resolve_llm(
            cfg,
            args,
            profile.oh_profile.as_deref(),
            plan_route.effective_model.as_deref(),
        )?;
    };

    let plan = parse_pm_plan(&log_text)?;
    let written = apply_pm_plan(repo, &preflight_ctx, &plan)?;
    println!("\nCreated {} ticket(s):", written.len());
    for path in &written {
        println!("  {}", path.display());
    }

    Ok(())
}

fn build_pm_plan_task(profile: &Profile, ctx: &PmPreflight, target: &str) -> Result<String> {
    Ok(format!(
        "Repository: {} ({})\nLocal path: {}\nTarget branch: {}\n\n\
         You are a project manager. Use only the preflight context below plus the target request. \
         Do not assume you will remember to inspect the repo yourself.\n\
         Do not write code in PM mode.\n\
         Return only valid JSON matching this schema:\n\
         {{\"title\":string,\"summary\":string,\"tickets\":[{{\"title\":string,\"summary\":string,\"difficulty\":\"easy|medium|hard\",\"risk\":\"low|medium|high\",\"recommended_backend\":string|null,\"duplicate_evidence\":[string],\"affected_files\":[string],\"acceptance_criteria\":[string],\"verification_commands\":[string],\"uncovered_reason\":string}}]}}\n\n\
         Rules:\n\
         - Default action is to avoid creating new tickets.\n\
         - Do not create a ticket if an open MR already covers the issue.\n\
         - Do not create a ticket if an existing ticket already covers the issue.\n\
         - Do not create a ticket if a recently merged MR already fixed the issue.\n\
         - Consolidate small related fixes into one ticket.\n\
         - Prefer \"already covered\" or \"update existing ticket\" when unsure.\n\
         - If creating tickets, create only genuinely uncovered, atomic work.\n\
         - Cite the relevant MR or ticket evidence whenever you decide work is already covered.\n\
         - If the work is already covered, return an empty tickets array.\n\
         - Each new ticket must be independently completable in one session.\n\n\
         ## Preflight Context\n\n{}\n\n\
         ## Target Request\n\n{}\n",
        profile.display_name,
        profile.repo,
        profile.local_path,
        profile.default_target_branch,
        ctx.rendered,
        target,
    ))
}

fn collect_pm_preflight(profile: &Profile, repo: &Path) -> Result<PmPreflight> {
    let memory_path = repo.join("docs/MANAGER_MEMORY.md");
    let manager_memory = fs::read_to_string(&memory_path).with_context(|| {
        format!(
            "PM mode requires manager memory at {}",
            memory_path.display()
        )
    })?;

    let repo_state = collect_pm_repo_state(repo);
    let tickets = collect_ticket_summaries(&repo.join("docs/tickets"))?;
    let open_mrs = collect_mr_context(profile, repo, "opened", None)?;
    let merged_mrs = collect_mr_context(profile, repo, "merged", Some("20"))?;

    let mut out = String::new();
    out.push_str("### Manager Memory\n");
    out.push_str(&manager_memory);
    if !manager_memory.ends_with('\n') {
        out.push('\n');
    }

    out.push_str("\n### Open Merge Requests\n");
    out.push_str(&open_mrs);
    out.push_str("\n\n### Recently Merged Merge Requests\n");
    out.push_str(&merged_mrs);
    out.push_str("\n\n### Existing Tickets\n");
    if tickets.is_empty() {
        out.push_str("(none found)");
    } else {
        out.push_str(&tickets.join("\n"));
    }
    out.push_str("\n\n### Repo State\n");
    out.push_str(&repo_state);
    Ok(PmPreflight {
        rendered: out,
        existing_tickets: tickets,
        open_mrs,
        merged_mrs,
    })
}

fn collect_mr_context(
    profile: &Profile,
    repo: &Path,
    state: &str,
    per_page: Option<&str>,
) -> Result<String> {
    if profile.provider != "gitlab" {
        return Ok("(provider is not gitlab)".to_string());
    }

    ensure_bin("glab")?;
    let mut args = vec![
        "mr",
        "list",
        "--repo",
        profile.repo.as_str(),
        "--state",
        state,
    ];
    if let Some(per_page) = per_page {
        args.push("--per-page");
        args.push(per_page);
    }
    let output = command_output("glab", &args, repo)?;
    if output.is_empty() {
        Ok("(none)".to_string())
    } else {
        Ok(output)
    }
}

fn collect_ticket_summaries(tickets_dir: &Path) -> Result<Vec<String>> {
    if !tickets_dir.exists() {
        return Ok(vec![]);
    }

    let mut tickets = vec![];
    for entry in fs::read_dir(tickets_dir)
        .with_context(|| format!("reading {}", tickets_dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let body = fs::read_to_string(&path).unwrap_or_default();
        let title = first_markdown_heading(&body).unwrap_or("(no heading)");
        tickets.push(format!(
            "- {}: {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            title
        ));
    }
    tickets.sort();
    Ok(tickets)
}

fn first_markdown_heading(body: &str) -> Option<&str> {
    body.lines().map(str::trim).find_map(|line| {
        if !line.starts_with('#') {
            return None;
        }
        let stripped = line.trim_start_matches('#').trim();
        (!stripped.is_empty()).then_some(stripped)
    })
}

fn collect_pm_repo_state(repo: &Path) -> String {
    let branch = command_output("git", &["rev-parse", "--abbrev-ref", "HEAD"], repo)
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));
    let dirty = command_output("git", &["status", "--short"], repo)
        .map(|s| if s.is_empty() { "clean".to_string() } else { s })
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));
    let commits = command_output("git", &["log", "--oneline", "-5"], repo)
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));

    format!(
        "Current branch: {}\n\nDirty status:\n{}\n\nRecent commits:\n{}",
        branch, dirty, commits
    )
}

fn review(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
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
            "MR: {}\nURL: {}\nSource: {}\nTarget: {}\nRepo: {}\nTitle: {}\nCI: {}",
            target.mr_id.as_deref().unwrap_or("n/a"),
            target.mr_url.as_deref().unwrap_or("n/a"),
            target.source_branch,
            target.target_branch,
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

    // Everything except the capability-activation prefix is identical
    // regardless of which backend ends up running the review.
    let prompt_suffix = format!(
        "Review this diff for correctness, test coverage, and safety. \
         Return two sections:\n\
         1. Markdown review notes.\n\
         2. A JSON object with fields: verdict, confidence, human_required, blocking_findings, non_blocking_findings, risk_notes.\n\
         blocking_findings, non_blocking_findings, and risk_notes must be JSON arrays of strings, even when empty or when only one item exists.\n\
         Verdict must be one of APPROVE_STRONG, APPROVE_WEAK, NEEDS_FIX, REJECT, HUMAN_REVIEW, defined as:\n\
         - APPROVE_STRONG: you have high confidence this change is correct, safe, and complete. No unresolved concern is worth surfacing as something that should change before merge.\n\
         - APPROVE_WEAK: you believe the change is likely fine, but YOUR OWN review confidence is low -- insufficient context, a domain you couldn't fully verify, or a partial review. This is not a substitute for NEEDS_FIX.\n\
         - NEEDS_FIX: you found a concrete, real problem that should be fixed before merge. Put it in blocking_findings, even if it isn't an immediate crash -- e.g. silent data loss, a hidden failure mode, or anything that would take real effort to diagnose later if left in. Do not downgrade a genuine risk into non_blocking_findings/risk_notes just because it wouldn't break the build today.\n\
         - REJECT: the change is fundamentally wrong and should not be merged as-is.\n\
         - HUMAN_REVIEW: you cannot make a confident recommendation at all.\n\
         Repo: {}. MR: {}. Source: {}. Target: {}. CI status: {}.\n\
         MR title: {}\nMR body:\n{}\n\
         Prior run state:\n{}\n\nDiff:\n```\n{}\n```\nChanged files:\n{}",
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
    let env_vars = if resolved_env.is_empty() {
        vec![]
    } else {
        runner::load_env_file(resolved_env)
    };

    let mut route = decide_route(
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
        ledger,
    )?;

    // Bounded retry across review_candidates: an empty/unavailable-backend
    // outcome (e.g. AGY quota exhaustion -- see agy_empty_output_diagnosis)
    // used to fail the whole review outright even though review_candidates
    // often lists real fallbacks (agy-second, claude) that just sat unused.
    const MAX_REVIEW_ATTEMPTS: usize = 3;
    let mut applied_capabilities = vec![];
    let mut result = None;
    for attempt_number in 0..MAX_REVIEW_ATTEMPTS {
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
        let prompt = format!("{capability_prefix}{prompt_suffix}");

        let attempt = runner::run_review_backend(
            profile,
            &route.effective_backend,
            repo,
            &prompt,
            session_dir,
            route.effective_model.as_deref(),
            &env_vars,
        );
        let is_last_attempt = attempt_number + 1 == MAX_REVIEW_ATTEMPTS;
        if !is_last_attempt {
            if let runner::ReviewProcessOutcome::NonZeroExit(_) = attempt.outcome {
                if let Some(parsed) = mark_backend_unavailable_from_output(
                    &route.effective_backend,
                    route.effective_model.as_deref(),
                    None,
                    &attempt.stdout,
                    &session_dir.join("review-stdout.log").display().to_string(),
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
    if !result.stderr.trim().is_empty() {
        fs::write(session_dir.join("review-stderr.log"), &result.stderr)?;
    }

    match result.outcome {
        runner::ReviewProcessOutcome::Success => {
            let mut review_usage = usage::parse_generic_usage(&result.stdout, "review_output_log");
            if review_usage.usage_source.is_some() {
                review_usage.observed_at = Some(
                    time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                );
            }
            let reviewer_tier = derive_reviewer_tier(cfg, profile, &route);
            let verdict =
                match parse_review_verdict(&result.stdout, &route, &review_usage, reviewer_tier) {
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
                        return Err(err);
                    }
                };
            fs::write(&verdict_path, serde_json::to_string_pretty(&verdict)?)?;
            println!("{}", result.stdout);
            println!("Written: {}", report_path.display());
            println!("Written: {}", verdict_path.display());
            ledger.backend_exit_code = Some(0);
            ledger.validation_result = Some(verdict.verdict.clone());
            ledger.human_required = verdict.human_required;
            ledger.confidence_impact = Some(verdict.confidence.clone());
            ledger.usage = review_usage.clone();
            // TICKET-125: attribute this verdict back to the branch's
            // implementation entry (the backend that wrote the code being
            // reviewed), not this review dispatch's own entry (the reviewer).
            if let Err(err) = crate::ledger::backfill_review_verdict(
                cfg,
                &target.source_branch,
                &verdict.verdict,
                &verdict.confidence,
                &route.effective_backend,
                route.effective_model.as_deref(),
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
                profile,
                NotifyEvent::ReviewVerdict {
                    verdict: published_review_verdict(&verdict.verdict).leak(),
                    mr_url: mr_url.as_deref().unwrap_or("unknown"),
                },
            );
            if verdict.human_required {
                notify_event(
                    profile,
                    NotifyEvent::HumanRequired {
                        reason: "review verdict requires human attention",
                        reference: mr_url.as_deref(),
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
            } else if let Err(err) =
                provider::post_review_comment(profile, &target.source_branch, &mr_body, &labels)
            {
                eprintln!("warning: failed to post MR review comment: {:#}", err);
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
        }
        runner::ReviewProcessOutcome::SpawnFailure => {
            ledger.set_failure(
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::BackendLaunch,
            );
            ledger.validation_result = Some("not_run".into());
            println!("Review backend failed to launch.");
            println!("Review bundle written to: {}", bundle.display());
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
        }
        runner::ReviewProcessOutcome::SignalTermination(signal) => {
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::Review,
            );
            ledger.backend_exit_code = Some(-signal);
            ledger.validation_result = Some("not_run".into());
            println!("Review backend terminated by signal {}.", signal);
            println!("Review bundle written to: {}", bundle.display());
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
            } else {
                let cloud = args.backend == "cloud-coder";
                let default_model = cfg.defaults.llm_model(cloud);
                let model_name = args.model.as_deref().unwrap_or(&default_model);
                println!("LLM model:    {}", model_name);
                println!("LLM base:     {}", cfg.defaults.llm_base_url());
            }
            println!("Backend:      {}", args.backend);
            if let Some(route) = &route {
                println!("Effective:    {}", route.effective_backend);
                println!("Routing:      {}", route.routing_reason);
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
                    println!("Effective:    {}", route.effective_backend);
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
                println!("Effective:    {}", route.effective_backend);
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
/// If `target` is a path to a candidates.json file, build a structured packet from the first candidate.
/// Otherwise build a mode-appropriate prompt with target as the task body.
/// `issue_details` must be resolved once by the caller (see
/// `resolve_target_to_issue_or_string`) -- resolving it again in here would
/// mean a second live gh/glab fetch per dispatch for the same target, with
/// no guarantee the two fetches agree.
fn build_task(
    profile: &Profile,
    wt: &Path,
    mode: &str,
    target: &str,
    issue_details: Option<&IssueDetails>,
) -> String {
    if let Some(issue) = issue_details {
        return build_task_with_issue(profile, wt, mode, issue);
    }
    // Try to load as candidate artifact
    if !target.is_empty() {
        let p = Path::new(target);
        if p.extension().is_some_and(|e| e == "json") && p.exists() {
            if let Ok(text) = fs::read_to_string(p) {
                if let Ok(artifact) = serde_json::from_str::<CandidateArtifact>(&text) {
                    if let Some(candidate) = artifact.candidates.first() {
                        return format_candidate_task(profile, wt, mode, candidate);
                    }
                }
            }
        }
    }

    let instruction = match mode {
        "fix" => {
            "Fix the specific issue described in the Focus section below.\n\
             Run the relevant tests to confirm the fix. All tests in the test suite must pass.\n\
             Do not push or create MRs."
        }
        "experiment" => {
            "This is a research/experiment task. Write scripts, generate Jupyter notebooks, \
             CSV exports, plots, or markdown reports directly in the working directory.\n\
             Do not worry about breaking unrelated tests. Prioritize producing observable \
             output files (*.ipynb, *.html, *.csv, *.png, *.md) over clean commits.\n\
             Do not push or create MRs."
        }
        _ if !target.is_empty() => {
            "Implement ONLY the specific ticket described in the Focus section below. \
             Ignore any other backlog items, priorities, or tickets mentioned in Manager \
             Memory above -- those are background context, not additional work to pick up.\n\
             Run tests if a test command is available and ensure they pass.\n\
             Do not push or create MRs."
        }
        _ => {
            "Select and implement the highest-priority improvement from the backlog, \
             recent CI failures, or test gaps.\n\
             Run tests if a test command is available and ensure they pass.\n\
             Do not push or create MRs."
        }
    };

    let mut task = format!(
        "Repository: {} ({})\nWorking directory: {}\nTarget branch: {}\n\n{}\n",
        profile.display_name,
        profile.repo,
        wt.display(),
        profile.default_target_branch,
        instruction,
    );

    // Read from the main checkout (profile.local_path), not the worktree --
    // same source `collect_pm_preflight` already reads for PM mode. Manager
    // memory is live operational state, not something that should need a
    // commit+push round-trip through a target-branch worktree to reach a
    // dispatched agent. Optional here (unlike PM mode, which requires it)
    // since not every repo has this file.
    if let Ok(memory) =
        fs::read_to_string(Path::new(&profile.local_path).join("docs/MANAGER_MEMORY.md"))
    {
        task.push_str(
            "\n## Manager Memory (read this before exploring -- documents known \
             environment setup, conventions, and known issues)\n\n",
        );
        task.push_str(&memory);
        if !memory.ends_with('\n') {
            task.push('\n');
        }
    }

    if !target.is_empty() {
        task.push_str(&format!("\n## Focus\n\n{}\n", target));
    }
    task
}

/// Build task with issue details for the Focus section
fn build_task_with_issue(profile: &Profile, wt: &Path, mode: &str, issue: &IssueDetails) -> String {
    let instruction = match mode {
        "fix" => {
            "Fix the specific issue described in the Focus section below.\n\
             Run the relevant tests to confirm the fix. All tests in the test suite must pass.\n\
             Do not push or create MRs."
        }
        "experiment" => {
            "This is a research/experiment task. Write scripts, generate Jupyter notebooks, \
\
             CSV exports, plots, or markdown reports directly in the working directory.\n\
             Do not worry about breaking unrelated tests. Prioritize producing observable \
\
             output files (*.ipynb, *.html, *.csv, *.png, *.md) over clean commits.\n\
             Do not push or create MRs."
        }
        _ => {
            "Implement ONLY the specific ticket described in the Focus section below. \
\
             Ignore any other backlog items, priorities, or tickets mentioned in Manager \
\
             Memory above -- those are background context, not additional work to pick up.\n\
             Run tests if a test command is available and ensure they pass.\n\
             Do not push or create MRs."
        }
    };

    let mut task = format!(
        "Repository: {} ({})\nWorking directory: {}\nTarget branch: {}\n\n{}\n",
        profile.display_name,
        profile.repo,
        wt.display(),
        profile.default_target_branch,
        instruction,
    );

    // Read from the main checkout (profile.local_path), not the worktree --
    // same source `collect_pm_preflight` already reads for PM mode. Manager
    // memory is live operational state, not something that should need a
    // commit+push round-trip through a target-branch worktree to reach a
    // dispatched agent. Optional here (unlike PM mode, which requires it)
    // since not every repo has this file.
    if let Ok(memory) =
        fs::read_to_string(Path::new(&profile.local_path).join("docs/MANAGER_MEMORY.md"))
    {
        task.push_str(
            "\n## Manager Memory (read this before exploring -- documents known \
\
             environment setup, conventions, and known issues)\n\n",
        );
        task.push_str(&memory);
        if !memory.ends_with('\n') {
            task.push('\n');
        }
    }

    task.push_str(&format!(
        "\n## Focus\n\n{}\n",
        format_issue_for_focus(issue)
    ));
    task
}

fn format_candidate_task(
    profile: &Profile,
    _wt: &Path,
    mode: &str,
    c: &crate::models::Candidate,
) -> String {
    let mut out = format!(
        "# Task: {}\n\n\
         Repository: {} ({})\n\
         Local path: {}\n\
         Target branch: {}\n\n",
        c.candidate_id,
        profile.display_name,
        profile.repo,
        profile.local_path,
        profile.default_target_branch,
    );

    if !c.evidence.is_empty() {
        out.push_str("## Context\n");
        for e in &c.evidence {
            out.push_str(&format!("- {}\n", e));
        }
        out.push('\n');
    }

    if !c.affected_files.is_empty() {
        out.push_str("## Files likely involved\n");
        for f in &c.affected_files {
            out.push_str(&format!("- {}\n", f));
        }
        out.push('\n');
    }

    if !c.acceptance_criteria.is_empty() {
        out.push_str("## Acceptance criteria\n");
        for (i, ac) in c.acceptance_criteria.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, ac));
        }
        out.push('\n');
    }

    if !c.verification.is_empty() {
        out.push_str("## Verification steps\n");
        for (i, v) in c.verification.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, v));
        }
        out.push('\n');
    }

    let closing = match mode {
        "fix" => {
            "Fix the code to satisfy every acceptance criterion above.\n\
             Run the verification steps to confirm. All tests must pass. Do not push or create MRs.\n"
        }
        "experiment" => {
            "This is a research task. Satisfy the acceptance criteria by producing output files \
             (*.ipynb, *.html, *.csv, *.png, *.md). Do not push or create MRs.\n"
        }
        _ => {
            "Implement the changes required to satisfy the acceptance criteria above.\n\
             Run tests if a test command exists and ensure they pass. Do not push or create MRs.\n"
        }
    };
    out.push_str(closing);
    out
}

#[cfg(test)]
mod tests {
    use super::preflight;
    use super::run_auto_fix_commands;
    use super::validate;
    use super::{
        apply_authoritative_work_identity, apply_diff_stats, apply_pm_plan, apply_route_to_ledger,
        attempt_usage, build_experiment_mr_body, build_fix_or_improve_mr_body,
        build_metadata_rich_mr_body, build_mr_title, build_pm_plan_task, build_standard_mr_body,
        build_task, classify_validation_failure_progress, collect_pm_preflight,
        collect_ticket_summaries, derive_reviewer_tier, extract_issue_number,
        first_markdown_heading, format_issue_for_focus, is_issue_number_reference,
        mark_backend_unavailable_from_output_at, next_ticket_id, parse_pm_plan,
        parse_review_verdict, parse_ticket_metadata, parse_ticket_metadata_from_issue,
        published_review_verdict, render_review_comment, review_labels, review_preflight,
        run_backend, scan_available_tickets, strip_terminal_noise,
        validation_failure_no_progress_reason, ExperimentMrRenderContext, IssueDetails,
        MrRenderContext, ReviewerTier, RouteDecision, TicketMetadata, ValidationFailureProgress,
    };
    use crate::availability::{availability_for, load_state, Reason};
    use crate::config::{Defaults, GahConfig, Profile, RoutingPolicy};
    use crate::ledger::LedgerEntry;
    use crate::models::PmPlan;
    use crate::test_support::PathGuard;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use time::OffsetDateTime;

    const CODEX_FULL_RESET: &str =
        include_str!("../tests/fixtures/quota-logs/codex_usage_exhausted_full_reset.txt");

    #[test]
    fn strip_terminal_noise_removes_ansi_codes_and_openhands_exit_banner() {
        // Reproduces the exact garbage confirmed live in worldcup-props MR
        // !243's "What changed and why" section: ANSI color codes, box-
        // drawing panel borders, and openhands' unconditional exit banner,
        // all landing verbatim in a PR/MR body.
        let raw = "\u{1b}[36m\u{2502}\u{1b}[0m Fixed the eligibility gate.        \u{1b}[36m\u{2502}\u{1b}[0m\n\
                   \u{1b}[92m\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{1b}[0m\n\
                   Goodbye! \u{1f44b}\n\
                   Conversation ID: 4fae58dacf204cff9f92fbb7fbed8229\n\
                   Hint: run openhands --resume 4fae58da-cf20-4cff-9f92-fbb7fbed8229 to resume this\n\
                   conversation.\n";
        let cleaned = strip_terminal_noise(raw);
        assert_eq!(cleaned, "Fixed the eligibility gate.");
    }

    #[test]
    fn strip_terminal_noise_leaves_plain_text_untouched() {
        let plain = "Implemented the fix.\nAll tests pass.";
        assert_eq!(strip_terminal_noise(plain), plain);
    }

    fn profile(local_path: &Path) -> Profile {
        Profile {
            prune_older_than_days: None,
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: local_path.display().to_string(),
            artifact_root: "/tmp/artifacts".into(),
            default_target_branch: "main".into(),
            provider_api_base: None,
            provider_project_id: None,
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            vibe_args: vec![],
            vibe_path: None,
            opencode_args: vec![],
            opencode_path: None,
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds: None,
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            auto_fix_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            notify_command: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
            publishing: Default::default(),
        }
    }

    fn gah_config(routing: RoutingPolicy) -> GahConfig {
        GahConfig {
            defaults: Defaults {
                artifact_root: String::new(),
                worktree_base: String::new(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing,
            },
            profiles: std::collections::HashMap::new(),
        }
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
    fn reviewer_tier_weak_when_backend_matches_weak_config() {
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
    fn approve_strong_from_weak_tier_still_forces_human_review_and_label() {
        // TICKET-108: this is the case the old fallback_used-based rewrite
        // used to handle by corrupting the verdict string. Now the verdict
        // text is untouched and reviewer_tier carries the distrust signal.
        let json = r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("codex", None, true);
        let verdict = parse_review_verdict(json, &route, &usage, ReviewerTier::Weak).unwrap();

        assert_eq!(
            verdict.verdict, "APPROVE_STRONG",
            "verdict text is never rewritten"
        );
        assert_eq!(verdict.reviewer_tier.as_deref(), Some("weak"));
        assert!(verdict.human_required);
        assert_eq!(verdict.confidence, "medium");
        assert_eq!(
            review_labels(&verdict),
            vec!["gah-review-weak", "gah-human-review"]
        );
    }

    #[test]
    fn approve_strong_from_strong_tier_is_not_forced_to_human_review() {
        let json = r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#;
        let usage = crate::ledger::LedgerUsage::default();
        let route = route_decision("claude", Some("sonnet"), false);
        let verdict = parse_review_verdict(json, &route, &usage, ReviewerTier::Strong).unwrap();

        assert_eq!(verdict.reviewer_tier.as_deref(), Some("strong"));
        assert!(!verdict.human_required);
        assert_eq!(verdict.confidence, "high");
        assert_eq!(review_labels(&verdict), vec!["gah-ready-for-human"]);
    }

    #[test]
    fn approve_weak_verdict_forces_human_review_regardless_of_tier() {
        // A weak VERDICT (the reviewer's own uncertainty) is a separate
        // signal from reviewer TIER (who reviewed) -- even a strong-tier
        // reviewer returning APPROVE_WEAK must still get human eyes.
        let json = r#"{"verdict":"APPROVE_WEAK","confidence":"medium","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#;
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
        let vibe_json_output = r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#;

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

        assert_eq!(verdict.verdict, "APPROVE_STRONG");
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
    fn review_preflight_fails_with_backend_unavailable_when_executable_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.claude_path = Some("/definitely/does/not/exist/claude".into());
        let cfg = gah_config(RoutingPolicy::default());

        let err = review_preflight(&cfg, &prof, "claude").unwrap_err();
        assert!(format!("{:#}", err).contains("backend unavailable"));
    }

    #[test]
    fn attempt_usage_parses_real_log_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("backend-output.log");
        fs::write(
            &path,
            "some agent output\ninput_tokens: 500\noutput_tokens: 120\n",
        )
        .unwrap();

        let usage = attempt_usage(path.to_str().unwrap(), None, None, None, None);
        assert_eq!(usage.input_tokens, Some(500));
        assert_eq!(usage.output_tokens, Some(120));
        assert_eq!(usage.total_tokens, Some(620));
    }

    #[test]
    fn attempt_usage_is_empty_not_zero_when_log_missing() {
        // TICKET-101: unknown must remain unknown, never a fabricated zero.
        let usage = attempt_usage(
            "/definitely/does/not/exist/backend-output.log",
            None,
            None,
            None,
            None,
        );
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.usage_source, None);
    }

    #[test]
    fn attempt_usage_is_empty_when_log_has_no_usage_info() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("backend-output.log");
        fs::write(&path, "agent made some edits, no usage reported\n").unwrap();

        let usage = attempt_usage(path.to_str().unwrap(), None, None, None, None);
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.usage_source, None);
    }

    #[test]
    fn scan_available_tickets_reports_never_dispatched_ticket() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-200-test.md"),
            "# TICKET-200: Test ticket\n\nGoal: test\n\nRecommended backend: codex\nRecommended model: gpt-5.4\n",
        )
        .unwrap();
        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        // Not testing issue-tracker scanning here -- an unmapped provider
        // keeps scan_available_tickets from shelling out to a real `gh`/`glab`
        // on whatever happens to be on PATH during this test.
        prof.provider = String::new();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].work_id.as_deref(), Some("TICKET-200"));
        assert_eq!(candidates[0].prior_attempt_count, 0);
        assert_eq!(candidates[0].last_failure_class, None);
        assert!(!candidates[0].has_active_mr);
        assert_eq!(candidates[0].recommended_backend.as_deref(), Some("codex"));
    }

    #[test]
    fn scan_available_tickets_includes_open_github_issues() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\nRecommended model: Gemini 3.5 Flash (Medium)\n","labels":[]}]"#;
        let gh_path = bin_dir.join("gh");
        // Uses `printf '%s\n'` rather than `echo` -- dash's `echo` builtin
        // (the usual `/bin/sh` on Debian/Ubuntu) interprets `\n` inside a
        // single-quoted argument as an actual newline by default, which
        // corrupts the embedded JSON string content (a raw control
        // character where a `\n` escape sequence should stay literal).
        // `printf '%s'` never reinterprets escapes inside its argument.
        fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&gh_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&gh_path, perms).unwrap();
        }
        let _guard = PathGuard::set(&bin_dir);

        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = "github".to_string();
        prof.repo = "owner/repo".to_string();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].ticket_path, "118");
        assert_eq!(
            candidates[0].work_id.as_deref(),
            Some("TICKET-101-fail-closed-version-drift")
        );
        assert_eq!(candidates[0].recommended_backend.as_deref(), Some("agy"));
        assert_eq!(candidates[0].prior_attempt_count, 0);
        assert!(!candidates[0].has_active_mr);
    }

    #[test]
    fn scan_available_tickets_excludes_issue_already_archived_locally() {
        // Regression: migrating docs/tickets/*.md to native issues (#46)
        // doesn't close the issue when the local file is later archived to
        // docs/tickets/closed/ -- the open issue must not resurrect as
        // available work just because its markdown twin is done.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\n","labels":[]}]"#;
        let gh_path = bin_dir.join("gh");
        fs::write(
            &gh_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&gh_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&gh_path, perms).unwrap();
        }
        let _guard = PathGuard::set(&bin_dir);

        let closed_dir = tmp.path().join("docs/tickets/closed");
        fs::create_dir_all(&closed_dir).unwrap();
        fs::write(
            closed_dir.join("TICKET-101-fail-closed-version-drift.md"),
            "# TICKET-101: Fail closed\n\nGoal: test\n",
        )
        .unwrap();

        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = "github".to_string();
        prof.repo = "owner/repo".to_string();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert!(
            candidates.is_empty(),
            "expected locally-archived TICKET-101 issue to be excluded, got {candidates:?}"
        );
    }

    #[test]
    fn scan_available_tickets_reports_failed_history_with_no_active_mr() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-201-test.md"),
            "# TICKET-201: Test ticket\n\nGoal: test\n",
        )
        .unwrap();
        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        let mut entry = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        entry.work_id = Some("TICKET-201".into());
        entry.set_failure(
            crate::ledger::FailureClass::AgentNoProgress,
            crate::ledger::FailureStage::PostValidation,
        );
        crate::ledger::append(&cfg, &entry).unwrap();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].prior_attempt_count, 1);
        assert_eq!(
            candidates[0].last_failure_class.as_deref(),
            Some("agent_no_progress")
        );
        assert!(!candidates[0].has_active_mr);
    }

    #[test]
    fn scan_available_tickets_excludes_ticket_with_active_mr() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-202-test.md"),
            "# TICKET-202: Test ticket\n\nGoal: test\n",
        )
        .unwrap();
        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        let mut entry = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        entry.work_id = Some("TICKET-202".into());
        entry.branch = Some("gah/repo-1".into());
        crate::ledger::append(&cfg, &entry).unwrap();

        let mrs = vec![crate::sync::SyncMr {
            title: "[GAH] Fix: TICKET-202".into(),
            body: None,
            branch: "gah/repo-1".into(),
            labels: vec![],
            url: Some("https://example/pull/1".into()),
            id: Some("1".into()),
            state: Some("OPEN".into()),
            draft: false,
            merge_status: None,
            merged: false,
            updated_at: None,
            ci_failed: false,
            ci_passed: false,
            ci_pending: false,
            work_id: Some("TICKET-202".into()),
        }];

        let candidates = scan_available_tickets(
            &prof,
            &mrs,
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].has_active_mr);
    }

    #[test]
    fn scan_available_tickets_excludes_ticket_completed_via_merged_mr() {
        // Regression: a ticket that failed once, then succeeded and got its MR
        // merged, must not keep poisoning the queue via its old failure count.
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-090-test.md"),
            "# TICKET-090: Test ticket\n\nGoal: test\n",
        )
        .unwrap();
        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        let mut failed_entry = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        failed_entry.work_id = Some("TICKET-090".into());
        failed_entry.branch = Some("gah/repo-1".into());
        failed_entry.set_failure(
            crate::ledger::FailureClass::AgentNoProgress,
            crate::ledger::FailureStage::PostValidation,
        );
        crate::ledger::append(&cfg, &failed_entry).unwrap();

        let mut merged_entry = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        merged_entry.work_id = Some("TICKET-090".into());
        merged_entry.branch = Some("gah/repo-2".into());
        crate::ledger::append(&cfg, &merged_entry).unwrap();

        let mrs = vec![crate::sync::SyncMr {
            title: "[GAH] Fix: TICKET-090".into(),
            body: None,
            branch: "gah/repo-2".into(),
            labels: vec![],
            url: Some("https://example/pull/45".into()),
            id: Some("45".into()),
            state: Some("MERGED".into()),
            draft: false,
            merge_status: None,
            merged: true,
            updated_at: None,
            ci_failed: false,
            ci_passed: false,
            ci_pending: false,
            work_id: Some("TICKET-090".into()),
        }];

        let candidates = scan_available_tickets(
            &prof,
            &mrs,
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert!(
            candidates.is_empty(),
            "ticket completed via merged MR must be excluded entirely, got {candidates:?}"
        );
    }

    #[test]
    fn scan_available_tickets_ignores_ledger_entries_from_a_different_repo() {
        // Regression: the ledger is one global file shared by every profile,
        // and work_id is just a heading-derived string like "TICKET-090" with
        // no repo namespace. A totally unrelated repo's failed/merged history
        // for the same literal work_id must not poison this repo's ticket.
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-090-test.md"),
            "# TICKET-090: Test ticket\n\nGoal: test\n",
        )
        .unwrap();
        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        let mut prof = profile(tmp.path());
        prof.repo_id = "worldcup-props".into();
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        let mut other_repo_prof = profile(tmp.path());
        other_repo_prof.repo_id = "gah".into();
        other_repo_prof.provider = String::new();

        let mut failed_entry =
            LedgerEntry::new("test", &other_repo_prof, "codex", "fix", "x", None, None);
        failed_entry.work_id = Some("TICKET-090".into());
        failed_entry.set_failure(
            crate::ledger::FailureClass::AgentNoProgress,
            crate::ledger::FailureStage::PostValidation,
        );
        crate::ledger::append(&cfg, &failed_entry).unwrap();

        let mut second_entry =
            LedgerEntry::new("test", &other_repo_prof, "codex", "fix", "y", None, None);
        second_entry.work_id = Some("TICKET-090".into());
        crate::ledger::append(&cfg, &second_entry).unwrap();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].prior_attempt_count, 0,
            "another repo's ledger entries for the same literal work_id must not count here"
        );
        assert!(!candidates[0].has_active_mr);
    }

    #[test]
    fn scan_available_tickets_uses_preloaded_ledger_index_for_multiple_tickets() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-210-first.md"),
            "# TICKET-210: First ticket\n\nGoal: test\n",
        )
        .unwrap();
        fs::write(
            ticket_dir.join("TICKET-211-second.md"),
            "# TICKET-211: Second ticket\n\nGoal: test\n",
        )
        .unwrap();

        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        let mut first = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        first.work_id = Some("TICKET-210".into());
        first.set_failure(
            crate::ledger::FailureClass::AgentNoProgress,
            crate::ledger::FailureStage::PostValidation,
        );

        let mut second = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        second.work_id = Some("TICKET-211".into());
        second.branch = Some("gah/repo-211".into());

        let index = crate::ledger::index_entries_by_work_id(&[first, second]);
        let mrs = vec![crate::sync::SyncMr {
            title: "[GAH] Fix: TICKET-211".into(),
            body: None,
            branch: "gah/repo-211".into(),
            labels: vec![],
            url: Some("https://example/pull/211".into()),
            id: Some("211".into()),
            state: Some("OPEN".into()),
            draft: false,
            merge_status: None,
            merged: false,
            updated_at: None,
            ci_failed: false,
            ci_passed: false,
            ci_pending: false,
            work_id: Some("TICKET-211".into()),
        }];

        let candidates = scan_available_tickets(&prof, &mrs, &index);
        assert_eq!(candidates.len(), 2);
        let first = candidates
            .iter()
            .find(|candidate| candidate.work_id.as_deref() == Some("TICKET-210"))
            .unwrap();
        assert_eq!(first.prior_attempt_count, 1);
        assert_eq!(
            first.last_failure_class.as_deref(),
            Some("agent_no_progress")
        );
        assert!(!first.has_active_mr);
        let second = candidates
            .iter()
            .find(|candidate| candidate.work_id.as_deref() == Some("TICKET-211"))
            .unwrap();
        assert_eq!(second.prior_attempt_count, 1);
        assert!(second.has_active_mr);
    }

    // Issue #95: a tombstone entry (mode="clear_attempts") resets the
    // prior_attempt_count and genuine_agent_failure_count for its work_id.
    #[test]
    fn clear_attempts_tombstone_resets_ticket_count() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-300-test.md"),
            "# TICKET-300: Test\n\nGoal: test\n",
        )
        .unwrap();

        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        // 3 infra failures before the tombstone
        let mut e1 = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        e1.work_id = Some("TICKET-300".into());
        e1.failure_class = Some("backend_error".into());
        let mut e2 = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        e2.work_id = Some("TICKET-300".into());
        e2.failure_class = Some("environment_error".into());
        let mut e3 = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        e3.work_id = Some("TICKET-300".into());
        e3.failure_class = Some("backend_error".into());

        // Tombstone
        let tombstone = LedgerEntry::new_clear_attempts("test", &prof, "TICKET-300");

        let index = crate::ledger::index_entries_by_work_id(&[e1, e2, e3, tombstone]);
        let candidates = scan_available_tickets(&prof, &[], &index);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].prior_attempt_count, 0,
            "tombstone should reset prior_attempt_count to 0"
        );
        assert_eq!(
            candidates[0].genuine_agent_failure_count, 0,
            "tombstone should reset genuine_agent_failure_count to 0"
        );
    }

    // Issue #95: entries after a tombstone DO count.
    #[test]
    fn entries_after_tombstone_still_count() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-301-test.md"),
            "# TICKET-301: Test\n\nGoal: test\n",
        )
        .unwrap();

        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        // Pre-tombstone failures
        let mut e1 = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        e1.work_id = Some("TICKET-301".into());
        e1.failure_class = Some("agent_no_progress".into());

        // Tombstone
        let tombstone = LedgerEntry::new_clear_attempts("test", &prof, "TICKET-301");

        // Post-tombstone failure
        let mut e2 = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        e2.work_id = Some("TICKET-301".into());
        e2.failure_class = Some("backend_error".into());

        let index = crate::ledger::index_entries_by_work_id(&[e1, tombstone, e2]);
        let candidates = scan_available_tickets(&prof, &[], &index);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].prior_attempt_count, 1,
            "only the post-tombstone entry should count"
        );
        assert_eq!(
            candidates[0].genuine_agent_failure_count, 0,
            "post-tombstone entry is infra failure, not agent"
        );
    }

    // Issue #95: infra failures don't count toward genuine_agent_failure_count
    #[test]
    fn infra_failures_not_counted_as_agent_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-302-test.md"),
            "# TICKET-302: Test\n\nGoal: test\n",
        )
        .unwrap();

        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        let mut e1 = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        e1.work_id = Some("TICKET-302".into());
        e1.failure_class = Some("backend_error".into());
        let mut e2 = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        e2.work_id = Some("TICKET-302".into());
        e2.failure_class = Some("environment_error".into());
        let mut e3 = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        e3.work_id = Some("TICKET-302".into());
        e3.failure_class = Some("harness_error".into());

        let index = crate::ledger::index_entries_by_work_id(&[e1, e2, e3]);
        let candidates = scan_available_tickets(&prof, &[], &index);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].prior_attempt_count, 3,
            "all 3 entries should count in prior_attempt_count"
        );
        assert_eq!(
            candidates[0].genuine_agent_failure_count, 0,
            "none are genuine agent failures"
        );
    }

    #[test]
    fn duplicate_work_error_detection_is_typed_not_string_matched() {
        let err = anyhow::Error::new(super::DuplicateWorkError {
            work_id: "TICKET-999".into(),
            branch: Some("gah/repo-999".into()),
            mr_url: Some("https://example/pull/999".into()),
        })
        .context("outer wording changed completely");

        let duplicate = super::duplicate_work_error(&err).unwrap();
        assert_eq!(duplicate.work_id, "TICKET-999");
        assert_eq!(duplicate.branch.as_deref(), Some("gah/repo-999"));
        assert_eq!(
            duplicate.mr_url.as_deref(),
            Some("https://example/pull/999")
        );
    }

    fn init_repo(repo: &Path) {
        fs::create_dir_all(repo.join("docs/tickets")).unwrap();
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(repo)
            .output()
            .unwrap();
        fs::write(repo.join("README.md"), "hi\n").unwrap();
        Command::new("git")
            .args(["add", "README.md"])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(repo)
            .output()
            .unwrap();
    }

    fn make_fake_bin(dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[test]
    fn agy_second_backend_runs_with_agy_second_home_override() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let home_capture = tmp.path().join("captured-home.txt");

        let fake_agy = bin_dir.join("agy");
        fs::write(
            &fake_agy,
            format!(
                "#!/bin/sh\necho \"$HOME\" > {}\nexit 0\n",
                home_capture.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&fake_agy).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&fake_agy, perms).unwrap();
        }

        let mut prof = profile(tmp.path());
        prof.agy_path = Some(fake_agy.display().to_string());
        prof.agy_second_home = Some("/tmp/second-account-home".to_string());

        let session_dir = tmp.path().join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let llm = crate::runner::LlmConfig {
            base_url: String::new(),
            api_key: String::new(),
            model: "Gemini 3.5 Flash (Medium)".to_string(),
        };

        run_backend(
            "agy-second",
            &prof,
            tmp.path(),
            "do the thing",
            &session_dir,
            &llm,
            None,
            None,
        )
        .unwrap();

        let captured = fs::read_to_string(&home_capture).unwrap();
        assert_eq!(captured.trim(), "/tmp/second-account-home");
    }

    #[test]
    fn agy_backend_without_second_home_uses_real_home() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let home_capture = tmp.path().join("captured-home.txt");

        let fake_agy = bin_dir.join("agy");
        fs::write(
            &fake_agy,
            format!(
                "#!/bin/sh\necho \"$HOME\" > {}\nexit 0\n",
                home_capture.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&fake_agy).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&fake_agy, perms).unwrap();
        }

        let mut prof = profile(tmp.path());
        prof.agy_path = Some(fake_agy.display().to_string());
        prof.agy_second_home = Some("/tmp/second-account-home".to_string());

        let session_dir = tmp.path().join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let llm = crate::runner::LlmConfig {
            base_url: String::new(),
            api_key: String::new(),
            model: "Gemini 3.5 Flash (Medium)".to_string(),
        };

        run_backend(
            "agy",
            &prof,
            tmp.path(),
            "do the thing",
            &session_dir,
            &llm,
            None,
            None,
        )
        .unwrap();

        let captured = fs::read_to_string(&home_capture).unwrap();
        assert_ne!(captured.trim(), "/tmp/second-account-home");
    }

    #[test]
    fn run_backend_looks_up_agy_print_timeout_by_exact_model_name() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let argv_capture = tmp.path().join("captured-argv.txt");

        let fake_agy = bin_dir.join("agy");
        fs::write(
            &fake_agy,
            format!(
                "#!/bin/sh\necho \"$@\" > {}\nexit 0\n",
                argv_capture.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&fake_agy).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&fake_agy, perms).unwrap();
        }

        let mut prof = profile(tmp.path());
        prof.agy_path = Some(fake_agy.display().to_string());
        prof.agy_print_timeout_seconds
            .insert("Gemini 3.5 Flash (Medium)".to_string(), 900);
        prof.agy_print_timeout_seconds
            .insert("Gemini 3.1 Pro (High)".to_string(), 300);

        let session_dir = tmp.path().join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let llm = crate::runner::LlmConfig {
            base_url: String::new(),
            api_key: String::new(),
            model: "Gemini 3.5 Flash (Medium)".to_string(),
        };

        run_backend(
            "agy",
            &prof,
            tmp.path(),
            "do the thing",
            &session_dir,
            &llm,
            None,
            None,
        )
        .unwrap();

        let captured = fs::read_to_string(&argv_capture).unwrap();
        assert!(captured.contains("--print-timeout 900s"), "got: {captured}");
    }

    #[test]
    fn run_backend_omits_print_timeout_for_unmapped_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let argv_capture = tmp.path().join("captured-argv.txt");

        let fake_agy = bin_dir.join("agy");
        fs::write(
            &fake_agy,
            format!(
                "#!/bin/sh\necho \"$@\" > {}\nexit 0\n",
                argv_capture.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&fake_agy).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&fake_agy, perms).unwrap();
        }

        let mut prof = profile(tmp.path());
        prof.agy_path = Some(fake_agy.display().to_string());
        prof.agy_print_timeout_seconds
            .insert("Gemini 3.5 Flash (Medium)".to_string(), 900);

        let session_dir = tmp.path().join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let llm = crate::runner::LlmConfig {
            base_url: String::new(),
            api_key: String::new(),
            model: "Gemini 3.1 Pro (High)".to_string(), // not in the map
        };

        run_backend(
            "agy",
            &prof,
            tmp.path(),
            "do the thing",
            &session_dir,
            &llm,
            None,
            None,
        )
        .unwrap();

        let captured = fs::read_to_string(&argv_capture).unwrap();
        assert!(!captured.contains("--print-timeout"), "got: {captured}");
    }

    #[test]
    fn run_backend_routes_vibe_to_run_vibe_not_the_openhands_fallthrough() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        // Regression: run_backend's match had a catch-all `_ => run_openhands(...)`.
        // An unrecognized backend name silently ran openhands instead of
        // erroring -- adding "vibe" without an explicit match arm would have
        // silently spent real OpenHands API $ on every "vibe" dispatch instead
        // of running vibe at all.
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let marker = tmp.path().join("which-backend-ran.txt");

        let fake_vibe = bin_dir.join("vibe");
        fs::write(
            &fake_vibe,
            format!("#!/bin/sh\necho vibe > {}\nexit 0\n", marker.display()),
        )
        .unwrap();
        fs::write(
            bin_dir.join("openhands"),
            format!("#!/bin/sh\necho openhands > {}\nexit 0\n", marker.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for bin in ["vibe", "openhands"] {
                let path = bin_dir.join(bin);
                let mut perms = fs::metadata(&path).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&path, perms).unwrap();
            }
        }

        let mut prof = profile(tmp.path());
        prof.vibe_path = Some(fake_vibe.display().to_string());

        let session_dir = tmp.path().join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let llm = crate::runner::LlmConfig {
            base_url: String::new(),
            api_key: String::new(),
            model: "unused-for-vibe".to_string(),
        };

        run_backend(
            "vibe",
            &prof,
            tmp.path(),
            "do the thing",
            &session_dir,
            &llm,
            None,
            None,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&marker).unwrap().trim(), "vibe");
    }

    #[test]
    fn build_task_includes_manager_memory_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        // profile.local_path == tmp.path() (the main checkout) -- manager
        // memory must be read from there, not the worktree, so it's live
        // operational state rather than something pinned to a git ref.
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/MANAGER_MEMORY.md"),
            "Use .venv/bin/python, do not pip install from scratch.\n",
        )
        .unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();

        let task = build_task(&prof, &wt, "improve", "some ticket text", None);

        assert!(task.contains("Manager Memory"));
        assert!(task.contains("Use .venv/bin/python, do not pip install from scratch."));
        // Manager Memory must come before Focus so the agent reads
        // environment/project context before the specific task.
        let memory_pos = task.find("Manager Memory").unwrap();
        let focus_pos = task.find("## Focus").unwrap();
        assert!(memory_pos < focus_pos);
    }

    #[test]
    fn build_task_omits_manager_memory_section_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let prof = profile(tmp.path());

        // Empty target -- the "implement ONLY..." instruction (which itself
        // mentions "Manager Memory" by name) only applies when a target is
        // given, so this isolates the file-injection behavior specifically.
        let task = build_task(&prof, &wt, "improve", "", None);

        assert!(!task.contains("## Manager Memory"));
    }

    #[test]
    fn improve_mode_with_a_target_says_ignore_other_backlog_items() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let prof = profile(tmp.path());

        let task = build_task(&prof, &wt, "improve", "TICKET-014: boost shots ROI", None);

        assert!(task.contains("Implement ONLY the specific ticket"));
        assert!(!task.contains("Select and implement the highest-priority"));
    }

    #[test]
    fn improve_mode_without_a_target_still_picks_from_backlog() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let prof = profile(tmp.path());

        let task = build_task(&prof, &wt, "improve", "", None);

        assert!(task.contains("Select and implement the highest-priority"));
    }

    #[test]
    fn apply_route_to_ledger_records_effective_model() {
        let tmp = tempfile::tempdir().unwrap();
        let mut entry = LedgerEntry::new(
            "test",
            &profile(tmp.path()),
            "codex",
            "improve",
            "target",
            Some("session-1".into()),
            None,
        );
        let route = RouteDecision {
            requested_backend: "auto".into(),
            effective_backend: "codex".into(),
            requested_model: None,
            effective_model: Some("claude-sonnet-4".into()),
            effective_quota_pool: None,
            routing_reason: "ticket recommendation".into(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        };

        apply_route_to_ledger(&mut entry, &route);

        assert_eq!(entry.effective_model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(entry.effective_backend, "codex");
        assert_eq!(
            entry.routing_reason.as_deref(),
            Some("ticket recommendation")
        );
    }

    #[test]
    fn apply_route_to_ledger_leaves_null_when_no_model() {
        let tmp = tempfile::tempdir().unwrap();
        let mut entry = LedgerEntry::new(
            "test",
            &profile(tmp.path()),
            "codex",
            "fix",
            "target",
            Some("session-1".into()),
            None,
        );
        let route = RouteDecision {
            requested_backend: "auto".into(),
            effective_backend: "openhands".into(),
            requested_model: None,
            effective_model: None,
            effective_quota_pool: None,
            routing_reason: "profile routing policy".into(),
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
        };

        apply_route_to_ledger(&mut entry, &route);

        assert_eq!(entry.effective_model, None);
        assert_eq!(entry.effective_backend, "openhands");
    }

    #[test]
    fn preflight_uses_profile_executable_override() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let claude_path = make_fake_bin(&bin_dir, "claude-explicit");
        let git_path = make_fake_bin(&bin_dir, "git");
        let _guard = PathGuard::set(git_path.parent().unwrap());

        let mut profile = profile(tmp.path());
        profile.claude_path = Some(claude_path.display().to_string());

        let result = preflight(&profile, "claude");

        assert!(result.is_ok());
    }

    #[test]
    fn ticket_summaries_include_filename_and_heading() {
        let tmp = tempfile::tempdir().unwrap();
        let tickets = tmp.path().join("docs/tickets");
        fs::create_dir_all(&tickets).unwrap();
        fs::write(tickets.join("TICKET-001-fix.md"), "# Fix login\nbody\n").unwrap();

        assert_eq!(
            collect_ticket_summaries(&tickets).unwrap(),
            vec!["- TICKET-001-fix.md: Fix login"]
        );
    }

    #[test]
    fn backend_failure_fixture_marks_unavailability() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("availability.json");
        let parsed = mark_backend_unavailable_from_output_at(
            &state,
            "codex",
            Some("local/test"),
            None,
            CODEX_FULL_RESET,
            "/tmp/backend-output.log",
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            parsed.kind,
            crate::quota_parser::FailureKind::QuotaExhausted
        );
        let state = load_state(&state).unwrap();
        assert_eq!(state.records.len(), 1);
        assert_eq!(state.records[0].backend, "codex");
        assert_eq!(state.records[0].model.as_deref(), Some("local/test"));
        assert_eq!(state.records[0].reason, Reason::QuotaExhausted);
        assert!(state.records[0].unavailable_until.is_some());
    }

    #[test]
    fn unrecognized_backend_failure_does_not_invent_unavailability() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("availability.json");
        let parsed = mark_backend_unavailable_from_output_at(
            &state,
            "codex",
            Some("local/test"),
            None,
            "plain old crash with no quota language",
            "/tmp/backend-output.log",
        )
        .unwrap();

        assert!(parsed.is_none());
        let decision = availability_for(
            &state,
            "codex",
            Some("local/test"),
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert!(decision.eligible);
    }

    #[test]
    fn backend_failure_reset_time_resolves_in_local_offset_not_utc() {
        // Live-observed bug: a Codex reset message with a bare "9:01 PM"
        // (no timezone) was resolved as if it were UTC, so on this
        // UTC-5 host a ~3am local reset displayed as "~14h remaining"
        // instead of already having passed. now_with_local_offset() must
        // supply the host's real offset so "9:01 PM" means 9:01 PM local
        // time, not 9:01 PM UTC.
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("availability.json");
        mark_backend_unavailable_from_output_at(
            &state,
            "codex",
            Some("local/test"),
            None,
            CODEX_FULL_RESET,
            "/tmp/backend-output.log",
        )
        .unwrap()
        .unwrap();

        let state = load_state(&state).unwrap();
        let unavailable_until = state.records[0].unavailable_until.as_deref().unwrap();
        let resolved = OffsetDateTime::parse(
            unavailable_until,
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let local_offset_seconds = chrono::Local::now().offset().local_minus_utc();
        let local_offset = time::UtcOffset::from_whole_seconds(local_offset_seconds).unwrap();
        let in_local = resolved.to_offset(local_offset);

        // The fixture says "9:01 PM" -- that must be the LOCAL hour/minute
        // regardless of what the host's offset actually is.
        assert_eq!(in_local.hour(), 21);
        assert_eq!(in_local.minute(), 1);
    }

    #[test]
    fn pm_preflight_requires_manager_memory() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let err = collect_pm_preflight(&profile(tmp.path()), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("PM mode requires manager memory"));
    }

    #[test]
    fn pm_task_includes_preflight_context_and_rules() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/MANAGER_MEMORY.md"),
            "# Memory\nRemember open work.\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("docs/tickets/TICKET-002-auth.md"),
            "# Fix push auth\n",
        )
        .unwrap();

        let ctx = collect_pm_preflight(&profile(tmp.path()), tmp.path()).unwrap();
        let task = build_pm_plan_task(&profile(tmp.path()), &ctx, "Fix push auth").unwrap();
        assert!(task.contains("## Preflight Context"));
        assert!(task.contains("Remember open work."));
        assert!(task.contains("TICKET-002-auth.md: Fix push auth"));
        assert!(task.contains("Default action is to avoid creating new tickets."));
        assert!(task.contains("### Repo State"));
        assert!(task.contains("Current branch:"));
        assert!(task.contains("Recent commits:"));
    }

    #[test]
    fn first_heading_skips_non_headings() {
        assert_eq!(
            first_markdown_heading("intro\n## Heading\n"),
            Some("Heading")
        );
    }

    #[test]
    fn parse_pm_plan_extracts_json_from_log() {
        let plan =
            parse_pm_plan("noise\n{\"title\":\"T\",\"summary\":\"S\",\"tickets\":[]}\n").unwrap();
        assert_eq!(plan.title, "T");
    }

    #[test]
    fn validation_failure_matching_baseline_is_classified_separately() {
        let progress =
            classify_validation_failure_progress(Some("same failure"), None, "same failure");
        assert_eq!(progress, ValidationFailureProgress::UnchangedFromBaseline);
        assert!(progress.unchanged_from_baseline());
        assert!(!progress.unchanged_from_previous_attempt());
    }

    #[test]
    fn validation_failure_matching_previous_attempt_is_classified_separately() {
        let progress = classify_validation_failure_progress(
            Some("baseline failure"),
            Some("same failure"),
            "same failure",
        );
        assert_eq!(
            progress,
            ValidationFailureProgress::UnchangedFromPreviousAttempt
        );
        assert!(!progress.unchanged_from_baseline());
        assert!(progress.unchanged_from_previous_attempt());
    }

    #[test]
    fn validation_failure_matching_both_baseline_and_previous_is_distinct() {
        let progress = classify_validation_failure_progress(
            Some("same failure"),
            Some("same failure"),
            "same failure",
        );
        assert_eq!(
            progress,
            ValidationFailureProgress::UnchangedFromBaselineAndPreviousAttempt
        );
        assert!(progress.unchanged_from_baseline());
        assert!(progress.unchanged_from_previous_attempt());
    }

    #[test]
    fn validation_failure_changes_are_not_misclassified() {
        let progress = classify_validation_failure_progress(
            Some("baseline failure"),
            Some("previous failure"),
            "new failure",
        );
        assert_eq!(progress, ValidationFailureProgress::Changed);
        assert!(!progress.unchanged_from_baseline());
        assert!(!progress.unchanged_from_previous_attempt());
    }

    #[test]
    fn validation_failure_reasons_explain_baseline_vs_previous_attempt() {
        assert!(validation_failure_no_progress_reason(
            ValidationFailureProgress::UnchangedFromBaseline
        )
        .unwrap()
        .contains("pristine-tree baseline"));
        assert!(validation_failure_no_progress_reason(
            ValidationFailureProgress::UnchangedFromPreviousAttempt
        )
        .unwrap()
        .contains("previous attempt"));
    }

    #[test]
    fn apply_pm_plan_skips_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        fs::create_dir_all(repo.join("docs/tickets")).unwrap();
        let ctx = super::PmPreflight {
            rendered: String::new(),
            existing_tickets: vec!["- TICKET-001-fix.md: Fix login".into()],
            open_mrs: String::new(),
            merged_mrs: String::new(),
        };
        let plan: PmPlan = serde_json::from_str(
            r#"{"title":"Plan","summary":"Summary","tickets":[
                {"title":"Fix login","summary":"dup","difficulty":"easy","risk":"low","recommended_backend":null,"duplicate_evidence":[],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"uncovered_reason":"x"},
                {"title":"Fix auth","summary":"new","difficulty":"easy","risk":"low","recommended_backend":null,"duplicate_evidence":[],"affected_files":["a"],"acceptance_criteria":["b"],"verification_commands":["pytest"],"uncovered_reason":"x"}
            ]}"#,
        )
        .unwrap();

        let written = apply_pm_plan(repo, &ctx, &plan).unwrap();
        assert_eq!(written.len(), 1);
        assert!(written[0].display().to_string().contains("fix-auth"));
    }

    #[test]
    fn next_ticket_id_avoids_collision_with_manager_memory_reservation() {
        // TICKET-091 AC6/7: a ticket ID reserved only in manager memory
        // prose (no docs/tickets/ file yet) must not be reused -- this is
        // exactly how the TICKET-102/103/104 collisions happened.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let tickets_dir = repo.join("docs/tickets");
        fs::create_dir_all(&tickets_dir).unwrap();
        fs::write(tickets_dir.join("TICKET-005-old.md"), "old").unwrap();
        fs::write(
            repo.join("docs/MANAGER_MEMORY.md"),
            "## TICKET-042 -- reserved but not yet filed\n\nStatus: TODO\n",
        )
        .unwrap();

        let id = next_ticket_id(&tickets_dir, Some(&repo.join("docs/MANAGER_MEMORY.md"))).unwrap();
        assert_eq!(id, 43, "must skip past the memory-reserved TICKET-042");
    }

    #[test]
    fn parses_ticket_metadata_for_routing() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-058-descriptive-mr-titles.md");
        fs::write(
            &ticket,
            "# TICKET-058: Descriptive MR Titles\n\nDifficulty: hard\nRisk: high\nRecommended backend: codex\nRecommended model: gpt-x\n\n## Affected Files\n- src/auth.rs\n\n## Verification Commands\n- `pytest tests/test_auth.py -x`\n",
        )
        .unwrap();
        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-058"));
        assert_eq!(meta.title.as_deref(), Some("Descriptive MR Titles"));
        assert_eq!(meta.recommended_backend.as_deref(), Some("codex"));
        assert_eq!(meta.recommended_model.as_deref(), Some("gpt-x"));
        assert_eq!(meta.difficulty.as_deref(), Some("hard"));
        assert_eq!(meta.risk.as_deref(), Some("high"));
        assert_eq!(
            meta.verification_commands,
            vec!["pytest tests/test_auth.py -x"]
        );
    }

    #[test]
    fn parses_structured_ticket_sections_into_typed_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-092-structured-work-metadata.md");
        fs::write(
            &ticket,
            "# TICKET-092: Structured work metadata\n\n\
Goal: Represent task metadata as typed structured fields rather than prompt parsing.\n\n\
Difficulty: medium\n\
Risk: medium\n\
Recommended backend: codex\n\
Recommended model: gpt-5.4\n\
Source: docs/tickets/TICKET-092-structured-work-metadata.md\n\n\
## Problem\n\
The parser should retain structured sections.\n\n\
## Acceptance Criteria\n\
- Define a single structured metadata type\n\
- Missing fields handled explicitly\n\n\
## Constraints\n\
- Do not require a new file format\n\
- No database\n\n\
## Affected Files\n\
- src/dispatch.rs\n\
- src/models.rs\n\n\
## Verification Commands\n\
- `cargo fmt --check`\n\
- `cargo test`\n",
        )
        .unwrap();

        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-092"));
        assert_eq!(meta.work_id.as_deref(), Some("TICKET-092"));
        assert_eq!(meta.summary.as_deref(), Some("Structured work metadata"));
        assert_eq!(
            meta.problem.as_deref(),
            Some("The parser should retain structured sections.")
        );
        assert_eq!(
            meta.acceptance_criteria,
            vec![
                "Define a single structured metadata type",
                "Missing fields handled explicitly"
            ]
        );
        assert_eq!(
            meta.constraints,
            vec!["Do not require a new file format", "No database"]
        );
        assert_eq!(
            meta.affected_files,
            vec!["src/dispatch.rs", "src/models.rs"]
        );
        assert_eq!(
            meta.verification_commands,
            vec!["cargo fmt --check", "cargo test"]
        );
        assert_eq!(
            meta.source.as_deref(),
            Some("docs/tickets/TICKET-092-structured-work-metadata.md")
        );
    }

    #[test]
    fn parses_ticket_metadata_preserves_colons_in_normal_headings() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-104-auth-hardening.md");
        fs::write(
            &ticket,
            "# Auth: reject empty token\n\nDifficulty: medium\nRisk: low\n",
        )
        .unwrap();

        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-104"));
        assert_eq!(meta.title.as_deref(), Some("Auth: reject empty token"));
    }

    #[test]
    fn parses_ticket_metadata_strips_ticket_prefix_from_heading_title() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket = tmp.path().join("TICKET-105-heading-title.md");
        fs::write(&ticket, "# TICKET-105: Keep title intact\n").unwrap();

        let meta = parse_ticket_metadata(&ticket).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-105"));
        assert_eq!(meta.title.as_deref(), Some("Keep title intact"));
    }

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
    fn mr_title_collision_detection_prevents_stale_id_in_title() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let tickets_dir = repo.join("docs/tickets");
        fs::create_dir_all(&tickets_dir).unwrap();

        // Write MANAGER_MEMORY.md with TICKET-074 mapped to baseline disposition.
        // Must be a "| TICKET-N | ... |" table row -- that's the only format
        // parse_ticket_metadata treats as a real status claim (a prose aside
        // that merely mentions the ID isn't a staleness signal).
        fs::write(
            repo.join("docs/MANAGER_MEMORY.md"),
            "| TICKET-074 | P1 | FIX | MERGED | Baseline disposition classifier |\n",
        )
        .unwrap();

        // Write ticket file with filename TICKET-074-fix...md but heading has a different title
        let ticket_path = tickets_dir.join("TICKET-074-fix-closed-unmerged-classification.md");
        fs::write(
            &ticket_path,
            "# TICKET-074: Fix closed unmerged MR classification\n\nGoal: Treat closed unmerged MRs as terminal\n",
        )
        .unwrap();

        // Parse should detect collision and set is_authoritative to false
        let meta = parse_ticket_metadata(&ticket_path).unwrap().unwrap();
        assert!(!meta.is_authoritative);

        // Since it's not authoritative, the MR title should NOT contain the ID "TICKET-074"
        let title = build_mr_title("fix", "real", false, Some(&meta));
        assert_eq!(title, "[GAH] Fix: Fix closed unmerged MR classification");
        assert!(!title.contains("TICKET-074"));
    }

    #[test]
    fn parse_ticket_metadata_ignores_incidental_manager_memory_prose_mentions() {
        // Regression: MANAGER_MEMORY.md prose that merely cross-references a
        // ticket ID (an ordering note, a categorization aside) is not a status
        // claim and must not invalidate an otherwise-consistent ticket file,
        // even if the wording doesn't literally repeat the ticket's title.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let tickets_dir = repo.join("docs/tickets");
        fs::create_dir_all(&tickets_dir).unwrap();

        fs::write(
            repo.join("docs/MANAGER_MEMORY.md"),
            "- **TICKET-114 is a serving-integrity control**\n\
             - **TICKET-110 before TICKET-112**\n",
        )
        .unwrap();

        let ticket_path = tickets_dir.join("TICKET-114-artifact-load-integrity.md");
        fs::write(
            &ticket_path,
            "# TICKET-114 — Artifact load integrity verification\n\nGoal: test\n",
        )
        .unwrap();

        let meta = parse_ticket_metadata(&ticket_path).unwrap().unwrap();
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-114"));
        assert_eq!(meta.work_id.as_deref(), Some("TICKET-114"));
        assert!(meta.is_authoritative);
    }

    #[test]
    fn is_issue_number_reference_recognizes_plain_numbers() {
        assert!(is_issue_number_reference("42"));
        assert!(is_issue_number_reference("123"));
        assert!(!is_issue_number_reference("abc"));
        assert!(!is_issue_number_reference(""));
        assert!(!is_issue_number_reference("42abc"));
    }

    #[test]
    fn is_issue_number_reference_recognizes_hash_numbers() {
        assert!(is_issue_number_reference("#42"));
        assert!(is_issue_number_reference("#123"));
        assert!(!is_issue_number_reference("#"));
        assert!(!is_issue_number_reference("#abc"));
        // Spaces are trimmed, so this should be recognized
        assert!(is_issue_number_reference(" #42 "));
    }

    #[test]
    fn extract_issue_number_from_plain_number() {
        assert_eq!(extract_issue_number("42"), Some("42".to_string()));
        assert_eq!(extract_issue_number("123"), Some("123".to_string()));
        assert_eq!(extract_issue_number("abc"), None);
        assert_eq!(extract_issue_number(""), None);
    }

    #[test]
    fn extract_issue_number_from_hash_number() {
        assert_eq!(extract_issue_number("#42"), Some("42".to_string()));
        assert_eq!(extract_issue_number("#123"), Some("123".to_string()));
        assert_eq!(extract_issue_number("#"), None);
        assert_eq!(extract_issue_number("#abc"), None);
    }

    #[test]
    fn format_issue_for_focus_formats_correctly() {
        let issue = IssueDetails {
            number: "42".to_string(),
            title: "Test Issue".to_string(),
            body: "This is the issue body".to_string(),
            labels: vec!["bug".to_string(), "enhancement".to_string()],
        };

        let result = format_issue_for_focus(&issue);
        assert!(result.contains("# Issue #42: Test Issue"));
        assert!(result.contains("This is the issue body"));
        assert!(result.contains("Labels: bug, enhancement"));
    }

    #[test]
    fn parse_ticket_metadata_from_issue_extracts_basic_fields() {
        let issue = IssueDetails {
            number: "42".to_string(),
            title: "TICKET-42: Fix the bug".to_string(),
            body:
                "## Problem\n\nSomething is broken\n\n## Acceptance Criteria\n\n- Fix the issue\n- Add tests"
                    .to_string(),
            labels: vec!["bug".to_string()],
        };

        let meta = parse_ticket_metadata_from_issue(&issue);
        assert_eq!(meta.ticket_id.as_deref(), Some("TICKET-42"));
        assert_eq!(meta.work_id.as_deref(), Some("TICKET-42"));
        assert_eq!(meta.issue_number.as_deref(), Some("42"));
        assert!(meta.is_authoritative);
        assert!(meta
            .acceptance_criteria
            .contains(&"Fix the issue".to_string()));
        assert!(meta.acceptance_criteria.contains(&"Add tests".to_string()));
    }

    #[test]
    fn parse_ticket_metadata_from_issue_handles_metadata_fields() {
        let issue = IssueDetails {
            number: "42".to_string(),
            title: "Test Issue".to_string(),
            body: "Difficulty: High\nRisk: Medium\nRecommended backend: agy\nGoal: Fix everything"
                .to_string(),
            labels: vec![],
        };

        let meta = parse_ticket_metadata_from_issue(&issue);
        assert_eq!(meta.difficulty.as_deref(), Some("High"));
        assert_eq!(meta.risk.as_deref(), Some("Medium"));
        assert_eq!(meta.recommended_backend.as_deref(), Some("agy"));
        assert_eq!(meta.goal.as_deref(), Some("Fix everything"));
    }

    #[test]
    fn render_review_comment_includes_non_blocking_findings_and_risk_notes() {
        // Regression: a verdict with zero blocking_findings (e.g. APPROVE_WEAK)
        // still carries real substance in these two fields. The posted PR
        // comment was silently dropping both, leaving reviewers with nothing
        // but a bare verdict/confidence line and no actual feedback.
        let verdict: crate::models::ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE_WEAK","confidence":"0.78","human_required":true,
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
    fn published_review_verdict_strips_internal_tier() {
        // Issue #129 Bug B: the reviewer may return APPROVE_STRONG /
        // APPROVE_WEAK as an internal routing tier. That tier must NOT leak
        // into the human-facing review status -- humans see a strict
        // APPROVE / REJECT. The STRONG/WEAK strength stays internal and only
        // drives auto-merge eligibility.
        assert_eq!(published_review_verdict("APPROVE_STRONG"), "APPROVE");
        assert_eq!(published_review_verdict("APPROVE_WEAK"), "APPROVE");
        assert_eq!(published_review_verdict("REJECT"), "REJECT");
        // NEEDS_FIX / HUMAN_REVIEW carry no tier, published as-is.
        assert_eq!(published_review_verdict("NEEDS_FIX"), "NEEDS_FIX");
        assert_eq!(published_review_verdict("HUMAN_REVIEW"), "HUMAN_REVIEW");
    }

    #[test]
    fn render_review_comment_publishes_approve_not_internal_tier() {
        // Issue #129 Bug B: the posted MR body must show APPROVE, never the
        // internal APPROVE_STRONG / APPROVE_WEAK routing tier.
        let strong: crate::models::ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false,
                "blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#,
        )
        .unwrap();
        let weak: crate::models::ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE_WEAK","confidence":"medium","human_required":true,
                "blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#,
        )
        .unwrap();
        let s = render_review_comment(&strong, Path::new("/tmp/session"));
        let w = render_review_comment(&weak, Path::new("/tmp/session"));
        assert!(s.contains("GAH review verdict: `APPROVE`"));
        assert!(!s.contains("APPROVE_STRONG"));
        assert!(w.contains("GAH review verdict: `APPROVE`"));
        assert!(!w.contains("APPROVE_WEAK"));
    }

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
    fn authoritative_ticket_metadata_populates_ledger_work_identity() {
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-095".into()),
            work_id: Some("TICKET-095".into()),
            title: Some("Ledger work identity propagation".into()),
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

        apply_authoritative_work_identity(&mut ledger, Some(&ticket), "gah/real-123");

        assert_eq!(ledger.work_id.as_deref(), Some("TICKET-095"));
        assert_eq!(
            ledger.work_title.as_deref(),
            Some("Ledger work identity propagation")
        );
    }

    #[test]
    fn non_authoritative_ticket_metadata_falls_back_to_synthetic_work_id() {
        // TICKET-091 AC4: no authoritative external ticket -> generate an
        // internal ID (the branch name) rather than leaving work_id unset.
        let ticket = TicketMetadata {
            ticket_id: Some("TICKET-095".into()),
            work_id: Some("TICKET-095".into()),
            title: Some("Ledger work identity propagation".into()),
            is_authoritative: false,
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

        apply_authoritative_work_identity(&mut ledger, Some(&ticket), "gah/real-123");

        assert_eq!(ledger.work_id.as_deref(), Some("gah/real-123"));
        assert_eq!(ledger.work_title, None);
    }

    #[test]
    fn no_ticket_falls_back_to_synthetic_work_id() {
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

        apply_authoritative_work_identity(&mut ledger, None, "gah/real-456");

        assert_eq!(ledger.work_id.as_deref(), Some("gah/real-456"));
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
        ledger.attempts_started = 2;
        ledger.attempts_completed = 2;
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
    fn experiment_mr_body_includes_judge_and_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ledger = LedgerEntry::new(
            "real",
            &profile(tmp.path()),
            "codex",
            "experiment",
            "target",
            Some("session-1".into()),
            None,
        );
        ledger.files_changed = Some(1);
        ledger.insertions = Some(8);
        ledger.deletions = Some(0);

        let backend_summary = "Generated research findings report.";
        let ctx = ExperimentMrRenderContext {
            backend: "codex",
            model: "gpt-5.4",
            artifact_count: 3,
            answered: false,
            backend_summary,
        };
        let body = build_experiment_mr_body(&ctx);

        assert!(body.contains("## Experiment Result"));
        assert!(body.contains("Judge verdict: partial"));
        assert!(body.contains("Artifacts: 3"));
        assert!(body.contains("## What changed and why"));
        assert!(body.contains("Generated research findings report."));
        assert!(!body.contains("## Changes"));
        assert!(!body.contains("## Branch"));
    }

    #[test]
    fn validate_runs_shell_syntax() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        // `cd x && y` requires a shell — this was silently impossible before
        let cmds = vec!["cd sub && true".to_string()];
        assert!(validate(&cmds, tmp.path()).is_ok());
    }

    #[test]
    fn validate_reports_failing_command_output() {
        let tmp = tempfile::tempdir().unwrap();
        let cmds = vec!["echo oops >&2 && false".to_string()];
        let err = validate(&cmds, tmp.path()).unwrap_err();
        assert!(format!("{:#}", err).contains("oops"));
    }

    #[test]
    fn run_auto_fix_commands_actually_fixes_the_worktree() {
        // The whole point: a formatter run here should mean a subsequent
        // validate() with a --check-style command passes, instead of
        // burning an LLM retry on pure whitespace.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "unformatted\n").unwrap();
        let fix_cmds = vec!["printf 'fixed\\n' > f.txt".to_string()];
        run_auto_fix_commands(&fix_cmds, tmp.path());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(),
            "fixed\n"
        );
    }

    #[test]
    fn run_auto_fix_commands_swallows_a_failing_command() {
        // A formatter that isn't installed, or that errors on this
        // particular tree, must never abort the dispatch -- it's a
        // best-effort convenience, not a validation gate.
        let tmp = tempfile::tempdir().unwrap();
        let cmds = vec!["exit 1".to_string()];
        run_auto_fix_commands(&cmds, tmp.path()); // must not panic
    }

    fn setup_fake_gh(bin_dir: &Path, response_json: &str) {
        let gh_path = bin_dir.join("gh");
        let content = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then\n\
                 echo '{}'\n\
             fi\n",
            response_json.replace('\'', "'\\''")
        );
        fs::write(&gh_path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&gh_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&gh_path, perms).unwrap();
        }
    }

    #[test]
    fn test_check_duplicate_work_cases() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        // 1. Create a fake ticket markdown
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        let ticket_path = ticket_dir.join("TICKET-097-test.md");
        fs::write(
            &ticket_path,
            "# TICKET-097: Test ticket\n\n\
             Goal: Test duplicate work guard\n\n\
             ## Problem\n\
             Test\n",
        )
        .unwrap();

        // 2. Setup config & profile
        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };

        let mut prof = profile(tmp.path());
        prof.provider = "github".to_string();
        prof.repo = "owner/repo".to_string();

        let ledger_path = tmp.path().join("ledger.jsonl");

        // 3. Case A: No previous work -> Should pass
        let args = super::DispatchArgs {
            profile: "test".to_string(),
            mode: "improve".to_string(),
            backend: "codex".to_string(),
            target: ticket_path.display().to_string(),
            branch: None,
            mr: None,
            current_branch: false,
            budget: 0,
            dry_run: false,
            config_path: None,
            oh_profile: None,
            model: None,
            retries: 0,
            allow_draft_fail: false,
            prod: false,
            allow_unknown_red_baseline: false,
            escalate: false,
            existing_branch: None,
            skip_validation_gate: false,
            dispatch_reason: None,
        };

        // No ledger exists yet.
        let res = super::check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());

        // 4. Case B: Active open PR exists -> Should block
        let pr_json = r#"[{"title":"Fix login","headRefName":"gah/repo-active","url":"https://github.com/owner/repo/pull/1","labels":[],"number":1,"state":"OPEN","isDraft":false,"mergeStateStatus":"CLEAN","mergedAt":null,"updatedAt":"2026-07-04T17:22:35-05:00","statusCheckRollup":[]}]"#;
        setup_fake_gh(&bin_dir, pr_json);
        let _guard = PathGuard::set(&bin_dir);

        // Write ledger entry matching the ticket and branch
        let mut entry = LedgerEntry::new(
            "test",
            &prof,
            "codex",
            "improve",
            &ticket_path.display().to_string(),
            Some("session-1".into()),
            None,
        );
        entry.work_id = Some("TICKET-097".to_string());
        entry.branch = Some("gah/repo-active".to_string());
        entry.mr_url = Some("https://github.com/owner/repo/pull/1".to_string());
        entry.timestamp = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();

        let ledger_line = serde_json::to_string(&entry).unwrap();
        fs::write(&ledger_path, format!("{}\n", ledger_line)).unwrap();

        let res = super::check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_err());
        let err = res.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("Refusing dispatch: active open PR already exists"));
        let duplicate = super::duplicate_work_error(&err).unwrap();
        assert_eq!(duplicate.work_id, "TICKET-097");
        assert_eq!(
            duplicate.mr_url.as_deref(),
            Some("https://github.com/owner/repo/pull/1")
        );

        // 5. Case C: PR is merged -> Should pass
        let pr_json_merged = r#"[{"title":"Fix login","headRefName":"gah/repo-active","url":"https://github.com/owner/repo/pull/1","labels":[],"number":1,"state":"MERGED","isDraft":false,"mergeStateStatus":"CLEAN","mergedAt":"2026-07-04T17:22:35-05:00","updatedAt":"2026-07-04T17:22:35-05:00","statusCheckRollup":[]}]"#;
        setup_fake_gh(&bin_dir, pr_json_merged);

        let res = super::check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());

        // 6. Case D: PR is closed unmerged -> Should pass
        let pr_json_closed = r#"[{"title":"Fix login","headRefName":"gah/repo-active","url":"https://github.com/owner/repo/pull/1","labels":[],"number":1,"state":"CLOSED","isDraft":false,"mergeStateStatus":"CLEAN","mergedAt":null,"updatedAt":"2026-07-04T17:22:35-05:00","statusCheckRollup":[]}]"#;
        setup_fake_gh(&bin_dir, pr_json_closed);

        let res = super::check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());

        // 7. Case E: Ledger entry is stale (> 14 days) -> Should pass
        setup_fake_gh(&bin_dir, pr_json);
        entry.timestamp = (OffsetDateTime::now_utc() - time::Duration::days(15))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let ledger_line_stale = serde_json::to_string(&entry).unwrap();
        fs::write(&ledger_path, format!("{}\n", ledger_line_stale)).unwrap();

        let res = super::check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_ok());

        // 8. Case F: Active branch may own work -> Warn
        setup_fake_gh(&bin_dir, "[]");
        let local_repo_path = tmp.path().join("local_repo");
        fs::create_dir_all(&local_repo_path).unwrap();
        init_repo(&local_repo_path);
        Command::new("git")
            .args(["branch", "gah/repo-active"])
            .current_dir(&local_repo_path)
            .output()
            .unwrap();
        let mut prof_with_repo = prof.clone();
        prof_with_repo.local_path = local_repo_path.display().to_string();

        entry.timestamp = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let ledger_line_active_branch = serde_json::to_string(&entry).unwrap();
        fs::write(&ledger_path, format!("{}\n", ledger_line_active_branch)).unwrap();

        let res = super::check_duplicate_work(&cfg, &prof_with_repo, &args);
        assert!(res.is_ok());
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

/// TICKET-128: emit deterministic, machine-readable human-handoff metadata when
/// a profile's publishing policy forbids agent-authored repository messaging
/// (PR/MR creation or LLM-generated commit text). No PR/MR is created and no
/// tracker comment is posted; the worktree/branch is left for a human to
/// complete. The output is intentionally free of any LLM-generated prose.
fn emit_human_handoff(profile: &Profile, ledger: &LedgerEntry, branch: &str, reason: &str) {
    println!("=== GAH human handoff (publishing policy) ===");
    println!("reason: {}", reason);
    println!("profile: {}", profile.display_name);
    println!("branch: {}", branch);
    println!(
        "validation_status: {}",
        ledger.validation_result.as_deref().unwrap_or("unknown")
    );
    println!("changed_files: {}", ledger.files_changed.unwrap_or(0));
    if let Some(verdict) = &ledger.review_verdict {
        println!("review_verdict: {}", verdict);
    }
    println!("=== end GAH human handoff ===");
}

/// TICKET-128: whether the profile may publish the work autonomously. A
/// restricted profile that forbids PR/MR creation OR LLM-generated commit
/// messages must stop at a deterministic human handoff instead of publishing:
/// there is nothing to push without a commit, and an empty/uncommitted branch
/// cannot seed a PR. Each flag is an independent policy axis; neither is
/// overloaded onto `human_required`.
fn publishing_allows_publish(profile: &Profile) -> bool {
    profile.publishing.allow_pull_request_creation
        && profile.publishing.allow_commit_message_generation
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
    routing::decide(
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
    )
    .ok()
}

fn classify_validation_failure_progress(
    baseline_failure: Option<&str>,
    previous_failure: Option<&str>,
    current_failure: &str,
) -> ValidationFailureProgress {
    let same_as_baseline = baseline_failure == Some(current_failure);
    let same_as_previous = previous_failure == Some(current_failure);
    match (same_as_baseline, same_as_previous) {
        (true, true) => ValidationFailureProgress::UnchangedFromBaselineAndPreviousAttempt,
        (true, false) => ValidationFailureProgress::UnchangedFromBaseline,
        (false, true) => ValidationFailureProgress::UnchangedFromPreviousAttempt,
        (false, false) => ValidationFailureProgress::Changed,
    }
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
            prior_state: None,
        }),
        Err(_) => Ok(ReviewTarget {
            mr_id: None,
            mr_url: None,
            mr_title: None,
            mr_body: None,
            ci_status: None,
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
struct PmPreflight {
    rendered: String,
    existing_tickets: Vec<String>,
    open_mrs: String,
    merged_mrs: String,
}

#[derive(Debug, Clone)]
struct ReviewTarget {
    mr_id: Option<String>,
    mr_url: Option<String>,
    mr_title: Option<String>,
    mr_body: Option<String>,
    ci_status: Option<String>,
    source_branch: String,
    target_branch: String,
    prior_state: Option<String>,
}

#[derive(Debug, Clone)]
struct ReviewDiffBundle {
    diff: String,
    files: String,
}

fn parse_pm_plan(log_text: &str) -> Result<PmPlan> {
    let json = extract_first_json_object(log_text)
        .ok_or_else(|| anyhow::anyhow!("PM planner did not return valid JSON"))?;
    let plan = serde_json::from_str::<PmPlan>(&json)?;
    if plan.title.trim().is_empty() || plan.summary.trim().is_empty() {
        anyhow::bail!("PM plan missing title or summary");
    }
    Ok(plan)
}

fn extract_first_json_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        if bytes[start] != b'{' {
            continue;
        }
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        for end in start..bytes.len() {
            let ch = bytes[end] as char;
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let candidate = &text[start..=end];
                        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                            return Some(candidate.to_string());
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn apply_pm_plan(repo: &Path, ctx: &PmPreflight, plan: &PmPlan) -> Result<Vec<PathBuf>> {
    let tickets_dir = repo.join("docs/tickets");
    fs::create_dir_all(&tickets_dir)?;
    let manager_memory_path = repo.join("docs/MANAGER_MEMORY.md");
    let next_id = next_ticket_id(&tickets_dir, Some(&manager_memory_path))?;
    let mut written = vec![];
    let mut id = next_id;
    for ticket in &plan.tickets {
        if should_skip_ticket(ctx, ticket) {
            continue;
        }
        validate_ticket(ticket)?;
        let slug = slugify(ticket.title.as_deref().unwrap_or(""));
        let filename = format!("TICKET-{:03}-{}.md", id, slug);
        let path = tickets_dir.join(filename);
        fs::write(&path, render_ticket(ticket, id))?;
        written.push(path);
        id += 1;
    }
    Ok(written)
}

fn should_skip_ticket(ctx: &PmPreflight, ticket: &WorkMetadata) -> bool {
    let title = normalize_match(ticket.title.as_deref().unwrap_or(""));
    if title.is_empty() {
        return true;
    }
    ctx.existing_tickets
        .iter()
        .any(|item| normalize_match(item).contains(&title))
        || normalize_match(&ctx.open_mrs).contains(&title)
        || normalize_match(&ctx.merged_mrs).contains(&title)
}

fn validate_ticket(ticket: &WorkMetadata) -> Result<()> {
    let title = ticket.title.as_deref().unwrap_or("");
    if title.trim().is_empty() || ticket.summary.as_deref().unwrap_or("").trim().is_empty() {
        anyhow::bail!("ticket missing title or summary");
    }
    if !matches!(
        ticket.difficulty.as_deref(),
        Some("easy" | "medium" | "hard")
    ) {
        anyhow::bail!("ticket '{}' has invalid difficulty", title);
    }
    if !matches!(ticket.risk.as_deref(), Some("low" | "medium" | "high")) {
        anyhow::bail!("ticket '{}' has invalid risk", title);
    }
    if ticket.acceptance_criteria.is_empty() || ticket.verification_commands.is_empty() {
        anyhow::bail!("ticket '{}' missing acceptance or verification", title);
    }
    Ok(())
}

fn render_ticket(ticket: &WorkMetadata, id: usize) -> String {
    let mut out = format!(
        "# TICKET-{id:03}: {title}\n\n\
Goal: {summary}\n\n\
Difficulty: {difficulty}\n\
Risk: {risk}\n\
Recommended backend: {backend}\n\n\
## Why This Is Uncovered\n{reason}\n\n\
## Affected Files\n",
        id = id,
        title = ticket.title.as_deref().unwrap_or(""),
        summary = ticket.summary.as_deref().unwrap_or(""),
        difficulty = ticket.difficulty.as_deref().unwrap_or(""),
        risk = ticket.risk.as_deref().unwrap_or(""),
        backend = ticket
            .recommended_backend
            .as_deref()
            .unwrap_or("unspecified"),
        reason = ticket.uncovered_reason.as_deref().unwrap_or(""),
    );
    for file in &ticket.affected_files {
        out.push_str(&format!("- {}\n", file));
    }
    if !ticket.duplicate_evidence.is_empty() {
        out.push_str("\n## Duplicate Evidence Considered\n");
        for item in &ticket.duplicate_evidence {
            out.push_str(&format!("- {}\n", item));
        }
    }
    out.push_str("\n## Acceptance Criteria\n");
    for item in &ticket.acceptance_criteria {
        out.push_str(&format!("- {}\n", item));
    }
    out.push_str("\n## Verification Commands\n");
    for cmd in &ticket.verification_commands {
        out.push_str(&format!("- `{}`\n", cmd));
    }
    out
}

/// TICKET-091 AC6/7: a new ticket ID must not collide with one already
/// reserved in manager memory prose, even if no file exists yet for it
/// (this is exactly how the TICKET-102/103/104 collisions happened).
fn next_ticket_id(tickets_dir: &Path, manager_memory_path: Option<&Path>) -> Result<usize> {
    let mut max_id = 0usize;
    if tickets_dir.exists() {
        for entry in fs::read_dir(tickets_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix("TICKET-") {
                if let Some((num, _)) = rest.split_once('-') {
                    max_id = max_id.max(num.parse::<usize>().unwrap_or(0));
                }
            }
        }
    }
    if let Some(path) = manager_memory_path {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            for id in scan_ticket_ids(&content) {
                max_id = max_id.max(id);
            }
        }
    }
    Ok(max_id + 1)
}

fn scan_ticket_ids(text: &str) -> Vec<usize> {
    text.split("TICKET-")
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<usize>().ok()
        })
        .collect()
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn normalize_match(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn apply_route_to_ledger(ledger: &mut LedgerEntry, route: &RouteDecision) {
    ledger.backend = route.effective_backend.clone();
    ledger.requested_backend = route.requested_backend.clone();
    ledger.effective_backend = route.effective_backend.clone();
    ledger.requested_model = route.requested_model.clone();
    ledger.effective_model = route.effective_model.clone();
    ledger.routing_reason = Some(route.routing_reason.clone());
    ledger.fallback_used = route.fallback_used;
    ledger.confidence_impact = route.confidence_impact.clone();
    ledger.human_required = route.human_required;
    ledger.routing_diagnostics = route.routing_diagnostics.clone();
}

fn decide_route(
    cfg: &GahConfig,
    profile: &Profile,
    req: RouteRequest<'_>,
    ledger: &mut LedgerEntry,
) -> Result<RouteDecision> {
    match routing::decide(&cfg.defaults, profile, req) {
        Ok(route) => Ok(route),
        Err(err) => {
            if err.downcast_ref::<RouteError>().is_some() {
                ledger.set_failure(
                    crate::ledger::FailureClass::HumanBlocked,
                    crate::ledger::FailureStage::Route,
                );
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

fn route_identity(backend: &str, model: Option<&str>) -> String {
    format!("{backend}\u{0}{}", model.unwrap_or(""))
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

fn mark_backend_unavailable_from_output(
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
    log_text: &str,
    log_path: &str,
) -> Result<Option<crate::quota_parser::ParsedFailure>> {
    mark_backend_unavailable_from_output_at(
        &crate::availability::resolve_state_path(),
        backend,
        model,
        quota_pool,
        log_text,
        log_path,
    )
}

fn mark_backend_unavailable_from_output_at(
    state_path: &Path,
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
    log_text: &str,
    log_path: &str,
) -> Result<Option<crate::quota_parser::ParsedFailure>> {
    let now = now_with_local_offset();
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
    };
    let summary = format!(
        "{}; confidence={:?}; log={}",
        parsed.matched_evidence, parsed.confidence, log_path
    );
    crate::availability::record_unavailable(
        state_path,
        backend,
        model.filter(|m| !m.is_empty()),
        quota_pool,
        reason,
        crate::availability::Source::BackendError,
        unavailable_until,
        Some(summary),
        now,
    )?;
    Ok(Some(parsed))
}

type TicketMetadata = WorkMetadata;

fn parse_ticket_metadata(path: &Path) -> Result<Option<TicketMetadata>> {
    if path.extension().and_then(|e| e.to_str()) != Some("md") || !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(path)?;
    let ticket_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| {
            let mut parts = stem.split('-');
            match (parts.next(), parts.next()) {
                (Some("TICKET"), Some(number)) if !number.is_empty() => {
                    Some(format!("TICKET-{number}"))
                }
                _ => None,
            }
        });
    let raw_heading = first_markdown_heading(&body);
    let mut work_id_from_heading = None;
    if let Some(heading) = raw_heading {
        let trimmed = heading.trim();
        if trimmed.starts_with("TICKET-") {
            // Tickets are titled either "TICKET-N: Title" or "TICKET-N — Title"
            // (em dash, no colon) -- both are in real use across this repo's
            // own ticket backlog, so both must be recognized or the em-dash
            // style silently fails is_authoritative and never gets dispatched.
            if let Some((id, _)) = trimmed
                .split_once(':')
                .or_else(|| trimmed.split_once(" — "))
            {
                work_id_from_heading = Some(id.trim().to_string());
            }
        }
    }
    let title = raw_heading.map(|title| normalize_ticket_title(title.into()));
    let mut meta = TicketMetadata {
        ticket_id,
        title,
        ..TicketMetadata::default()
    };
    meta.summary = meta.title.clone();
    meta.problem = extract_markdown_section(&body, "Problem");
    meta.acceptance_criteria = extract_markdown_list_section(&body, "Acceptance Criteria");
    meta.constraints = extract_markdown_list_section(&body, "Constraints");
    meta.verification_commands = extract_markdown_code_list_section(&body, "Verification Commands");
    meta.affected_files = extract_markdown_list_section(&body, "Affected Files");
    meta.source = extract_field_value(&body, "Source")
        .or_else(|| extract_markdown_section(&body, "Source"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    for line in body.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Difficulty:") {
            meta.difficulty = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Risk:") {
            meta.risk = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Recommended backend:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_backend = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Recommended model:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_model = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Goal:") {
            let value = value.trim();
            if !value.is_empty() {
                meta.goal = Some(value.to_string());
            }
            if meta.title.is_none() && !value.is_empty() {
                meta.title = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Suggested MR Title:") {
            meta.suggested_mr_title = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Work ID:") {
            meta.work_id = Some(value.trim().to_string());
        }
    }
    if meta.work_id.is_none() {
        meta.work_id = work_id_from_heading;
    }

    let mut is_authoritative = false;
    if let Some(ref file_id) = meta.ticket_id {
        if let Some(ref cont_id) = meta.work_id {
            if file_id == cont_id {
                is_authoritative = true;
            }
        }
    }
    if is_authoritative {
        let repo_dir = path.parent().and_then(|p| p.parent());
        let manager_memory_path = repo_dir.map(|p| p.join("MANAGER_MEMORY.md"));
        if let Some(ref p) = manager_memory_path {
            if p.exists() {
                if let Ok(content) = fs::read_to_string(p) {
                    let file_id = meta.ticket_id.as_ref().unwrap();
                    for line in content.lines() {
                        // Only a "| TICKET-N | ... |" status-table row is a real
                        // status claim about this ticket -- any other prose
                        // mention (a cross-reference, an ordering note, a
                        // one-off aside) isn't a staleness signal and shouldn't
                        // be able to invalidate the ticket file over wording.
                        let is_table_row = line.trim_start().starts_with('|');
                        if is_table_row && line.contains(file_id) {
                            if let Some(ref title) = meta.title {
                                let norm_line = normalize_match(line);
                                let norm_title = normalize_match(title);
                                if !norm_line.contains(&norm_title) {
                                    is_authoritative = false;
                                    break;
                                }
                            } else {
                                is_authoritative = false;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    meta.is_authoritative = is_authoritative;

    Ok(Some(meta))
}

fn extract_markdown_section(body: &str, heading: &str) -> Option<String> {
    let mut capture = false;
    let mut lines = Vec::new();
    for raw_line in body.lines() {
        let trimmed = raw_line.trim();
        if trimmed.starts_with('#') {
            let normalized = trimmed.trim_start_matches('#').trim();
            if capture {
                break;
            }
            capture = normalized.eq_ignore_ascii_case(heading);
            continue;
        }
        if capture {
            lines.push(raw_line.trim_end().to_string());
        }
    }
    let section = lines.join("\n").trim().to_string();
    if section.is_empty() {
        None
    } else {
        Some(section)
    }
}

fn extract_markdown_list_section(body: &str, heading: &str) -> Vec<String> {
    extract_markdown_section(body, heading)
        .map(|section| {
            section
                .lines()
                .map(str::trim)
                .filter_map(|line| {
                    line.strip_prefix("- ")
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_markdown_code_list_section(body: &str, heading: &str) -> Vec<String> {
    extract_markdown_list_section(body, heading)
        .into_iter()
        .map(|item| {
            item.strip_prefix('`')
                .and_then(|value| value.strip_suffix('`'))
                .unwrap_or(item.as_str())
                .to_string()
        })
        .collect()
}

fn extract_field_value(body: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}:");
    body.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_ticket_title(title: String) -> String {
    let trimmed = title.trim();
    let Some(rest) = trimmed.strip_prefix("TICKET-") else {
        return title;
    };

    let digit_byte_count = rest
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, _)| i + 1) // +1 to include the last digit character
        .unwrap_or(0);
    if digit_byte_count == 0 {
        return title;
    }

    // ASCII digits are 1 byte each, so digit_byte_count should be a valid boundary
    // But use UTF-8 safe suffix to be extra safe
    let remainder = utf8_safe_suffix(rest, rest.len() - digit_byte_count).trim_start();
    let normalized = remainder
        .trim_start_matches([':', '-'])
        .trim_start()
        .to_string();

    if normalized.is_empty() {
        title
    } else {
        normalized
    }
}

fn render_ticket_label(ticket: Option<&TicketMetadata>) -> String {
    let Some(ticket) = ticket else {
        return "n/a".into();
    };
    match (ticket.ticket_id.as_deref(), ticket.title.as_deref()) {
        (Some(ticket_id), Some(title)) => format!("{ticket_id} {title}"),
        (Some(ticket_id), None) => ticket_id.to_string(),
        (None, Some(title)) => title.to_string(),
        (None, None) => "n/a".into(),
    }
}

/// Extract the backend summary from the tail of the backend output log.
/// This captures the backend's own final summary/reasoning.
/// Strips terminal rendering noise from a backend's raw log tail before it
/// goes into a PR/MR body. Confirmed live (MR !243, worldcup-props): the
/// openhands CLI's Rich-rendered final message panel (ANSI color codes,
/// box-drawing borders) and its unconditional exit banner ("Goodbye! 👋" /
/// "Conversation ID: ..." / "Hint: run openhands --resume ...") land in
/// backend-output.log outside the `--json` event stream and get grabbed
/// verbatim by the raw-tail extraction below -- landing in the PR body
/// looking like the model itself produced garbled text, when it's actually
/// terminal styling the extraction never stripped.
fn strip_terminal_noise(text: &str) -> String {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    let without_ansi = ansi.replace_all(text, "");

    without_ansi
        .lines()
        .map(|line| line.trim_matches(['│', '╭', '╮', '╰', '╯', '─', ' ']))
        .filter(|line| {
            !(line.is_empty()
                || *line == "Goodbye! 👋"
                || line.starts_with("Conversation ID:")
                // Terminal line-wrapping can split this hint across two
                // physical lines at an arbitrary column, so match on
                // substrings rather than a single fixed prefix.
                || line.contains("openhands --resume")
                || line.contains("resume this")
                || *line == "conversation.")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_backend_summary(log_path: &str) -> String {
    let log_text = fs::read_to_string(log_path).unwrap_or_default();
    // Take the last ~2000 characters to get the backend's final summary
    if log_text.is_empty() {
        String::new()
    } else {
        // Use UTF-8 safe suffix to avoid cutting in the middle of a character
        let tail = utf8_safe_suffix(&log_text, 2000);
        strip_terminal_noise(tail)
    }
}

fn format_validation_outcome(result: Option<&str>) -> &'static str {
    match result {
        Some("passed") => "passed",
        Some("failed-draft") => "failed, pushed as draft",
        Some("failed") => "failed",
        Some("not_run") => "not run",
        Some("answered") => "answered",
        Some("partial") => "partial",
        Some(_) => "recorded",
        None => "not recorded",
    }
}

fn format_failure_state(ledger: &LedgerEntry) -> Option<String> {
    if let Some(summary) = ledger.error_summary.as_deref() {
        let mut state = String::new();
        if let Some(class) = ledger.failure_class.as_deref() {
            state.push_str(&format!("Class: `{class}`\n"));
        }
        if let Some(stage) = ledger.failure_stage.as_deref() {
            state.push_str(&format!("Stage: `{stage}`\n"));
        }
        state.push_str(&format!("Summary: {}", summary.trim()));
        return Some(state);
    }
    if ledger.validation_result.as_deref() == Some("failed-draft") {
        return Some("Validation remained red and the change was pushed as a draft.".into());
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn build_standard_mr_body(
    mode: &str,
    ticket: Option<&TicketMetadata>,
    backend: &str,
    model: &str,
    _branch: &str,
    _target_branch: &str,
    validation_passed: bool,
    backend_summary: &str,
) -> String {
    let mut sections = vec![format!(
        "## GAH {} mode\n\nTicket: {}\nBackend/model: `{}` / `{}",
        mode,
        render_ticket_label(ticket),
        backend,
        model,
    )];

    // Add Closes directive for GitHub/GitLab auto-close if this came from an issue
    if let Some(ticket) = ticket {
        if let Some(ref issue_number) = ticket.issue_number {
            sections.push(format!("Closes #{}", issue_number));
        }
    }

    if !backend_summary.is_empty() {
        sections.push(format!("## What changed and why\n\n{}", backend_summary));
    }
    sections.push(format!(
        "Validation passed: {}\n\nGenerated by `gah dispatch`.",
        validation_passed,
    ));
    sections.join("\n\n")
}

struct MrRenderContext<'a> {
    backend: &'a str,
    model: &'a str,
    branch: &'a str,
    target_branch: &'a str,
    validation_commands: &'a [String],
    ledger: &'a LedgerEntry,
    backend_summary: &'a str,
}

fn build_metadata_rich_mr_body(
    mode: &str,
    ticket: &TicketMetadata,
    ctx: &MrRenderContext<'_>,
) -> String {
    let mut sections = Vec::new();

    let work_item = match (
        ticket.work_id.as_deref(),
        ticket.ticket_id.as_deref(),
        ticket.title.as_deref(),
    ) {
        (Some(id), _, Some(title)) => Some(format!("ID: `{id}`\nTitle: {title}")),
        (Some(id), _, None) => Some(format!("ID: `{id}`")),
        (None, Some(id), Some(title)) => Some(format!("ID: `{id}`\nTitle: {title}")),
        (None, Some(id), None) => Some(format!("ID: `{id}`")),
        (None, None, Some(title)) => Some(format!("Title: {title}")),
        (None, None, None) => None,
    };
    if let Some(section) = work_item {
        sections.push(format!("## Work Item\n\n{section}"));
    }

    // Add Closes directive for GitHub/GitLab auto-close if this came from an issue
    if let Some(ref issue_number) = ticket.issue_number {
        sections.push(format!("Closes #{}", issue_number));
    }

    if let Some(problem) = ticket.problem.as_deref() {
        sections.push(format!("## Problem\n\n{}", problem.trim()));
    }
    if let Some(goal) = ticket.goal.as_deref() {
        sections.push(format!("## Goal\n\n{}", goal.trim()));
    }
    if !ticket.acceptance_criteria.is_empty() {
        let lines = ticket
            .acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## Acceptance Criteria\n\n{lines}"));
    }
    if !ticket.constraints.is_empty() {
        let lines = ticket
            .constraints
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## Constraints\n\n{lines}"));
    }
    if !ctx.backend_summary.is_empty() {
        sections.push(format!(
            "## What changed and why\n\n{}",
            ctx.backend_summary
        ));
    }
    if !ctx.validation_commands.is_empty() || ctx.ledger.validation_result.is_some() {
        let mut lines = vec![format!(
            "Outcome: {}",
            format_validation_outcome(ctx.ledger.validation_result.as_deref())
        )];
        for cmd in ctx.validation_commands {
            lines.push(format!("- `{cmd}`"));
        }
        sections.push(format!("## Validation\n\n{}", lines.join("\n")));
    }

    sections.push(format!(
        "## Backend / Model\n\nBackend: `{}`\nModel: `{}`",
        ctx.backend, ctx.model
    ));
    sections.push(format!(
        "## Attempts\n\nStarted: {}\nCompleted: {}\nFallback used: {}",
        ctx.ledger.attempts_started,
        ctx.ledger.attempts_completed,
        if ctx.ledger.fallback_used {
            "yes"
        } else {
            "no"
        }
    ));

    if let Some(state) = format_failure_state(ctx.ledger) {
        sections.push(format!("## Failure / Stop State\n\n{state}"));
    }

    if let Some(source) = ticket.source.as_deref() {
        sections.push(format!("## Source\n\n{}", source.trim()));
    }

    sections.push(format!("Generated by `gah dispatch --mode {mode}`."));
    sections.join("\n\n")
}

fn build_fix_or_improve_mr_body(
    mode: &str,
    ticket: Option<&TicketMetadata>,
    ctx: &MrRenderContext<'_>,
    validation_passed: bool,
) -> String {
    match ticket {
        Some(ticket) => build_metadata_rich_mr_body(mode, ticket, ctx),
        None => build_standard_mr_body(
            mode,
            None,
            ctx.backend,
            ctx.model,
            ctx.branch,
            ctx.target_branch,
            validation_passed,
            ctx.backend_summary,
        ),
    }
}

struct ExperimentMrRenderContext<'a> {
    backend: &'a str,
    model: &'a str,
    artifact_count: usize,
    answered: bool,
    backend_summary: &'a str,
}

fn build_experiment_mr_body(ctx: &ExperimentMrRenderContext<'_>) -> String {
    let mut sections = vec![
        "## Experiment Result".to_string(),
        format!(
            "\nBackend: `{backend}`\nModel: `{model}`\nJudge verdict: {}\nArtifacts: {}\n",
            if ctx.answered { "answered" } else { "partial" },
            ctx.artifact_count,
            backend = ctx.backend,
            model = ctx.model
        ),
    ];
    if !ctx.backend_summary.is_empty() {
        sections.push(format!(
            "## What changed and why\n\n{}",
            ctx.backend_summary
        ));
    }
    sections.push("Generated by `gah dispatch --mode experiment`.".into());
    sections.join("\n\n")
}

fn truncate_title(title: &str, limit: usize) -> String {
    if title.len() <= limit {
        title.to_string()
    } else {
        let mut truncated = String::new();
        let mut char_count = 0;
        for c in title.chars() {
            if char_count < limit - 3 {
                truncated.push(c);
                char_count += 1;
            } else {
                break;
            }
        }
        truncated.push_str("...");
        truncated
    }
}

fn build_mr_title(
    mode: &str,
    repo_id: &str,
    validation_failed: bool,
    ticket: Option<&TicketMetadata>,
) -> String {
    let mode_label = match mode {
        "fix" => "Fix",
        "improve" => "Improve",
        "review" => "Review",
        "pm" => "Plan",
        other => other,
    };
    let prefix = if validation_failed {
        "[GAH][DRAFT-FAIL]"
    } else {
        "[GAH]"
    };
    let title_string = if let Some(ticket) = ticket {
        let title_text = ticket
            .suggested_mr_title
            .as_deref()
            .or(ticket.title.as_deref());
        if let Some(title) = title_text {
            let detail = if ticket.is_authoritative {
                if let Some(ref work_id) = ticket.work_id {
                    format!("{work_id} {title}")
                } else if let Some(ref ticket_id) = ticket.ticket_id {
                    format!("{ticket_id} {title}")
                } else {
                    title.to_string()
                }
            } else {
                title.to_string()
            };
            format!("{prefix} {mode_label}: {detail}")
        } else {
            format!("{prefix} {mode_label}: {repo_id}")
        }
    } else {
        format!("{prefix} {mode_label}: {repo_id}")
    };
    truncate_title(&title_string, 255)
}

/// TICKET-108: reviewer authority (who is reviewing) kept as a dimension
/// separate from review outcome (verdict/confidence, what they said).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewerTier {
    Strong,
    Standard,
    Weak,
}

impl ReviewerTier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Standard => "standard",
            Self::Weak => "weak",
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

fn parse_review_verdict(
    review_text: &str,
    route: &RouteDecision,
    parsed_usage: &crate::ledger::LedgerUsage,
    tier: ReviewerTier,
) -> Result<crate::models::ReviewVerdict> {
    let json = extract_first_json_object(review_text)
        .ok_or_else(|| anyhow::anyhow!("reviewer did not return verdict JSON"))?;
    let mut verdict = serde_json::from_str::<crate::models::ReviewVerdict>(&json)?;
    // Reviewer identity (tier) and review outcome (verdict text/confidence)
    // are separate dimensions -- the verdict text itself is never rewritten
    // based on who reviewed it (see review_labels for how tier affects
    // labeling instead).
    if tier == ReviewerTier::Weak {
        verdict.human_required = true;
        if verdict.confidence == "high" {
            verdict.confidence = "medium".into();
        }
    }
    if matches!(verdict.verdict.as_str(), "APPROVE_WEAK" | "HUMAN_REVIEW") {
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

/// Issue #129 / #128: the reviewer's own verdict vocabulary includes an
/// internal routing tier (`APPROVE_STRONG` / `APPROVE_WEAK`) that must never
/// be published verbatim to the repository. Humans and the issue tracker see
/// a strict `APPROVE` / `REJECT` status; the STRONG/WEAK strength stays
/// internal and only drives merge-policy / auto-merge eligibility in the
/// controller (see `review_labels` and `decide_next_action`).
pub(crate) fn published_review_verdict(verdict: &str) -> String {
    match verdict {
        "APPROVE_STRONG" | "APPROVE_WEAK" => "APPROVE".to_string(),
        "REJECT" => "REJECT".to_string(),
        // NEEDS_FIX / HUMAN_REVIEW carry no strength tier, so they are
        // published as-is.
        other => other.to_string(),
    }
}

fn render_review_comment(verdict: &crate::models::ReviewVerdict, session_dir: &Path) -> String {
    let published = published_review_verdict(&verdict.verdict);
    let mut out = format!(
        "GAH review verdict: `{}`\n\nConfidence: `{}`\nHuman required: `{}`\nReviewer: `{}` / `{}`\nArtifacts: `{}`\n",
        published,
        verdict.confidence,
        verdict.human_required,
        verdict.effective_backend.as_deref().unwrap_or("unknown"),
        verdict.effective_model.as_deref().unwrap_or("unknown"),
        session_dir.display(),
    );
    if !verdict.blocking_findings.is_empty() {
        out.push_str("\nBlocking findings:\n");
        for item in &verdict.blocking_findings {
            out.push_str(&format!("- {}\n", item));
        }
    }
    // A verdict with zero blocking findings (e.g. APPROVE_WEAK) still
    // carries real substance in these two fields -- dropping them left the
    // posted PR comment as a bare verdict line with no actual feedback.
    if !verdict.non_blocking_findings.is_empty() {
        out.push_str("\nNon-blocking findings:\n");
        for item in &verdict.non_blocking_findings {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if !verdict.risk_notes.is_empty() {
        out.push_str("\nRisk notes:\n");
        for item in &verdict.risk_notes {
            out.push_str(&format!("- {}\n", item));
        }
    }
    out
}

fn review_labels(verdict: &crate::models::ReviewVerdict) -> Vec<&'static str> {
    // TICKET-108: an APPROVE_STRONG verdict from a weak-tier reviewer still
    // needs human eyes -- reviewer identity and verdict text are combined
    // here, not conflated into a single rewritten string.
    let is_weak_tier = verdict.reviewer_tier.as_deref() == Some("weak");
    match verdict.verdict.as_str() {
        "APPROVE_STRONG" if is_weak_tier => vec!["gah-review-weak", "gah-human-review"],
        "APPROVE_STRONG" => vec!["gah-ready-for-human"],
        "APPROVE_WEAK" => vec!["gah-review-weak", "gah-human-review"],
        "NEEDS_FIX" | "REJECT" => vec!["gah-needs-fix"],
        "HUMAN_REVIEW" => vec!["gah-human-review"],
        _ => vec![],
    }
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

/// Details about an issue fetched from GitHub/GitLab
#[derive(Debug, Clone)]
struct IssueDetails {
    number: String,
    title: String,
    body: String,
    labels: Vec<String>,
}

/// Check if a string looks like an issue number (e.g., "42" or "#42")
fn is_issue_number_reference(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Check for "#42" format
    if let Some(number_part) = trimmed.strip_prefix('#') {
        return !number_part.is_empty() && number_part.chars().all(|c| c.is_ascii_digit());
    }

    // Check for plain number format
    trimmed.chars().all(|c| c.is_ascii_digit())
}

/// Extract issue number from a string that could be "42" or "#42"
fn extract_issue_number(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }

    let number_str = if let Some(number_part) = trimmed.strip_prefix('#') {
        if number_part.is_empty() {
            return None;
        }
        number_part
    } else {
        trimmed
    };

    if number_str.chars().all(|c| c.is_ascii_digit()) {
        Some(number_str.to_string())
    } else {
        None
    }
}

/// Fetch issue details from GitHub using gh CLI
fn fetch_github_issue(profile: &Profile, issue_number: &str) -> Result<IssueDetails> {
    let out = provider_command("gh")
        .arg("issue")
        .arg("view")
        .arg(issue_number)
        .arg("--repo")
        .arg(&profile.repo)
        .arg("--json")
        .arg("title,body,labels")
        .output()
        .context("gh issue view")?;

    if !out.status.success() {
        anyhow::bail!(
            "gh issue view failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let resp: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing GitHub issue response")?;

    let number = resp["number"]
        .as_i64()
        .map(|n| n.to_string())
        .unwrap_or_else(|| issue_number.to_string());

    let title = resp["title"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Issue #{}", issue_number));

    let body = resp["body"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();

    let labels = resp["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Ok(IssueDetails {
        number,
        title,
        body,
        labels,
    })
}

/// Fetch issue details from GitLab using glab CLI
fn fetch_gitlab_issue(profile: &Profile, issue_number: &str) -> Result<IssueDetails> {
    let out = provider_command("glab")
        .arg("issue")
        .arg("view")
        .arg(issue_number)
        .arg("--repo")
        .arg(&profile.repo)
        .arg("-F")
        .arg("json")
        .output()
        .context("glab issue view")?;

    if !out.status.success() {
        anyhow::bail!(
            "glab issue view failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let resp: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing GitLab issue response")?;

    let number = resp["iid"]
        .as_i64()
        .map(|n| n.to_string())
        .unwrap_or_else(|| issue_number.to_string());

    let title = resp["title"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Issue #{}", issue_number));

    let body = resp["description"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();

    let labels = resp["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Ok(IssueDetails {
        number,
        title,
        body,
        labels,
    })
}

/// List open issues from GitHub using gh CLI.
fn list_open_github_issues(profile: &Profile) -> Result<Vec<IssueDetails>> {
    let out = provider_command("gh")
        .arg("issue")
        .arg("list")
        .arg("--repo")
        .arg(&profile.repo)
        .arg("--state")
        .arg("open")
        .arg("--json")
        .arg("number,title,body,labels")
        .arg("--limit")
        .arg("1000")
        .output()
        .context("gh issue list")?;

    if !out.status.success() {
        anyhow::bail!(
            "gh issue list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let items: Vec<serde_json::Value> =
        serde_json::from_slice(&out.stdout).context("parsing GitHub issue list response")?;

    Ok(items
        .into_iter()
        .map(|resp| {
            let number = resp["number"]
                .as_i64()
                .map(|n| n.to_string())
                .unwrap_or_default();
            let title = resp["title"].as_str().unwrap_or_default().to_string();
            let body = resp["body"].as_str().unwrap_or_default().to_string();
            let labels = resp["labels"]
                .as_array()
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|label| label["name"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            IssueDetails {
                number,
                title,
                body,
                labels,
            }
        })
        .collect())
}

/// List open issues from GitLab using glab CLI. Paginates until a
/// short page confirms there's nothing left -- a single 100-item page
/// on a backlog this large would silently truncate the scan.
fn list_open_gitlab_issues(profile: &Profile) -> Result<Vec<IssueDetails>> {
    const PAGE_SIZE: usize = 100;
    let mut all = Vec::new();
    let mut page = 1;
    loop {
        let out = provider_command("glab")
            .arg("issue")
            .arg("list")
            .arg("--repo")
            .arg(&profile.repo)
            .arg("--per-page")
            .arg(PAGE_SIZE.to_string())
            .arg("--page")
            .arg(page.to_string())
            .arg("-O")
            .arg("json")
            .output()
            .context("glab issue list")?;

        if !out.status.success() {
            anyhow::bail!(
                "glab issue list failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }

        let items: Vec<serde_json::Value> =
            serde_json::from_slice(&out.stdout).context("parsing GitLab issue list response")?;
        let count = items.len();

        for resp in items {
            let number = resp["iid"]
                .as_i64()
                .map(|n| n.to_string())
                .unwrap_or_default();
            let title = resp["title"].as_str().unwrap_or_default().to_string();
            let body = resp["description"].as_str().unwrap_or_default().to_string();
            let labels = resp["labels"]
                .as_array()
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|label| label.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            all.push(IssueDetails {
                number,
                title,
                body,
                labels,
            });
        }

        if count < PAGE_SIZE {
            break;
        }
        page += 1;
    }
    Ok(all)
}

/// List every open issue for the profile's provider. Returns an empty
/// list (not an error) for a provider that doesn't support issue tracking,
/// mirroring `scan_available_tickets`'s existing soft-fail-empty behavior
/// for a missing docs/tickets directory.
fn list_open_issues(profile: &Profile) -> Vec<IssueDetails> {
    let result = match profile.provider_cli() {
        Some("gh") => list_open_github_issues(profile),
        Some("glab") => list_open_gitlab_issues(profile),
        _ => return vec![],
    };
    result.unwrap_or_else(|e| {
        eprintln!("warning: failed to list open issues for ticket scan: {e:#}");
        vec![]
    })
}

/// Fetch issue details from the profile's provider
fn fetch_issue_details(profile: &Profile, issue_number: &str) -> Result<IssueDetails> {
    let cli = profile.provider_cli().ok_or_else(|| {
        anyhow::anyhow!(
            "provider '{}' does not support issue fetching",
            profile.provider
        )
    })?;

    let result = match cli {
        "gh" => fetch_github_issue(profile, issue_number),
        "glab" => fetch_gitlab_issue(profile, issue_number),
        other => anyhow::bail!("unsupported provider CLI: {}", other),
    };

    result
}

/// This extracts metadata from the issue title and body instead of from a markdown file.
fn parse_ticket_metadata_from_issue(issue: &IssueDetails) -> TicketMetadata {
    let mut meta = TicketMetadata {
        issue_number: Some(issue.number.clone()),
        ..TicketMetadata::default()
    };

    // Extract ticket ID from title if it follows the TICKET-N pattern
    let title = issue.title.trim();
    if title.starts_with("TICKET-") {
        if let Some((id, _)) = title.split_once(':').or_else(|| title.split_once(" — ")) {
            meta.ticket_id = Some(id.trim().to_string());
            meta.work_id = Some(id.trim().to_string());
        }
    }

    // Use the issue number as a fallback work_id
    if meta.work_id.is_none() {
        meta.work_id = Some(format!("TICKET-{}", issue.number));
    }

    // Parse the issue body for metadata fields
    // This mimics the existing markdown parsing but works on plain text
    for line in issue.body.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Difficulty:") {
            meta.difficulty = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Risk:") {
            meta.risk = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Recommended backend:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_backend = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Recommended model:") {
            let value = value.trim();
            if !value.is_empty() && value != "unspecified" {
                meta.recommended_model = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Goal:") {
            let value = value.trim();
            if !value.is_empty() {
                meta.goal = Some(value.to_string());
            }
            if meta.title.is_none() && !value.is_empty() {
                meta.title = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Suggested MR Title:") {
            meta.suggested_mr_title = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Work ID:") {
            meta.work_id = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Source:") {
            meta.source = Some(value.trim().to_string());
        }
    }

    // Set is_authoritative based on whether we have a proper ticket ID
    meta.is_authoritative = meta.ticket_id.is_some() || meta.work_id.is_some();

    // Extract problem from issue body
    meta.problem = extract_markdown_section(&issue.body, "Problem")
        .or_else(|| extract_markdown_section(&issue.body, "Background"))
        .or_else(|| extract_markdown_section(&issue.body, "Description"));

    // Extract acceptance criteria from issue body
    meta.acceptance_criteria = extract_markdown_list_section(&issue.body, "Acceptance Criteria");
    meta.constraints = extract_markdown_list_section(&issue.body, "Constraints");
    meta.verification_commands =
        extract_markdown_code_list_section(&issue.body, "Verification Commands");
    meta.affected_files = extract_markdown_list_section(&issue.body, "Affected Files");

    // Add labels as constraints or affected files if they look like file paths
    if !issue.labels.is_empty() {
        for label in &issue.labels {
            if label.contains('/') || label.contains('.') {
                if !meta.affected_files.contains(label) {
                    meta.affected_files.push(label.clone());
                }
            } else if !meta.constraints.contains(label) {
                meta.constraints.push(label.clone());
            }
        }
    }

    // Set title from the issue title if not already set
    if meta.title.is_none() {
        meta.title = Some(issue.title.trim().to_string());
    }
    meta.summary = meta.title.clone();

    meta
}

/// Format issue details for the Focus section in a task
fn format_issue_for_focus(issue: &IssueDetails) -> String {
    let mut content = format!("# Issue #{}: {}\n\n", issue.number, issue.title);

    if !issue.body.is_empty() {
        content.push_str(&issue.body);
        if !issue.body.ends_with('\n') {
            content.push('\n');
        }
    }

    if !issue.labels.is_empty() {
        content.push_str("\nLabels: ");
        content.push_str(&issue.labels.join(", "));
        content.push('\n');
    }

    content
}

/// Resolve a target string to either issue details or return the original target
fn resolve_target_to_issue_or_string(
    profile: &Profile,
    target: &str,
) -> Result<Option<IssueDetails>> {
    if is_issue_number_reference(target) {
        if let Some(issue_number) = extract_issue_number(target) {
            return Ok(Some(fetch_issue_details(profile, &issue_number)?));
        }
    }
    Ok(None)
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}
