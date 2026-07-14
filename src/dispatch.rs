use crate::config::{self, CandidateConfig, GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::models::CandidateArtifact;
use crate::models::{AvailableTicket, PmPlan, WorkMetadata};
use crate::notifications::{notify_event, NotifyEvent};
use crate::provider::provider_command;
use crate::routing::{
    self, CandidateIdentity, RouteDecision, RouteError, RouteRequest, RoutingRuntimeState,
    TaskRoutingContext,
};
use crate::validation_runner::{validate, validate_with_exit_code};
use crate::{provider, runner, usage, worktree};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::SyncSender;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const PROJECT_BRIEF_MAX_BYTES: usize = 10_000;
const LIVE_TASK_FALLBACK_MAX_BYTES: usize = 12_000;
const LIVE_TASK_TITLE_MAX_BYTES: usize = 1_024;
const LIVE_TASK_LABELS_MAX_BYTES: usize = 2_048;
const LIVE_TASK_PROBLEM_MAX_BYTES: usize = 4_096;
const LIVE_TASK_ACCEPTANCE_MAX_BYTES: usize = 8_192;
const LIVE_TASK_LIST_MAX_BYTES: usize = 4_096;
const LIVE_TASK_LIST_ITEM_MAX_BYTES: usize = 1_024;

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

/// Marks an error as a failed validation-gate self-check rather than a
/// ticket, backend, or transient controller failure. The controller uses this
/// typed boundary to pause a loop instead of repeatedly retrying a gate that
/// has already proved broken.
#[derive(Debug)]
pub struct ValidationGateError;

impl fmt::Display for ValidationGateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("validation gate self-check failed")
    }
}

impl std::error::Error for ValidationGateError {}

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

/// Parallel workers: how long a "claim" entry (`LedgerEntry::new_claim`)
/// blocks a ticket before it's treated as abandoned (worker crashed/killed
/// mid-flight, or was force-killed by the idle-timeout watchdog after
/// producing partial output that never reached a real completion entry).
/// 6 hours is a generous margin above the longest real dispatch duration
/// observed in practice (~3.9h, a slow openhands/hy3 run) -- long enough
/// that a live, still-working claim is never mistaken for abandoned, short
/// enough that a genuinely dead claim doesn't block a ticket for days.
const CLAIM_STALE_AFTER_HOURS: i64 = 6;

fn is_claim_stale(entry: &LedgerEntry) -> bool {
    let entry_time = if let Ok(parsed) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
        parsed
    } else if let Ok(secs) = entry.timestamp.parse::<i64>() {
        if let Ok(dt) = OffsetDateTime::from_unix_timestamp(secs) {
            dt
        } else {
            return true;
        }
    } else {
        return true;
    };
    let now = OffsetDateTime::now_utc();
    now - entry_time > time::Duration::hours(CLAIM_STALE_AFTER_HOURS)
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

/// Parallel workers: another concurrent `gah loop`/`gah dispatch` process
/// already claimed this work_id and hasn't finished (or abandoned) it yet.
/// Distinct from `DuplicateWorkError` (which means a real PR/MR already
/// exists) since no PR/branch may exist at all here -- the other worker
/// might still be mid-backend-run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveClaimError {
    pub work_id: String,
}

impl fmt::Display for ActiveClaimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Refusing dispatch: work ID '{}' was claimed by another in-flight dispatch within the last {CLAIM_STALE_AFTER_HOURS}h",
            self.work_id
        )
    }
}

impl std::error::Error for ActiveClaimError {}

/// Returns the resolved work_id on success (so `run()` can immediately
/// write a parallel-worker claim for it), or an error if this work_id is
/// already spoken for -- by a real open PR/MR (`DuplicateWorkError`) or by
/// another in-flight worker's claim (`ActiveClaimError`). `Ok(None)` means
/// no work_id could be resolved (nothing to claim, nothing to block).
fn check_duplicate_work(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
) -> Result<Option<String>> {
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
        return Ok(None);
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
        return Ok(None);
    };

    let matching_entries = match crate::ledger::entries_for_work_id(cfg, &work_id) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("warning: failed to read ledger entries: {:#}", e);
            return Ok(Some(work_id));
        }
    };

    if matching_entries.is_empty() {
        return Ok(Some(work_id));
    }

    // Try to fetch MRs/PRs from provider
    let mrs = crate::sync::fetch_mrs(profile).unwrap_or_default();

    for entry in matching_entries {
        if is_ledger_entry_stale(&entry) {
            continue;
        }

        // Parallel workers: another concurrent dispatch already claimed
        // this work_id and hasn't finished (or been abandoned long enough
        // to ignore) yet.
        if entry.mode == "claim" && !is_claim_stale(&entry) {
            return Err(anyhow::Error::new(ActiveClaimError {
                work_id: work_id.clone(),
            }));
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

    Ok(Some(work_id))
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
) -> Option<(usize, usize, Option<String>, bool, bool, bool)> {
    let Some(wid) = work_id else {
        return Some((0, 0, None, false, false, false));
    };
    let entries = ledger_entries_by_work_id.get(wid);
    let mut count = 0usize;
    let mut agent_failure_count = 0usize;
    let mut last_failure_class = None;
    let mut has_active_mr = false;
    let mut has_merged_mr = false;
    let mut has_active_claim = false;
    // TICKET-human-required-scoping: effective human_required is the most
    // recent state for this work item. A later review escalation explicitly
    // clears an earlier provisional human handoff; OR-ing historical flags
    // would leave the dashboard permanently blocked after automation recovers.
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
            has_active_claim = false;
            human_required = false;
            continue;
        }
        // Parallel workers: a claim marks the ticket as currently in-flight
        // for another concurrent worker. It's a lease marker, not a real
        // attempt outcome -- it doesn't count toward the retry cap, but it
        // does need to block re-selection until it either resolves (a real
        // completion entry follows) or goes stale (abandoned worker).
        if e.mode == "claim" {
            has_active_claim = !is_claim_stale(e);
            continue;
        }
        // Paid-route approvals are operator control records, not execution
        // attempts. Granting one releases the work-item human gate that asked
        // for approval; neither grant nor revoke consumes retry budget.
        if e.mode == "paid_route_approval_grant" {
            human_required = false;
            last_failure_class = Some(
                crate::ledger::FailureClass::AgentNoProgress
                    .as_str()
                    .to_string(),
            );
            continue;
        }
        if e.mode == "paid_route_approval_revoke" {
            continue;
        }
        count += 1;
        // A real completion entry (of any outcome) means whatever claim
        // preceded this attempt has resolved -- this ticket is no longer
        // in-flight from that worker's perspective.
        has_active_claim = false;
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
        // Only a review entry may CLEAR a prior human_required hold -- that is
        // the bounded escalation chain deliberately superseding its own earlier
        // provisional hold. Any other mode can still latch it true (e.g. a
        // fix/improve dispatch that hit "no eligible backend"), but must never
        // silently clear an existing hold set by something else: a racing
        // parallel worker's unrelated completion entry must not un-block a
        // ticket that a review already gave up on.
        if e.mode == "review" {
            human_required = e.human_required;
        } else if e.human_required {
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
        has_active_claim,
    ))
}

/// Numeric ID from either a legacy `TICKET-<digits>` work ID or the native
/// provider `#<digits>` identity.
fn ticket_number_prefix(work_id: &str) -> Option<&str> {
    let rest = work_id
        .strip_prefix("TICKET-")
        .or_else(|| work_id.strip_prefix('#'))?;
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
                has_active_claim,
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
                has_active_claim,
            });
        }
    }

    // Native issue tracker (GitHub/GitLab): the migration from docs/tickets
    // to real issues only wired up manual `--target
    // <issue-number>` dispatch -- `gah loop`'s own automatic ticket
    // discovery never learned to look here, so a fully-migrated profile's
    // backlog was invisible to `decide_next_action` (it saw 0-1 leftover
    // docs/tickets files instead of the real 100+ open issues). ticket_path
    // is the bare issue number string -- DispatchTicket/Retry/Escalate pass
    // it straight through as `--target`, and `resolve_target_to_issue_or_string`
    // already treats a numeric target as an issue reference.
    for issue in list_open_issues(profile) {
        // Every issue uses its provider-visible `#<number>` identity even
        // without a structured title, so is_authoritative is always
        // true here -- unlike docs/tickets files, there's no way for an
        // issue to opt out just by lacking metadata. A blocked/planning label
        // or `exec:owner-decision` is the generic signal for "don't
        // auto-dispatch this" (an owner-blocked infra issue with no code fix
        // available, a planning-only issue, or a task whose direction still
        // needs the owner's decision) --
        // without it, gah loop would burn real dispatch cycles on issues
        // no agent can meaningfully act on before HumanRequired kicks in.
        if issue_is_auto_dispatch_blocked(&issue.labels) {
            continue;
        }
        let meta = parse_ticket_metadata_from_issue(&issue);
        if !meta.is_authoritative {
            continue;
        }
        let work_id = meta.work_id.clone();
        // Some tracker issues were migrated from differently-numbered local
        // TICKET files (for example GitHub #118 from TICKET-101). Keep that
        // legacy title token only as a closed-work compatibility lookup; the
        // native issue identity remains `#118` everywhere else.
        let legacy_ticket_number = issue
            .title
            .find("TICKET-")
            .and_then(|idx| ticket_number_prefix(&issue.title[idx..]));
        if work_id
            .as_deref()
            .and_then(ticket_number_prefix)
            .is_some_and(|n| closed_ids.contains(n))
            || legacy_ticket_number.is_some_and(|n| closed_ids.contains(n))
        {
            continue;
        }
        let Some((
            prior_attempt_count,
            genuine_agent_failure_count,
            last_failure_class,
            has_active_mr,
            human_required,
            has_active_claim,
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
            has_active_claim,
        });
    }

    candidates
}

/// Labels that explicitly reserve a tracker issue for human action must take
/// precedence over the automatic issue-intake fallback. GitHub labels are
/// case-insensitive in practice, so normalize here rather than relying on a
/// single spelling in every profile.
fn issue_is_auto_dispatch_blocked(labels: &[String]) -> bool {
    labels.iter().any(|label| {
        matches!(
            label.trim().to_ascii_lowercase().as_str(),
            "blocked" | "planning" | "exec:owner-decision"
        )
    })
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
    run_id: Option<&str>,
) -> Result<()> {
    let mut entry = LedgerEntry::new(
        &profile.repo_id,
        profile,
        "none",
        "merge",
        branch,
        run_id.map(str::to_string),
        None,
    );
    entry.branch = Some(branch.to_string());
    entry.work_id = work_id.clone();
    entry.mr_url = mr_url.clone();
    entry.attempts_started = Some(1);

    let result = provider::merge_mr(profile, branch);
    match &result {
        Ok(()) => {
            entry.attempts_completed = Some(1);
            notify_event(
                cfg,
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

fn reserve_backend_slot(
    profile: &Profile,
    backend: &str,
    effective_model: Option<&str>,
) -> Result<routing::ConcurrencyGuard> {
    let concurrency_cap = profile
        .max_concurrent_per_model
        .get(&format!("{backend}/{}", effective_model.unwrap_or("")))
        .copied();
    routing::ConcurrencyGuard::acquire_shared(backend, effective_model, concurrency_cap)
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
    run_backend_with_reserved_route(
        backend,
        profile,
        wt,
        task,
        session_dir,
        llm,
        effective_model,
        env_path,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_backend_with_reserved_route(
    backend: &str,
    profile: &Profile,
    wt: &Path,
    task: &str,
    session_dir: &Path,
    llm: &runner::LlmConfig,
    effective_model: Option<&str>,
    env_path: Option<&str>,
    route_slot_already_reserved: bool,
) -> Result<runner::RunResult> {
    // Live incident (2026-07-11): concurrent dispatches landing on the same
    // shared free-tier backend+model (opencode/hy3-free) silently rate-limit.
    // Held for the duration of the actual backend call -- dropped on every
    // exit path (success, error, or panic) -- so routing's
    // `max_concurrent_per_model` check sees an accurate live count.
    let _concurrency_slot = (!route_slot_already_reserved)
        .then(|| reserve_backend_slot(profile, backend, effective_model))
        .transpose()?;
    let origin_before = worktree::git(&["remote", "get-url", "origin"], wt).ok();
    let mut env_vars = env_path.map(runner::load_env_file).unwrap_or_default();
    // Every agent and any test command it launches inherit this repository-
    // scoped target directory. Cargo safely serializes concurrent builds in a
    // shared target dir, while separate worktree-local `target/` directories
    // otherwise multiply multi-gigabyte artifacts until the host fills.
    env_vars.push((
        "CARGO_TARGET_DIR".to_string(),
        crate::build_cache::target_dir(&profile.artifact_root, session_dir)
            .to_string_lossy()
            .into_owned(),
    ));
    if backend == "agy-second" {
        if let Some(home) = profile.agy_second_home.as_deref().filter(|h| !h.is_empty()) {
            // Appended last so it overrides any HOME the env_file may have
            // set -- Command::env keeps the last value for a repeated key.
            env_vars.push(("HOME".to_string(), home.to_string()));
        }
    }
    let result = match backend {
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
            effective_model,
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
            effective_model,
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
            effective_model
                .and_then(|m| {
                    profile
                        .opencode_idle_timeout_seconds_by_model
                        .get(m)
                        .copied()
                })
                .unwrap_or_else(|| profile.opencode_idle_timeout_seconds()),
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
    };
    if let Some(origin_before) = origin_before {
        let origin_after = worktree::git(&["remote", "get-url", "origin"], wt)
            .context("checking git origin after backend run")?;
        if origin_after != origin_before {
            anyhow::bail!(
                "git origin changed during backend run: before='{origin_before}' after='{origin_after}'"
            );
        }
    }
    result
}

/// Run `auto_fix_commands` in the worktree, best-effort, right before
/// `validate()`. A formatter failing to run (missing binary, whatever) must
/// never block the dispatch -- it's a convenience, not a gate -- so every
/// failure is logged and swallowed rather than propagated.
fn run_auto_fix_commands(commands: &[String], wt: &Path, env_vars: &[(String, String)]) {
    for cmd_str in commands {
        if cmd_str.trim().is_empty() {
            continue;
        }
        let mut command = Command::new("sh");
        command.args(["-c", cmd_str]).current_dir(wt);
        for (key, value) in env_vars {
            command.env(key, value);
        }
        match command.output() {
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

const MIN_DISPATCH_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

fn validation_env(profile: &Profile, session_scope: &Path) -> Vec<(String, String)> {
    vec![(
        "CARGO_TARGET_DIR".to_string(),
        crate::build_cache::target_dir(&profile.artifact_root, session_scope)
            .to_string_lossy()
            .into_owned(),
    )]
}

fn ensure_dispatch_capacity(profile: &Profile, worktree_base: &Path) -> Result<()> {
    ensure_minimum_free_space(worktree_base, "worktree filesystem")?;
    ensure_minimum_free_space(&std::env::temp_dir(), "temporary filesystem")?;
    // Ensure the isolated-target parent exists before the first backend
    // inherits it; this also proves the configured artifact root is writable
    // early without sharing mutable Cargo outputs between worktrees.
    std::fs::create_dir_all(crate::build_cache::target_root(&profile.artifact_root))
        .context("creating isolated Cargo target root")?;
    Ok(())
}

fn ensure_minimum_free_space(path: &Path, label: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        // Dispatch creates the worktree directory after this preflight.  A
        // configured worktree base therefore commonly does not exist yet;
        // stat the nearest existing ancestor to measure the same filesystem
        // without making a harmless first dispatch fail with ENOENT.
        let filesystem_path = nearest_existing_ancestor(path)?;
        let path_c = CString::new(filesystem_path.as_os_str().as_bytes())?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path_c.as_ptr(), &mut stat) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "checking free space for {} (configured path {}; filesystem path {})",
                    label,
                    path.display(),
                    filesystem_path.display(),
                )
            });
        }
        let available = (stat.f_bavail as u128).saturating_mul(stat.f_frsize as u128);
        if available < MIN_DISPATCH_FREE_BYTES as u128 {
            anyhow::bail!(
                "insufficient free space on {} ({}): {} GiB available; require at least {} GiB before dispatch",
                label,
                path.display(),
                available / (1024 * 1024 * 1024),
                MIN_DISPATCH_FREE_BYTES / (1024 * 1024 * 1024),
            );
        }
    }
    #[cfg(not(unix))]
    let _ = (path, label);
    Ok(())
}

fn nearest_existing_ancestor(path: &Path) -> Result<&Path> {
    path.ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| anyhow::anyhow!("no existing ancestor for path {}", path.display()))
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

    let repo = Path::new(&profile.local_path);
    let target_sha = command_output("git", &["rev-parse", &profile.default_target_branch], repo)
        .map_err(|error| {
            // A dedicated ValidationGateError, not a generic error: this is
            // the gate itself failing to even run (e.g. default_target_branch
            // was renamed/deleted, or a shallow clone is missing the ref),
            // not a transient dispatch hiccup. Left as a plain error, it
            // would be misclassified by is_validation_gate_failure and the
            // daemon would retry it every 5 minutes forever instead of
            // pausing with a clear, actionable message like every other
            // broken-gate state in this function.
            anyhow::Error::new(ValidationGateError).context(format!(
                "VALIDATION GATE FAILED — could not resolve target branch '{}' for profile \
                 '{}': {error:#}",
                profile.default_target_branch, profile.repo_id
            ))
        })?;
    let gate_environment_signature =
        crate::build_cache::validation_environment_signature(&profile.artifact_root);
    let hash = vc::hash_validation_context(
        &profile.validation_commands,
        target_sha.trim(),
        &gate_environment_signature,
    );
    let state_path = vc::resolve_state_path();

    // Hold a per-profile lock across the whole decide-then-verify sequence,
    // not just the final write. Same-profile callers then share one proof,
    // while validation commands that invoke GAH for a different test profile
    // cannot deadlock behind their parent. The state file's global lock is
    // taken only for the short atomic record update below.
    let profile_lock = vc::acquire_profile_lock(&state_path, &profile.repo_id)?;
    if crate::runner::shutdown_requested() {
        fs2::FileExt::unlock(&profile_lock).ok();
        anyhow::bail!("shutdown requested before validation gate self-check");
    }

    let state = vc::load_state(&state_path)
        .with_context(|| format!("loading validation-check state {}", state_path.display()))?;

    if !vc::should_recheck(&state, &profile.repo_id, &hash) {
        println!(
            "[validation-gate] commands unchanged (hash {}) — skipping fresh-worktree self-check",
            &hash[..hash.len().min(8)]
        );
        fs2::FileExt::unlock(&profile_lock).ok();
        return Ok(());
    }

    println!(
        "[validation-gate] commands changed (hash {}) — verifying against a fresh worktree from '{}'...",
        &hash[..hash.len().min(8)],
        profile.default_target_branch
    );

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
    let cargo_target = crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root, &wt)?;
    let gate_environment = cargo_target.environment();
    let verified_at = vc::now_rfc3339(OffsetDateTime::now_utc());
    let result = validate(&profile.validation_commands, &wt, &gate_environment);
    let ok = result.is_ok();

    // Always clean up, regardless of pass/fail — a leftover validation-gate
    // worktree AND branch is state noise that the next dispatch would trip
    // over. The branch must be deleted too: worktree::cleanup only removes
    // the worktree dir and prunes, leaving the branch ref behind.
    worktree::cleanup(&wt, repo);
    let _ = worktree::git_raw(&["branch", "-D", &branch], repo);

    if crate::runner::shutdown_requested() {
        fs2::FileExt::unlock(&profile_lock).ok();
        anyhow::bail!("shutdown requested during validation gate self-check");
    }

    let record_result = vc::record_check(&state_path, &profile.repo_id, &hash, ok, &verified_at)
        .with_context(|| format!("recording validation-check result {}", state_path.display()));
    fs2::FileExt::unlock(&profile_lock).ok();
    record_result?;

    if let Err(text) = result {
        return Err(anyhow::Error::new(ValidationGateError).context(format!(
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
        )));
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
    effective_model: Option<&str>,
    transcript_path: Option<&str>,
    claude_path: Option<&str>,
) -> crate::ledger::LedgerUsage {
    let normalize = |mut usage: crate::ledger::LedgerUsage| {
        // These four named backends are subscription/account-backed in the
        // operator's routing policy. This is a classification of accounting
        // source, not a claim that their subscription has zero economic value.
        usage.backend_instance = backend.map(str::to_string);
        usage.usage_classification = match backend {
            Some("claude" | "codex" | "vibe" | "agy" | "agy-main" | "agy-second") => {
                Some("quota_backed".to_string())
            }
            Some(_) => Some("unknown".to_string()),
            None => None,
        };
        usage.provider = match backend {
            Some("claude") => Some("anthropic".to_string()),
            Some("codex") => Some("openai".to_string()),
            Some("vibe") => Some("mistral".to_string()),
            Some("agy" | "agy-main" | "agy-second") => Some("google".to_string()),
            _ => usage.provider,
        };
        // AGY receives this fully-qualified label as an explicit `--model`
        // argument. Its log exposes quota/reset state but no stable token or
        // model field on successful executions, so retain the bound model as
        // the exact invoked model rather than leaving coding attempts
        // unattributable. Other backends keep `actual_model` unknown unless a
        // backend-owned artifact reports it.
        if matches!(backend, Some("agy" | "agy-main" | "agy-second"))
            && usage.actual_model.is_none()
        {
            usage.actual_model = effective_model.map(str::to_string);
        }
        if matches!(backend, Some("agy" | "agy-main" | "agy-second")) {
            if usage.quota_window.is_none() {
                usage.quota_window = Some("AGY individual quota".to_string());
            }
            if usage.observed_at.is_none() {
                usage.observed_at = Some(
                    time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                );
            }
        }
        if usage.requests_count.is_none() {
            // A launched backend invocation is one consumed subscription
            // request even when its CLI exposes no token counters.
            usage.requests_count = Some(1);
        }
        if usage.usage_source.is_none() {
            usage.usage_source = Some("execution_observed".to_string());
        }
        if usage.actual_cost_usd.is_none() && usage.estimated_cost_usd.is_none() {
            usage.cost_unknown_reason = Some(
                "subscription backend does not expose a defensible per-execution dollar cost"
                    .to_string(),
            );
        }
        usage
    };
    let text = match fs::read_to_string(log_path) {
        Ok(t) => t,
        // No artifact means the execution evidence itself is unavailable;
        // preserve unknown rather than manufacturing a consumed request.
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
                    return normalize(merged);
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
        return normalize(usage);
    }

    // Vibe's structured session metadata is passed through the same artifact
    // slot as Claude's transcript.
    if backend == Some("vibe") {
        if let Some(metadata) = transcript_path {
            if let Ok(metadata_json) = fs::read_to_string(metadata) {
                let session_usage = usage::parse_vibe_session_metadata(&metadata_json);
                if session_usage.usage_source.is_some() {
                    return normalize(session_usage);
                }
            }
        }
    }

    // OpenCode persists exact per-session model and token counters in its
    // local SQLite store. The runner snapshots only this invocation's row
    // into a JSON artifact, avoiding a racy global "latest session" lookup.
    if backend == Some("opencode") {
        if let Some(metadata) = transcript_path {
            if let Ok(metadata_json) = fs::read_to_string(metadata) {
                let session_usage = usage::parse_opencode_session_metadata(&metadata_json);
                if session_usage.usage_source.is_some() {
                    return normalize(session_usage);
                }
            }
        }
    }

    // Try codex exec --json parser first — handles JSONL output from
    // codex exec --json where the generic regex parser would find nothing.
    let mut usage = if backend == Some("codex") {
        usage::parse_codex_exec_json(&text)
    } else {
        crate::ledger::LedgerUsage::default()
    };
    if backend == Some("openhands") {
        let openhands_usage = usage::parse_openhands_usage(&text);
        if openhands_usage.usage_source.is_some() {
            usage = usage::merge_usage(openhands_usage, usage);
        }
    }
    let has_json_lines = text.lines().any(|line| line.trim_start().starts_with('{'));
    if usage.usage_source.is_none() && (backend != Some("codex") || !has_json_lines) {
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
    normalize(usage)
}

/// Attribute a review invocation even when the reviewer does not expose token
/// counters. Reviews consume the same subscription/API capacity as coding
/// attempts, so an empty review-output parser result must not turn a completed
/// reviewer run into an invisible zero-usage ledger record.
///
/// AGY receives its fully-qualified model label as an explicit `--model`
/// argument. In that case the invoked model is directly observable from the
/// command contract, so retain it as the actual model when AGY itself did not
/// emit a more specific observation. Other backends may resolve aliases or
/// proxy routes after launch; leave their actual model unknown unless their
/// backend artifact reports it.
fn review_usage(
    log_path: &str,
    backend: &str,
    effective_model: Option<&str>,
    claude_path: Option<&str>,
) -> crate::ledger::LedgerUsage {
    attempt_usage(
        log_path,
        None,
        Some(backend),
        effective_model,
        None,
        claude_path,
    )
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

/// Whether `improve()` can skip its own per-dispatch pristine-worktree
/// baseline validation, relying instead on the profile-level validation gate
/// (`self_check_validation_gate`). That shared gate only ever proves
/// `profile.default_target_branch` -- so its proof only covers a dispatch
/// that skipping requires: a FRESH worktree cut from that branch (no
/// `existing_branch`, i.e. not a `FixMr`/repair dispatch, which validates the
/// existing MR branch instead) AND the gate not having been explicitly
/// bypassed (no shared proof exists in that case either).
fn should_skip_per_dispatch_baseline(
    validation_commands_empty: bool,
    has_existing_branch: bool,
    skip_validation_gate: bool,
) -> bool {
    validation_commands_empty || (!has_existing_branch && !skip_validation_gate)
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
                    usage: crate::ledger::LedgerUsage::default(),
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
            ledger.set_failure(
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::AgentRun,
            );
            ledger.validation_result = Some("cancelled_shutdown".into());
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
                    Some(route.effective_backend.as_str()),
                    Some(&llm.model),
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
            let semantic_no_progress = log_text.contains("GAH: killed after ")
                && log_text.contains("with no new worktree progress (stalled, not just slow).");
            let failure_class = if semantic_no_progress {
                crate::ledger::FailureClass::AgentNoProgress
            } else {
                crate::ledger::FailureClass::BackendError
            };
            ledger.set_failure(failure_class, crate::ledger::FailureStage::AgentRun);
            ledger.attempts.push(crate::ledger::AttemptRecord {
                attempt_number: attempt + 1,
                backend: route.effective_backend.clone(),
                effective_model: Some(llm.model.clone()),
                exit_code: Some(result.exit_code),
                validation_result: None,
                failure_class: Some(failure_class.as_str().into()),
                failure_stage: Some(crate::ledger::FailureStage::AgentRun.as_str().into()),
                duration_seconds: Some(attempt_start.elapsed().as_secs_f64()),
                diff_path: None,
                usage: attempt_usage(
                    &result.log_path,
                    result.agy_cli_log_delta.as_deref(),
                    Some(route.effective_backend.as_str()),
                    Some(&llm.model),
                    result.transcript_path.as_deref(),
                    Some(&claude_path),
                ),
                cli_version: result.agy_version.clone(),
            });
            let stalled = log_text.contains("GAH: killed after ")
                && log_text.contains("(stalled, not just slow).");
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
                if worktree::checkpoint_wip(
                    &wt,
                    &profile.default_target_branch,
                    &checkpoint,
                    &format!("gah: WIP failed {} attempt {}", args.mode, attempt + 1),
                )? {
                    println!("Preserved failed attempt on local branch {checkpoint}");
                    wip_checkpoints.push(checkpoint);
                }
                worktree::reset_to_target(&wt, &profile.default_target_branch)?;
                prior_phase_context = Some(task.clone());
                task = format!(
                    "{}\n\n## Previous attempt did not complete (attempt {}/{})\n\nThe backend exited with code {} before finishing (not a validation failure -- it errored, crashed, or was killed for producing no output). The worktree has been reset clean. Please try again.",
                    base_task,
                    attempt + 1,
                    max_attempts,
                    result.exit_code,
                );
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
        if !worktree::has_changes(&wt, &profile.default_target_branch)? {
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
                        Some(route.effective_backend.as_str()),
                        Some(&llm.model),
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
                    Some(route.effective_backend.as_str()),
                    Some(&llm.model),
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
                    Some(route.effective_backend.as_str()),
                    Some(&llm.model),
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
                        Some(route.effective_backend.as_str()),
                        Some(&llm.model),
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
                            Some(route.effective_backend.as_str()),
                            Some(&llm.model),
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
                            Some(route.effective_backend.as_str()),
                            Some(&llm.model),
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
                            Some(route.effective_backend.as_str()),
                            Some(&llm.model),
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
                            Some(route.effective_backend.as_str()),
                            Some(&llm.model),
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

    let has_changes = worktree::has_changes(&wt, &profile.default_target_branch)?;
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
        None,
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
    let wt = classify_worktree_result(
        ledger,
        worktree::create(
            repo,
            &profile.default_target_branch,
            &branch,
            &worktree_base,
        ),
    )?;
    ledger.branch = Some(branch.clone());
    println!("Worktree: {}", wt.display());
    println!("Branch:   {}", branch);
    let _cargo_target =
        crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root, session_dir)?;

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
                final_summary: None,
                agy_cli_log_delta: None,
                internal_log_delta: None,
                internal_log_path: None,
                transcript_path: None,
                agy_version: None,
            }
        }
    };
    println!(
        "Backend finished: exit={} duration={:.0}s log={}",
        result.exit_code, result.duration_secs, result.log_path
    );
    ledger.backend_exit_code = Some(result.exit_code);

    let backend_summary = runner::output::publishable_summary(
        result.final_summary.as_deref(),
        ledger.target_summary.as_deref(),
        &wt,
    );

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
    classify_git_operation_result(
        ledger,
        crate::ledger::FailureStage::Push,
        worktree::push_branch(&wt, &branch, &push_url, &push_pat),
    )?;
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
        cfg,
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
    let mut plan_route = decide_route(cfg, profile, route_req.clone(), None, ledger)?;
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
    let _cargo_target =
        crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root, session_dir)?;

    let mut attempted_routes = HashSet::new();
    let log_text = loop {
        let attempt_index = attempted_routes.len() + 1;
        let attempt_dir = session_dir.join(format!("pm-run-{attempt_index}"));
        fs::create_dir_all(&attempt_dir)?;
        fs::write(attempt_dir.join("task.md"), crate::redact::redact(&task))?;

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

        let rerouted = decide_route(cfg, profile, route_req.clone(), None, ledger)?;
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

        let attempt = runner::run_review_backend(
            profile,
            &route.effective_backend,
            repo,
            &prompt,
            session_dir,
            route.effective_model.as_deref(),
            &env_vars,
        );
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
                    session_dir.join("review-stderr.log")
                } else {
                    session_dir.join("review-stdout.log")
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
    if !result.stderr.trim().is_empty() {
        fs::write(session_dir.join("review-stderr.log"), &result.stderr)?;
    }

    match result.outcome {
        runner::ReviewProcessOutcome::Success => {
            let review_output_log = session_dir.join("review-stdout.log");
            let review_usage = review_usage(
                &review_output_log.display().to_string(),
                &route.effective_backend,
                route.effective_model.as_deref(),
                profile.claude_path.as_deref(),
            );
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
            ledger.usage = review_usage.clone();
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
            ledger.set_failure(
                crate::ledger::FailureClass::BackendError,
                crate::ledger::FailureStage::Review,
            );
            ledger.backend_exit_code = Some(-signal);
            ledger.validation_result = Some("not_run".into());
            println!("Review backend terminated by signal {}.", signal);
            println!("Review bundle written to: {}", bundle.display());
            anyhow::bail!("review backend terminated by signal {signal}")
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
             Ignore any other backlog items, priorities, or tickets mentioned in background \
             context -- those are not additional work to pick up.\n\
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

    append_project_brief(&mut task, profile);

    if !target.is_empty() {
        task.push_str(&format!("\n## Focus\n\n{}\n", target));
    }
    task
}

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
             Ignore any other backlog items, priorities, or tickets mentioned in background \
\
             context -- those are not additional work to pick up.\n\
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

    append_project_brief(&mut task, profile);
    append_live_task_pack(&mut task, issue);

    task.push_str(&format!(
        "\n## Focus\n\n{}\n",
        format_issue_focus_reference(issue)
    ));
    task
}

/// Worker prompts deliberately use a concise, committed project brief rather
/// than the live manager ledger. MANAGER_MEMORY remains PM-only operational
/// state: injecting it into every worker made stale status and retry history
/// compete with the ticket being executed.
fn append_project_brief(task: &mut String, profile: &Profile) {
    let brief_path = Path::new(&profile.local_path).join("docs/PROJECT_BRIEF.md");
    let Ok(brief) = fs::read_to_string(brief_path) else {
        return;
    };
    task.push_str("\n## Project Brief\n\n");
    append_bounded_text(task, &brief, PROJECT_BRIEF_MAX_BYTES, "Project brief");
}

/// Build a bounded, task-specific packet from structured issue metadata.
/// The free-form body is used only as a capped fallback when the issue did
/// not provide a structured problem, acceptance criteria, or constraints.
fn append_live_task_pack(task: &mut String, issue: &IssueDetails) {
    let meta = parse_ticket_metadata_from_issue(issue);
    task.push_str("\n## Live Task Pack\n\n");
    task.push_str(&format!("Work item: #{} — ", issue.number));
    append_bounded_text(
        task,
        &indent_untrusted_text(&issue.title),
        LIVE_TASK_TITLE_MAX_BYTES,
        "Work item title",
    );
    if !issue.labels.is_empty() {
        task.push_str("Labels: ");
        append_bounded_text(
            task,
            &indent_untrusted_text(&issue.labels.join(", ")),
            LIVE_TASK_LABELS_MAX_BYTES,
            "Labels",
        );
    }
    if let Some(problem) = meta.problem.as_deref().or(meta.goal.as_deref()) {
        task.push_str("\n### Problem\n\n");
        append_bounded_text(
            task,
            &indent_untrusted_text(problem),
            LIVE_TASK_PROBLEM_MAX_BYTES,
            "Problem",
        );
    }
    append_task_pack_list(
        task,
        "Acceptance Criteria",
        &meta.acceptance_criteria,
        LIVE_TASK_ACCEPTANCE_MAX_BYTES,
    );
    append_task_pack_list(
        task,
        "Constraints",
        &meta.constraints,
        LIVE_TASK_LIST_MAX_BYTES,
    );
    append_task_pack_list(
        task,
        "Affected Files",
        &meta.affected_files,
        LIVE_TASK_LIST_MAX_BYTES,
    );
    if !meta.verification_commands.is_empty() {
        task.push_str("\n### Verification Commands\n\n");
        append_task_pack_list_items(
            task,
            &meta.verification_commands,
            LIVE_TASK_LIST_MAX_BYTES,
            true,
        );
    }
    if issue_has_no_structured_body(issue) {
        task.push_str("\n### Issue Description\n\n");
        append_bounded_text(
            task,
            &indent_untrusted_text(&issue.body),
            LIVE_TASK_FALLBACK_MAX_BYTES,
            "Issue description",
        );
    }
}

fn append_task_pack_list(task: &mut String, heading: &str, entries: &[String], max_bytes: usize) {
    if entries.is_empty() {
        return;
    }
    task.push_str(&format!("\n### {heading}\n\n"));
    append_task_pack_list_items(task, entries, max_bytes, false);
}

fn append_task_pack_list_items(
    task: &mut String,
    entries: &[String],
    max_bytes: usize,
    code: bool,
) {
    let start = task.len();
    let mut truncated = false;
    for entry in entries {
        if task.len().saturating_sub(start) >= max_bytes {
            truncated = true;
            break;
        }
        let value = indent_untrusted_text(utf8_safe_prefix(entry, LIVE_TASK_LIST_ITEM_MAX_BYTES));
        let line = if code {
            format!("- `{value}`\n")
        } else {
            format!("- {value}\n")
        };
        if task.len().saturating_sub(start) + line.len() > max_bytes {
            let remaining = max_bytes.saturating_sub(task.len().saturating_sub(start));
            if remaining > 3 {
                task.push_str(utf8_safe_prefix(&line, remaining));
            }
            truncated = true;
            break;
        }
        if entry.len() > LIVE_TASK_LIST_ITEM_MAX_BYTES {
            truncated = true;
        }
        task.push_str(&line);
    }
    if truncated {
        task.push_str(&format!(
            "[List truncated at {max_bytes} bytes; retrieve the issue for remaining detail.]\n"
        ));
    }
}

fn issue_has_no_structured_body(issue: &IssueDetails) -> bool {
    extract_markdown_section(&issue.body, "Problem").is_none()
        && extract_markdown_section(&issue.body, "Background").is_none()
        && extract_markdown_section(&issue.body, "Description").is_none()
        && extract_markdown_list_section(&issue.body, "Acceptance Criteria").is_empty()
        && extract_markdown_list_section(&issue.body, "Constraints").is_empty()
}

fn append_bounded_text(task: &mut String, text: &str, max_bytes: usize, label: &str) {
    let truncated = text.len() > max_bytes;
    task.push_str(utf8_safe_prefix(text, max_bytes));
    if truncated {
        task.push_str(&format!(
            "\n\n[{label} truncated at {max_bytes} bytes; retrieve only relevant source material for more detail.]\n"
        ));
    } else if !text.ends_with('\n') {
        task.push('\n');
    }
}

fn indent_untrusted_text(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Helper function to safely render multiline candidate-controlled content.
/// Indents each line to prevent Markdown heading injection that could create
/// synthetic protected sections during context compaction.
fn safe_render_multiline(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {}", line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
            out.push_str(&format!("- {}\n", safe_render_multiline(e)));
        }
        out.push('\n');
    }

    if !c.affected_files.is_empty() {
        out.push_str("## Files likely involved\n");
        for f in &c.affected_files {
            out.push_str(&format!("- {}\n", safe_render_multiline(f)));
        }
        out.push('\n');
    }

    if !c.acceptance_criteria.is_empty() {
        out.push_str("## Acceptance criteria\n");
        for (i, ac) in c.acceptance_criteria.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, safe_render_multiline(ac)));
        }
        out.push('\n');
    }

    if !c.verification.is_empty() {
        out.push_str("## Verification steps\n");
        for (i, v) in c.verification.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, safe_render_multiline(v)));
        }
        out.push('\n');
    }

    append_project_brief(&mut out, profile);

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
    out.push_str("\n## Safety\n\n");
    out.push_str(closing);
    out
}

#[cfg(test)]
mod tests {
    use super::preflight;
    use super::run_auto_fix_commands;
    use super::self_check_validation_gate;
    use super::ValidationGateError;
    use super::{
        apply_authoritative_work_identity, apply_diff_stats, apply_pm_plan, apply_route_to_ledger,
        attempt_usage, build_experiment_mr_body, build_fix_or_improve_mr_body,
        build_metadata_rich_mr_body, build_mr_title, build_pm_plan_task, build_standard_mr_body,
        build_task, check_review_budget, classify_git_operation_result,
        classify_validation_failure_progress, classify_worktree_result, collect_pm_preflight,
        collect_ticket_summaries, decide_route, derive_reviewer_tier, extract_issue_number,
        first_markdown_heading, format_candidate_task, github_issue_author_is_allowed,
        is_issue_number_reference, mark_backend_unavailable_from_output_at,
        nearest_existing_ancestor, next_escalatory_reviewer, next_ticket_id, parse_pm_plan,
        parse_review_verdict, parse_review_verdict_with_context, parse_ticket_metadata,
        parse_ticket_metadata_from_issue, render_review_comment, review_escalation_reason,
        review_labels, review_preflight, review_usage, reviewer_dedup_class, routing_runtime_state,
        run_backend, scan_available_tickets, should_skip_per_dispatch_baseline,
        validation_failure_fingerprint, validation_failure_no_progress_reason,
        ExperimentMrRenderContext, IssueDetails, MrRenderContext, ReviewDiffBundle,
        ReviewGateContext, ReviewerTier, RouteDecision, TicketMetadata, ValidationFailureProgress,
        LIVE_TASK_ACCEPTANCE_MAX_BYTES, PROJECT_BRIEF_MAX_BYTES,
    };
    use crate::availability::{availability_for, load_state, Reason};
    use crate::config::{CandidateConfig, Defaults, GahConfig, Profile, RoutingPolicy};
    use crate::ledger::LedgerEntry;
    use crate::models::{Candidate, PmPlan};
    use crate::routing::{CandidateIdentity, RouteError, RouteRequest};
    use crate::test_support::PathGuard;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use time::OffsetDateTime;

    const CODEX_FULL_RESET: &str =
        include_str!("../tests/fixtures/quota-logs/codex_usage_exhausted_full_reset.txt");
    const OPENCODE_HY3_RATE_LIMIT: &str =
        include_str!("../tests/fixtures/quota-logs/opencode_hy3_rate_limit.log");

    fn profile(local_path: &Path) -> Profile {
        Profile {
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
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
            opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
            max_concurrent_per_model: std::collections::HashMap::new(),
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
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
            context: Default::default(),
            defaults: Defaults {
                current_manager: None,
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

    // Like `gah_config`, but with `artifact_root` pointed at a real tempdir
    // so `ledger::append`/`read_entries` have somewhere to write.
    fn gah_config_with_ledger(tmp: &Path, routing: RoutingPolicy) -> GahConfig {
        GahConfig {
            context: Default::default(),
            defaults: Defaults {
                current_manager: None,
                artifact_root: tmp.display().to_string(),
                worktree_base: String::new(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing,
            },
            profiles: std::collections::HashMap::new(),
        }
    }

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

        let usage = attempt_usage(path.to_str().unwrap(), None, Some("vibe"), None, None, None);
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

        let usage = attempt_usage(path.to_str().unwrap(), None, Some("vibe"), None, None, None);
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
        assert_eq!(usage.requests_count, Some(1));
        assert_eq!(usage.usage_classification, Some("quota_backed".to_string()));
    }

    #[test]
    fn attempt_usage_records_the_bound_agy_model_when_cli_logs_only_quota_state() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("backend-output.log");
        fs::write(&path, "completed successfully\n").unwrap();

        let usage = attempt_usage(
            path.to_str().unwrap(),
            Some("quotaRefreshLoop: completed"),
            Some("agy"),
            Some("Gemini 3.5 Flash (Medium)"),
            None,
            None,
        );

        assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
        assert_eq!(usage.usage_classification.as_deref(), Some("quota_backed"));
        assert_eq!(usage.provider.as_deref(), Some("google"));
        assert_eq!(
            usage.actual_model.as_deref(),
            Some("Gemini 3.5 Flash (Medium)")
        );
        assert_eq!(usage.requests_count, Some(1));
        assert_eq!(usage.quota_window.as_deref(), Some("AGY individual quota"));
    }

    #[test]
    fn review_usage_records_an_agy_review_without_token_counters() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("review-stdout.log");
        fs::write(&path, "review completed; no token counters exposed\n").unwrap();

        let usage = review_usage(
            path.to_str().unwrap(),
            "agy",
            Some("Claude Sonnet 4.6 (Thinking)"),
            None,
        );

        assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
        assert_eq!(usage.usage_classification.as_deref(), Some("quota_backed"));
        assert_eq!(usage.backend_instance.as_deref(), Some("agy"));
        assert_eq!(usage.provider.as_deref(), Some("google"));
        assert_eq!(
            usage.actual_model.as_deref(),
            Some("Claude Sonnet 4.6 (Thinking)")
        );
        assert_eq!(usage.requests_count, Some(1));
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.quota_window.as_deref(), Some("AGY individual quota"));
    }

    #[test]
    fn attempt_usage_does_not_scrape_codex_tool_output_as_usage() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("backend-output.log");
        fs::write(
            &path,
            r#"{"type":"item.completed","item":{"aggregated_output":"input_tokens: 500"}}
{"type":"item.started","item":{"type":"command_execution"}}
"#,
        )
        .unwrap();

        let usage = attempt_usage(
            path.to_str().unwrap(),
            None,
            Some("codex"),
            None,
            None,
            None,
        );
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.requests_count, Some(1));
        assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
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
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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

        let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\nRecommended model: Gemini 3.5 Flash (Medium)\n","labels":[],"author":{"login":"owner"}}]"#;
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
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
        assert_eq!(candidates[0].work_id.as_deref(), Some("#118"));
        assert_eq!(candidates[0].recommended_backend.as_deref(), Some("agy"));
        assert_eq!(candidates[0].prior_attempt_count, 0);
        assert!(!candidates[0].has_active_mr);
    }

    #[test]
    fn scan_available_tickets_uses_native_identity_for_gitlab_issues() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let issue_json = r#"[{"iid":77,"title":"TICKET-9: Legacy title must not become identity","description":"Work ID: TICKET-9\nRecommended backend: codex","labels":[],"state":"opened"}]"#;
        let glab_path = bin_dir.join("glab");
        fs::write(
            &glab_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
                issue_json.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&glab_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&glab_path, perms).unwrap();
        }
        let _guard = PathGuard::set(&bin_dir);

        let cfg = crate::config::GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
        prof.provider = "gitlab".to_string();
        prof.repo = "group/project".to_string();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].ticket_path, "77");
        assert_eq!(candidates[0].work_id.as_deref(), Some("#77"));
        assert_eq!(
            candidates[0].title.as_deref(),
            Some("Legacy title must not become identity")
        );
    }

    #[test]
    fn scan_available_tickets_excludes_owner_decision_github_issues() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let issue_json = r#"[{"number":92,"title":"MS-5: Fleet ledger","body":"","labels":[{"name":"EXEC:OWNER-DECISION"}],"author":{"login":"owner"}}]"#;
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
        let cfg = crate::config::GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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

        assert!(candidates.is_empty());
    }

    #[test]
    fn github_issue_intake_author_allowlist_is_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        prof.repo = "Kh1ng/git-agent-harness".into();
        let owner = serde_json::json!({"author": {"login": "kh1ng"}});
        let outsider = serde_json::json!({"author": {"login": "untrusted"}});
        let missing = serde_json::json!({});

        // No explicit configuration is still safe for a personal repo: only
        // the repository owner is trusted.
        assert!(github_issue_author_is_allowed(&prof, &owner));
        assert!(!github_issue_author_is_allowed(&prof, &outsider));
        assert!(!github_issue_author_is_allowed(&prof, &missing));

        // An explicit allowlist is the complete trusted set, rather than an
        // additive exception to the owner-only default.
        prof.publishing.github_issue_author_allowlist = Some(vec!["teammate".into()]);
        let teammate = serde_json::json!({"author": {"login": "TEAMMATE"}});
        assert!(github_issue_author_is_allowed(&prof, &teammate));
        assert!(!github_issue_author_is_allowed(&prof, &owner));

        prof.publishing.github_issue_author_allowlist = Some(vec![]);
        assert!(!github_issue_author_is_allowed(&prof, &teammate));
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

        let issue_json = r#"[{"number":118,"title":"TICKET-101-fail-closed-version-drift: TICKET-101 — Fail closed","body":"Recommended backend: agy\n","labels":[],"author":{"login":"owner"}}]"#;
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
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
    fn human_required_is_not_cleared_by_a_later_non_review_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-300-test.md"),
            "# TICKET-300: Test ticket\n\nGoal: test\n",
        )
        .unwrap();
        let cfg = crate::config::GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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

        // A review escalation exhausted its chain and gave up on a human.
        let mut exhausted = LedgerEntry::new("test", &prof, "claude", "review", "x", None, None);
        exhausted.work_id = Some("TICKET-300".into());
        exhausted.human_required = true;
        crate::ledger::append(&cfg, &exhausted).unwrap();

        // A racing worker's unrelated fix dispatch completes afterward with a
        // normal (non-human-required) outcome. It must not silently un-block
        // a ticket a review already gave up on.
        let mut racing_fix = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        racing_fix.work_id = Some("TICKET-300".into());
        racing_fix.human_required = false;
        crate::ledger::append(&cfg, &racing_fix).unwrap();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert!(
            candidates[0].human_required,
            "a later non-review entry must not clear a human_required hold"
        );
    }

    #[test]
    fn paid_route_grant_clears_handoff_and_resumes_escalation_without_consuming_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-301-test.md"),
            "# TICKET-301: Test ticket\n\nGoal: test\n",
        )
        .unwrap();
        let cfg = crate::config::GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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

        let mut handoff = LedgerEntry::new("test", &prof, "auto", "fix", "x", None, None);
        handoff.work_id = Some("TICKET-301".into());
        handoff.human_required = true;
        handoff.set_failure(
            crate::ledger::FailureClass::HumanBlocked,
            crate::ledger::FailureStage::Route,
        );
        crate::ledger::append(&cfg, &handoff).unwrap();
        crate::ledger::append(
            &cfg,
            &LedgerEntry::new_paid_route_approval(
                "test",
                &prof,
                "TICKET-301",
                "opencode",
                Some("openai/gpt-paid"),
                true,
            ),
        )
        .unwrap();

        let candidates = scan_available_tickets(
            &prof,
            &[],
            &crate::ledger::index_entries_by_work_id(&crate::ledger::read_entries(&cfg).unwrap()),
        );
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].human_required);
        assert_eq!(candidates[0].prior_attempt_count, 1);
        assert_eq!(
            candidates[0].last_failure_class.as_deref(),
            Some("agent_no_progress")
        );
    }

    #[test]
    fn implementation_escalation_ignores_review_failure_routes() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
        let prof = profile(tmp.path());
        let mut review = LedgerEntry::new("test", &prof, "codex", "review", "x", None, None);
        review.work_id = Some("ISSUE-42".into());
        review.effective_backend = "codex".into();
        review.effective_model = Some("review-model".into());
        review.set_failure(
            crate::ledger::FailureClass::AgentFailure,
            crate::ledger::FailureStage::AgentRun,
        );
        crate::ledger::append(&cfg, &review).unwrap();

        let mut current = LedgerEntry::new("test", &prof, "auto", "fix", "x", None, None);
        current.work_id = Some("ISSUE-42".into());
        let state = routing_runtime_state(&cfg, &current).unwrap();
        assert!(state.attempted.is_empty());

        let mut implementation =
            LedgerEntry::new("test", &prof, "codex", "improve", "x", None, None);
        implementation.work_id = Some("ISSUE-42".into());
        implementation.effective_backend = "codex".into();
        implementation.effective_model = Some("worker-model".into());
        implementation.set_failure(
            crate::ledger::FailureClass::AgentFailure,
            crate::ledger::FailureStage::AgentRun,
        );
        crate::ledger::append(&cfg, &implementation).unwrap();

        let state = routing_runtime_state(&cfg, &current).unwrap();
        assert_eq!(state.attempted.len(), 1);
        assert!(state
            .attempted
            .contains(&CandidateIdentity::new("codex", Some("worker-model"))));
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
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
            merged_at: None,
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
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
            merged_at: None,
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
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
            merged_at: None,
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

    // Parallel workers: a fresh claim marks a ticket has_active_claim,
    // excluding it from re-selection; a real completion entry after the
    // claim resolves it, and a stale claim stops blocking on its own.
    #[test]
    fn scan_available_tickets_reflects_claim_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("TICKET-501-test.md"),
            "# TICKET-501: Test\n\nGoal: test claim lifecycle\n",
        )
        .unwrap();

        let mut prof = profile(tmp.path());
        prof.local_path = tmp.path().display().to_string();
        prof.provider = String::new();

        // A fresh claim, nothing else -> has_active_claim = true.
        let claim = LedgerEntry::new_claim("test", &prof, "TICKET-501");
        let index = crate::ledger::index_entries_by_work_id(std::slice::from_ref(&claim));
        let candidates = scan_available_tickets(&prof, &[], &index);
        assert_eq!(candidates.len(), 1);
        assert!(
            candidates[0].has_active_claim,
            "fresh claim should mark the ticket as actively claimed"
        );
        assert_eq!(
            candidates[0].prior_attempt_count, 0,
            "a claim is a lease marker, not a counted attempt"
        );

        // A real completion entry after the claim resolves it.
        let mut completed = LedgerEntry::new("test", &prof, "codex", "fix", "x", None, None);
        completed.work_id = Some("TICKET-501".into());
        completed.failure_class = Some("backend_error".into());
        let index = crate::ledger::index_entries_by_work_id(&[claim, completed]);
        let candidates = scan_available_tickets(&prof, &[], &index);
        assert!(
            !candidates[0].has_active_claim,
            "a completion entry after the claim must clear has_active_claim"
        );

        // A stale (>6h old) claim with no completion after it -> not active.
        let mut stale_claim = LedgerEntry::new_claim("test", &prof, "TICKET-501");
        stale_claim.timestamp = (OffsetDateTime::now_utc() - time::Duration::hours(7))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let index = crate::ledger::index_entries_by_work_id(&[stale_claim]);
        let candidates = scan_available_tickets(&prof, &[], &index);
        assert!(
            !candidates[0].has_active_claim,
            "a stale claim must no longer block re-selection"
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

    /// Issue: opencode routes both a free-tier model that hangs at zero
    /// output when rate-limited (kill fast) and a real-but-slow self-hosted
    /// litellm model (give it more time) through the same flat
    /// `opencode_idle_timeout_seconds`. Mirrors
    /// `run_backend_looks_up_agy_print_timeout_by_exact_model_name`: prove
    /// the per-model override in `opencode_idle_timeout_seconds_by_model`
    /// is what actually governs the kill, not the flat default, by setting
    /// the flat default so high the test would hang if it were used.
    #[test]
    fn run_backend_looks_up_opencode_idle_timeout_by_exact_model_name() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let fake_opencode = bin_dir.join("opencode");
        fs::write(
            &fake_opencode,
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&fake_opencode).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&fake_opencode, perms).unwrap();
        }

        let mut prof = profile(tmp.path());
        prof.opencode_path = Some(fake_opencode.display().to_string());
        prof.opencode_idle_timeout_seconds = Some(100); // flat default: would hang the test if used
        prof.opencode_idle_timeout_seconds_by_model
            .insert("litellm-lan/qwen3.6:35b-a3b".to_string(), 1);

        let session_dir = tmp.path().join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let llm = crate::runner::LlmConfig {
            base_url: String::new(),
            api_key: String::new(),
            model: "unused-for-opencode".to_string(),
        };

        let result = run_backend(
            "opencode",
            &prof,
            tmp.path(),
            "do the thing",
            &session_dir,
            &llm,
            Some("litellm-lan/qwen3.6:35b-a3b"),
            None,
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("killed after 1s with no new worktree progress"),
            "got: {log}"
        );
    }

    /// Complement to the above: a model with no per-model entry must fall
    /// back to the flat `opencode_idle_timeout_seconds`, not silently pick
    /// up some other model's override.
    #[test]
    fn run_backend_falls_back_to_flat_opencode_idle_timeout_for_unmapped_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let fake_opencode = bin_dir.join("opencode");
        fs::write(
            &fake_opencode,
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&fake_opencode).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&fake_opencode, perms).unwrap();
        }

        let mut prof = profile(tmp.path());
        prof.opencode_path = Some(fake_opencode.display().to_string());
        prof.opencode_idle_timeout_seconds = Some(1); // flat fallback: should apply
        prof.opencode_idle_timeout_seconds_by_model
            .insert("hy3-free".to_string(), 100); // a different model's override

        let session_dir = tmp.path().join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let llm = crate::runner::LlmConfig {
            base_url: String::new(),
            api_key: String::new(),
            model: "unused-for-opencode".to_string(),
        };

        let result = run_backend(
            "opencode",
            &prof,
            tmp.path(),
            "do the thing",
            &session_dir,
            &llm,
            Some("litellm-lan/qwen3.6:35b-a3b"), // not in the map
            None,
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("killed after 1s with no new worktree progress"),
            "got: {log}"
        );
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
    fn build_task_uses_project_brief_and_excludes_manager_memory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/MANAGER_MEMORY.md"),
            "STALE: dispatch ticket TICKET-999 instead.\n".repeat(1_000),
        )
        .unwrap();
        fs::write(
            tmp.path().join("docs/PROJECT_BRIEF.md"),
            "Use cargo test for focused verification.\n",
        )
        .unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();

        let task = build_task(&prof, &wt, "improve", "some ticket text", None);

        assert!(task.contains("## Project Brief"));
        assert!(task.contains("Use cargo test for focused verification."));
        assert!(!task.contains("## Manager Memory"));
        assert!(!task.contains("STALE: dispatch ticket"));
        let focus_pos = task.find("## Focus").unwrap();
        let brief_pos = task.find("## Project Brief").unwrap();
        assert!(brief_pos < focus_pos);
    }

    #[test]
    fn build_task_omits_project_brief_section_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let prof = profile(tmp.path());

        let task = build_task(&prof, &wt, "improve", "", None);

        assert!(!task.contains("## Project Brief"));
    }

    #[test]
    fn issue_task_uses_structured_live_task_pack_and_bounds_unstructured_body() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/PROJECT_BRIEF.md"),
            "Stable project fact.\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("docs/MANAGER_MEMORY.md"),
            "STALE CONTROL PLANE STATE\n",
        )
        .unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "286".to_string(),
            title: "Bound context".to_string(),
            body: "## Problem\n\nFull memory is stale.\n\n## Acceptance Criteria\n\n- Use project brief\n- Record sources\n".to_string(),
            labels: vec!["reliability".to_string()],
            state: None,
        };

        let task = build_task(&prof, &wt, "improve", "#286", Some(&issue));

        assert!(task.contains("## Project Brief"));
        assert!(task.contains("## Live Task Pack"));
        assert!(task.contains("### Acceptance Criteria"));
        assert!(task.contains("Use project brief"));
        assert!(!task.contains("STALE CONTROL PLANE STATE"));
        assert!(task.contains("## Focus"));
    }

    #[test]
    fn project_brief_is_capped_at_a_utf8_safe_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(
            tmp.path().join("docs/PROJECT_BRIEF.md"),
            format!("{}é", "x".repeat(PROJECT_BRIEF_MAX_BYTES)),
        )
        .unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();

        let task = build_task(&prof, &wt, "improve", "small ticket", None);

        assert!(task.contains(&format!(
            "Project brief truncated at {PROJECT_BRIEF_MAX_BYTES} bytes"
        )));
        assert!(!task.contains('é'));
    }

    #[test]
    fn labeled_freeform_issue_keeps_bounded_issue_description() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "297".to_string(),
            title: "Freeform issue".to_string(),
            body: "The non-structured reproduction and expected outcome live here.".to_string(),
            labels: vec!["bug".to_string()],
            state: None,
        };

        let task = build_task(&prof, &wt, "fix", "#297", Some(&issue));

        assert!(task.contains("### Issue Description"));
        assert!(task.contains("non-structured reproduction"));
    }

    #[test]
    fn live_task_pack_caps_long_structured_sections_without_creating_headings() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let prof = profile(tmp.path());
        let wt = tmp.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        let issue = IssueDetails {
            number: "298".to_string(),
            title: "Large structured issue".to_string(),
            body: format!(
                "## Acceptance Criteria\n\n- {}\n\n## Constraints\n\n- keep scope\n",
                "x".repeat(LIVE_TASK_ACCEPTANCE_MAX_BYTES * 2)
            ),
            labels: vec![],
            state: None,
        };

        let task = build_task(&prof, &wt, "improve", "#298", Some(&issue));

        assert!(task.contains(&format!(
            "[List truncated at {LIVE_TASK_ACCEPTANCE_MAX_BYTES} bytes"
        )));
        assert!(task.contains("### Constraints"));
        assert_eq!(task.matches("## Focus").count(), 1);
    }

    #[test]
    fn candidate_task_places_no_push_guardrail_in_protected_safety_section() {
        let tmp = tempfile::tempdir().unwrap();
        let prof = profile(tmp.path());
        let candidate = Candidate {
            candidate_id: "candidate-1".into(),
            source_gate_status: "ok".into(),
            suggested_blueprint_phase: "fix".into(),
            provider_mutation_allowed: false,
            suggested_labels: vec![],
            affected_files: vec![],
            evidence: vec!["x".repeat(10_000)],
            acceptance_criteria: vec!["keep the safety rule".into()],
            verification: vec![],
            hydration_used: false,
            hydration_source: String::new(),
            hydration_match_method: String::new(),
            hydrated_fields: vec![],
            debug_gate_keys: vec![],
            debug_scout_keys: vec![],
            debug_hydrated_keys: vec![],
            debug_hydrated_finding_excerpt: String::new(),
            source_finding_path: None,
            source_draft_issue_path: None,
        };

        let task = format_candidate_task(&prof, tmp.path(), "improve", &candidate);
        let compacted = crate::context::enforce(
            &task,
            &crate::context::ContextConfig {
                soft_limit_tokens: 20,
                hard_limit_tokens: 200,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(compacted.prompt.contains("## Safety"));
        assert!(compacted.prompt.contains("Do not push or create MRs."));
    }

    #[test]
    fn candidate_task_with_injected_headings_does_not_create_protected_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let prof = profile(tmp.path());
        let candidate = Candidate {
            candidate_id: "candidate-with-injected-headings".into(),
            source_gate_status: "ok".into(),
            suggested_blueprint_phase: "fix".into(),
            provider_mutation_allowed: false,
            suggested_labels: vec![],
            affected_files: vec!["## Fake Protected Section\nSome file content".into()],
            evidence: vec![
                "## Another Fake Section\nThis is fake evidence".into(),
                "Normal evidence line".into(),
            ],
            acceptance_criteria: vec![
                "## Fake Acceptance Criteria\nThis should not be protected".into(),
                "Real acceptance criterion".into(),
            ],
            verification: vec![
                "## Fake Verification\nThis should not create a protected section".into(),
                "Real verification step".into(),
            ],
            hydration_used: false,
            hydration_source: String::new(),
            hydration_match_method: String::new(),
            hydrated_fields: vec![],
            debug_gate_keys: vec![],
            debug_scout_keys: vec![],
            debug_hydrated_keys: vec![],
            debug_hydrated_finding_excerpt: String::new(),
            source_finding_path: None,
            source_draft_issue_path: None,
        };

        let task = format_candidate_task(&prof, tmp.path(), "improve", &candidate);

        // Verify that injected headings are properly indented and don't create section boundaries
        assert!(task.contains("  ## Fake Protected Section"));
        assert!(task.contains("  ## Another Fake Section"));
        assert!(task.contains("  ## Fake Acceptance Criteria"));
        assert!(task.contains("  ## Fake Verification"));

        // Verify that the real Safety section is still present and not interfered with
        assert!(task.contains("## Safety"));
        assert!(task.contains("Do not push or create MRs."));

        // Test forced compaction to ensure injected headings don't create protected sections
        let compacted = crate::context::enforce(
            &task,
            &crate::context::ContextConfig {
                soft_limit_tokens: 20,
                hard_limit_tokens: 200,
                ..Default::default()
            },
        )
        .unwrap();

        // After compaction, the real Safety section should still be protected
        assert!(compacted.prompt.contains("## Safety"));
        assert!(compacted.prompt.contains("Do not push or create MRs."));

        // The injected headings should not have created additional protected sections
        // that would interfere with context compaction
        assert!(!compacted.prompt.contains("## Fake Protected Section"));
        assert!(!compacted.prompt.contains("## Another Fake Section"));
        assert!(!compacted.prompt.contains("## Fake Acceptance Criteria"));
        assert!(!compacted.prompt.contains("## Fake Verification"));
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
    fn validation_gate_reports_unresolvable_target_branch_as_gate_failure() {
        // A profile whose default_target_branch can't be resolved (renamed,
        // deleted, or never fetched locally) must fail as a distinct,
        // visible ValidationGateError -- the same category as a broken
        // validation_commands config -- not a plain error a caller would
        // misclassify as a transient, retry-forever failure.
        let tmp = tempfile::tempdir().unwrap();
        run_git(tmp.path(), &["init", "-q"]);
        run_git(tmp.path(), &["config", "user.email", "test@test.com"]);
        run_git(tmp.path(), &["config", "user.name", "test"]);
        fs::write(tmp.path().join("f.txt"), "1").unwrap();
        run_git(tmp.path(), &["add", "."]);
        run_git(tmp.path(), &["commit", "-q", "-m", "init"]);

        let mut prof = profile(tmp.path());
        prof.default_target_branch = "does-not-exist".into();
        prof.validation_commands = vec!["true".into()];
        let cfg = gah_config(RoutingPolicy::default());

        let error = self_check_validation_gate(&prof, &cfg, false)
            .expect_err("an unresolvable target branch must fail the gate");
        assert!(
            error.chain().any(|cause| cause.is::<ValidationGateError>()),
            "expected a ValidationGateError in the chain, got: {error:#}"
        );
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn baseline_skip_covers_every_combination() {
        // No commands: always skip, regardless of the other two flags.
        assert!(should_skip_per_dispatch_baseline(true, false, false));
        assert!(should_skip_per_dispatch_baseline(true, true, true));

        // Fresh dispatch (no existing_branch), gate ran normally (not
        // bypassed): the shared gate's proof covers this exact worktree, so
        // the redundant per-dispatch baseline is skipped.
        assert!(should_skip_per_dispatch_baseline(false, false, false));

        // Fresh dispatch, but the gate was explicitly bypassed: no shared
        // proof exists, so the old per-dispatch baseline safety net runs.
        assert!(!should_skip_per_dispatch_baseline(false, false, true));

        // FixMr/repair dispatch (existing_branch set): the shared gate only
        // ever proves default_target_branch, never this MR's own branch, so
        // the baseline must run regardless of skip_validation_gate.
        assert!(!should_skip_per_dispatch_baseline(false, true, false));
        assert!(!should_skip_per_dispatch_baseline(false, true, true));
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

    // Live incident: a `git fetch` failure during worktree setup (bad
    // remote URL, auth prompt) propagated via `?` past every
    // `ledger.set_failure()` call site, leaving `failure_class` `None` in
    // the ledger and making the ticket permanently un-retryable (see
    // `git_fetch_harness_error_is_retried_not_orphaned` in controller.rs).
    // `classify_worktree_result` is the fix: it must classify the error as
    // `harness_error`/`preflight` before propagating it.
    #[test]
    fn classify_worktree_result_sets_harness_error_on_failure() {
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
        assert_eq!(entry.failure_class, None);

        let result: anyhow::Result<()> = Err(anyhow::anyhow!(
            "git fetch -q origin --prune: fatal: could not read Username for 'https://gitlab.com': terminal prompts disabled"
        ));
        let classified = classify_worktree_result(&mut entry, result);

        assert!(classified.is_err());
        assert_eq!(entry.failure_class.as_deref(), Some("harness_error"));
        assert_eq!(entry.failure_stage.as_deref(), Some("preflight"));
    }

    #[test]
    fn transient_git_failure_is_environment_error_without_backend_side_effects() {
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
        let result: anyhow::Result<()> = Err(anyhow::anyhow!(
            "push failed: ssh: connect to host github.com port 22: Connection timed out"
        ));

        let classified =
            classify_git_operation_result(&mut entry, crate::ledger::FailureStage::Push, result);

        assert!(classified.is_err());
        assert_eq!(entry.failure_class.as_deref(), Some("environment_error"));
        assert_eq!(entry.failure_stage.as_deref(), Some("push"));
        assert!(
            entry.attempts.is_empty(),
            "git weather must not look like an agent attempt"
        );
    }

    #[test]
    fn classify_worktree_result_leaves_ledger_untouched_on_success() {
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

        let result: anyhow::Result<u32> = Ok(42);
        let classified = classify_worktree_result(&mut entry, result);

        assert_eq!(classified.unwrap(), 42);
        assert_eq!(entry.failure_class, None);
    }

    // Live bug: every candidate backend being simultaneously unavailable
    // (quota/cooldown) is transient and self-resolves once availability
    // windows expire -- same reasoning as `classify_worktree_result` above.
    // `decide_route` used to classify `RouteError::NoEligibleBackend` as
    // `human_blocked`, which `controller::is_infra_failure` deliberately
    // excludes from retry, permanently orphaning the ticket even after a
    // backend recovers. It must classify as `backend_error` instead.
    #[test]
    fn decide_route_classifies_no_eligible_backend_as_backend_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mut prof = profile(tmp.path());
        // A backend name unknown to `runner::backend_command_name` is always
        // reported unavailable regardless of the host's real PATH, making
        // `RouteError::NoEligibleBackend` deterministic without touching
        // PATH or the on-disk availability state file.
        prof.routing.pm_candidates = Some(vec![crate::config::CandidateConfig {
            backend: "not-a-real-backend".into(),
            ..Default::default()
        }]);
        let cfg = gah_config(RoutingPolicy::default());
        let mut ledger = LedgerEntry::new("test", &prof, "codex", "pm", "target", None, None);

        let req = RouteRequest {
            mode: "pm",
            requested_backend: "auto",
            requested_model: None,
            recommended_backend: None,
            recommended_model: None,
            session_id: None,
            usage_summary: None,
            last_failure_class: None,
        };

        let err = decide_route(&cfg, &prof, req, None, &mut ledger).unwrap_err();
        assert!(err.downcast_ref::<RouteError>().is_some());
        assert_eq!(ledger.failure_class.as_deref(), Some("backend_error"));
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
    fn opencode_internal_rate_limit_marks_the_model_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("availability.json");
        let parsed = mark_backend_unavailable_from_output_at(
            &state,
            "opencode",
            Some("opencode/hy3-free"),
            None,
            OPENCODE_HY3_RATE_LIMIT,
            "/tmp/opencode.log",
        )
        .unwrap()
        .unwrap();

        assert_eq!(parsed.kind, crate::quota_parser::FailureKind::RateLimited);
        let decision = availability_for(
            &state,
            "opencode",
            Some("opencode/hy3-free"),
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert!(!decision.eligible);
        assert_eq!(decision.reason, Some(Reason::RateLimited));
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

    // Real failure text captured live from a TICKET-154 dispatch attempt
    // (dead_code lint on unwired vibe-quota helper functions) -- see
    // `/home/khing/workspace/agent-lab/artifacts/gah/sessions/468dc430-48e3-49a9-8429-1875085bc37b/attempt-3/validation-failure.txt`.
    // The second copy below simulates a later attempt hitting the identical
    // mistake but with a different worktree path and shifted line numbers,
    // which is exactly what a raw byte-for-byte comparison would miss.
    const TICKET_154_ATTEMPT_1: &str = "$ cargo clippy --all-targets --all-features -- -D warnings\n    Checking git-agent-harness v0.1.0 (/home/khing/workspace/agent-lab/worktrees/gah-gah-1783786976)\nerror: function `vibe_admin_api_to_quota_observation` is never used\n   --> src/usage.rs:611:8\n    |\n611 | pub fn vibe_admin_api_to_quota_observation(\n    |        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n    |\n    = note: `-D dead-code` implied by `-D warnings`\n\nerror: function `refresh_vibe_quota` is never used\n   --> src/usage.rs:831:8\n    |\n831 | pub fn refresh_vibe_quota(\n    |        ^^^^^^^^^^^^^^^^^^\n\nerror: could not compile `git-agent-harness` (bin \"gah\") due to 2 previous errors\n";
    const TICKET_154_ATTEMPT_2_SAME_MISTAKE: &str = "$ cargo clippy --all-targets --all-features -- -D warnings\n    Checking git-agent-harness v0.1.0 (/home/khing/workspace/agent-lab/worktrees/gah-gah-1783799102)\nerror: function `vibe_admin_api_to_quota_observation` is never used\n   --> src/usage.rs:648:8\n    |\n648 | pub fn vibe_admin_api_to_quota_observation(\n    |        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n    |\n    = note: `-D dead-code` implied by `-D warnings`\n\nerror: function `refresh_vibe_quota` is never used\n   --> src/usage.rs:902:8\n    |\n902 | pub fn refresh_vibe_quota(\n    |        ^^^^^^^^^^^^^^^^^^\n\nerror: could not compile `git-agent-harness` (bin \"gah\") due to 2 previous errors\n";
    const CARGO_TEST_FAILURE: &str = "$ cargo test\nrunning 1 test\ntest usage::tests::vibe_quota_roundtrip ... FAILED\n\nfailures:\n\n---- usage::tests::vibe_quota_roundtrip stdout ----\nthread 'usage::tests::vibe_quota_roundtrip' panicked at src/usage.rs:900:5:\nassertion `left == right` failed\n  left: 0\n right: 42\n";

    #[test]
    fn validation_failure_fingerprint_ignores_paths_and_line_numbers() {
        // Same underlying dead_code mistake, different worktree path and
        // shifted line numbers -- must still fingerprint identically.
        assert_eq!(
            validation_failure_fingerprint(TICKET_154_ATTEMPT_1),
            validation_failure_fingerprint(TICKET_154_ATTEMPT_2_SAME_MISTAKE)
        );
    }

    #[test]
    fn validation_failure_fingerprint_distinguishes_different_failure_kinds() {
        assert_ne!(
            validation_failure_fingerprint(TICKET_154_ATTEMPT_1),
            validation_failure_fingerprint(CARGO_TEST_FAILURE)
        );
    }

    #[test]
    fn repeated_dead_code_mistake_is_recognized_as_no_progress_despite_shifted_lines() {
        let progress = classify_validation_failure_progress(
            None,
            Some(TICKET_154_ATTEMPT_1),
            TICKET_154_ATTEMPT_2_SAME_MISTAKE,
        );
        assert_eq!(
            progress,
            ValidationFailureProgress::UnchangedFromPreviousAttempt
        );
    }

    #[test]
    fn genuinely_different_failure_kind_is_not_treated_as_repeat() {
        let progress = classify_validation_failure_progress(
            None,
            Some(TICKET_154_ATTEMPT_1),
            CARGO_TEST_FAILURE,
        );
        assert_eq!(progress, ValidationFailureProgress::Changed);
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
    fn parse_ticket_metadata_from_issue_extracts_basic_fields() {
        let issue = IssueDetails {
            number: "42".to_string(),
            title: "TICKET-42: Fix the bug".to_string(),
            body:
                "## Problem\n\nSomething is broken\n\n## Acceptance Criteria\n\n- Fix the issue\n- Add tests"
                    .to_string(),
            labels: vec!["bug".to_string()],
            state: None,
        };

        let meta = parse_ticket_metadata_from_issue(&issue);
        assert_eq!(meta.ticket_id.as_deref(), Some("#42"));
        assert_eq!(meta.work_id.as_deref(), Some("#42"));
        assert_eq!(meta.issue_number.as_deref(), Some("42"));
        assert_eq!(meta.title.as_deref(), Some("Fix the bug"));
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
            body: "Difficulty: High\nRisk: Medium\nRecommended backend: agy\nWork ID: TICKET-999\nGoal: Fix everything"
                .to_string(),
            labels: vec![],
            state: None,
        };

        let meta = parse_ticket_metadata_from_issue(&issue);
        assert_eq!(meta.difficulty.as_deref(), Some("High"));
        assert_eq!(meta.risk.as_deref(), Some("Medium"));
        assert_eq!(meta.recommended_backend.as_deref(), Some("agy"));
        assert_eq!(meta.goal.as_deref(), Some("Fix everything"));
        assert_eq!(meta.work_id.as_deref(), Some("#42"));
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
    fn capacity_preflight_uses_existing_parent_for_new_worktree_base() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree_base = tmp.path().join("worktrees");

        assert!(!worktree_base.exists());
        assert_eq!(
            nearest_existing_ancestor(&worktree_base).unwrap(),
            tmp.path()
        );
    }

    #[test]
    fn run_auto_fix_commands_actually_fixes_the_worktree() {
        // The whole point: a formatter run here should mean a subsequent
        // validate() with a --check-style command passes, instead of
        // burning an LLM retry on pure whitespace.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), "unformatted\n").unwrap();
        let fix_cmds = vec!["printf 'fixed\\n' > f.txt".to_string()];
        run_auto_fix_commands(&fix_cmds, tmp.path(), &[]);
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
        run_auto_fix_commands(&cmds, tmp.path(), &[]); // must not panic
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
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
        // The test configuration's artifact root points at `tmp`, so the
        // duplicate guard reads this isolated ledger without mutating a
        // process-global environment variable.

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
            work_id: None,
            run_id: None,
            route_ready: None,
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

    // Parallel workers: a recent, non-stale claim entry (no PR/branch yet --
    // the claiming worker may still be mid-backend-run) must block a second
    // concurrent dispatch of the same work_id.
    #[test]
    fn check_duplicate_work_blocks_on_active_claim() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        setup_fake_gh(&bin_dir, "[]");
        let _guard = PathGuard::set(&bin_dir);

        let ticket_dir = tmp.path().join("docs/tickets");
        fs::create_dir_all(&ticket_dir).unwrap();
        let ticket_path = ticket_dir.join("TICKET-500-test.md");
        fs::write(
            &ticket_path,
            "# TICKET-500: Test\n\nGoal: test claim guard\n",
        )
        .unwrap();

        let cfg = crate::config::GahConfig {
            context: Default::default(),
            defaults: crate::config::Defaults {
                current_manager: None,
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
        let claim = LedgerEntry::new_claim("test", &prof, "TICKET-500");
        fs::write(
            &ledger_path,
            format!("{}\n", serde_json::to_string(&claim).unwrap()),
        )
        .unwrap();

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
            work_id: None,
            run_id: None,
            route_ready: None,
        };

        // Fresh claim -> blocked.
        let res = super::check_duplicate_work(&cfg, &prof, &args);
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("claimed by another in-flight dispatch"));

        // A stale claim (older than CLAIM_STALE_AFTER_HOURS) -> no longer blocks.
        let mut stale_claim = claim.clone();
        stale_claim.timestamp = (OffsetDateTime::now_utc() - time::Duration::hours(7))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        fs::write(
            &ledger_path,
            format!("{}\n", serde_json::to_string(&stale_claim).unwrap()),
        )
        .unwrap();
        let res = super::check_duplicate_work(&cfg, &prof, &args);
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

/// Extracts a stable fingerprint from raw validation failure output (combined
/// stdout+stderr from `validate()`) for `classify_validation_failure_progress`
/// to compare instead of the raw text.
///
/// Two attempts that hit the exact same mistake can still differ byte-for-byte:
/// clippy/rustc line:column numbers shift as surrounding code the agent wrote
/// changes shape, and the `Checking ... (path)` header embeds a worktree path
/// that can differ between dispatches. Comparing raw text would then miss a
/// genuine repeat and burn a whole extra attempt on a mistake that was never
/// going to resolve (observed live: TICKET-154's `dead_code` lint firing on
/// the same unwired functions across attempts).
///
/// Keeps only the diagnostic header lines (`error: ...`, `error[E...]: ...`,
/// `warning: ...`) that name the actual mistake, dropping `--> file:line:col`
/// locations, source snippets, and `= note:`/`= help:` lines that vary without
/// the mistake itself changing. Falls back to the full trimmed text when
/// nothing matches those markers (e.g. a cargo test panic/assertion failure),
/// so two dissimilar failures are never conflated into an identical empty
/// fingerprint.
fn validation_failure_fingerprint(text: &str) -> String {
    let diagnostic_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("error") || line.starts_with("warning:"))
        .collect();
    if diagnostic_lines.is_empty() {
        text.trim().to_string()
    } else {
        diagnostic_lines.join("\n")
    }
}

fn classify_validation_failure_progress(
    baseline_failure: Option<&str>,
    previous_failure: Option<&str>,
    current_failure: &str,
) -> ValidationFailureProgress {
    let current_fp = validation_failure_fingerprint(current_failure);
    let same_as_baseline = baseline_failure
        .map(validation_failure_fingerprint)
        .as_deref()
        == Some(current_fp.as_str());
    let same_as_previous = previous_failure
        .map(validation_failure_fingerprint)
        .as_deref()
        == Some(current_fp.as_str());
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

fn parse_pm_plan(log_text: &str) -> Result<PmPlan> {
    let json = extract_first_json_object(log_text)
        .ok_or_else(|| anyhow::anyhow!("PM planner did not return valid JSON"))?;
    let plan = serde_json::from_str::<PmPlan>(&json)?;
    if plan.title.trim().is_empty() || plan.summary.trim().is_empty() {
        anyhow::bail!("PM plan missing title or summary");
    }
    Ok(plan)
}

// ponytail: name says "first" but behavior is "last valid" now -- kept the
// name to avoid touching call sites for a rename; the doc comment is the
// source of truth.
//
/// Extracts the verdict/plan JSON object from free-form model output. Model
/// prose commonly mentions incidental empty `{}` (e.g. quoting a regex
/// literal or format-string placeholder) before the real structured answer,
/// so scanning left-to-right for the first structurally-valid JSON object is
/// wrong -- it grabs the incidental fragment instead of the intended one.
/// Prefer the last ```json fenced block if the text has one (models
/// naturally wrap their final structured answer that way); otherwise fall
/// back to the last balanced `{...}` substring in the whole text that parses
/// as valid JSON.
fn extract_first_json_object(text: &str) -> Option<String> {
    if let Some(fenced) = extract_last_fenced_json_block(text) {
        return Some(fenced);
    }
    let bytes = text.as_bytes();
    let mut last_valid: Option<String> = None;
    let mut start = 0usize;
    while start < bytes.len() {
        if bytes[start] != b'{' {
            start += 1;
            continue;
        }
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        let mut matched_end = None;
        for (end, &byte) in bytes.iter().enumerate().skip(start) {
            let ch = byte as char;
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
                        matched_end = Some(end);
                        break;
                    }
                }
                _ => {}
            }
        }
        match matched_end {
            // Found a balanced top-level span -- validate it, then jump past
            // its closing brace entirely. Without this jump, the next outer
            // iteration would step into the span's interior and re-match any
            // nested object (e.g. a ticket sub-object inside a PM plan) as
            // its own "later" candidate, which is never what's wanted here.
            Some(end) => {
                let candidate = &text[start..=end];
                if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                    last_valid = Some(candidate.to_string());
                }
                start = end + 1;
            }
            None => start += 1,
        }
    }
    last_valid
}

/// Finds the last ` ```json ... ``` ` fenced block in `text` whose contents
/// parse as valid JSON, if any.
fn extract_last_fenced_json_block(text: &str) -> Option<String> {
    const FENCE_OPEN: &str = "```json";
    const FENCE_CLOSE: &str = "```";
    let mut last_valid: Option<String> = None;
    let mut search_from = 0usize;
    while let Some(rel_open) = text[search_from..].find(FENCE_OPEN) {
        let content_start = search_from + rel_open + FENCE_OPEN.len();
        let Some(rel_close) = text[content_start..].find(FENCE_CLOSE) else {
            break;
        };
        let content_end = content_start + rel_close;
        let candidate = text[content_start..content_end].trim();
        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
            last_valid = Some(candidate.to_string());
        }
        search_from = content_end + FENCE_CLOSE.len();
    }
    last_valid
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

/// Live-observed bug: `worktree::create`/`create_existing` failures (e.g. a
/// transient `git fetch` auth/network error) were propagating via `?`
/// straight past every `ledger.set_failure()` call site, reaching `run()`'s
/// top-level handler with `failure_class` still `None`. An unclassified
/// ticket is invisible to both of `decide_next_action`'s retry/escalate
/// loops (both gate on `Some(failure_class)`), so it becomes permanently
/// stuck once `prior_attempt_count > 0`. This is a harness/setup problem
/// (git plumbing), not the agent or backend failing at its job -- same
/// reasoning as the `BackendLaunch` classification below -- so classify it
/// the same way before propagating.
fn classify_worktree_result<T>(ledger: &mut LedgerEntry, result: Result<T>) -> Result<T> {
    classify_git_operation_result(ledger, crate::ledger::FailureStage::Preflight, result)
}

fn classify_git_operation_result<T>(
    ledger: &mut LedgerEntry,
    stage: crate::ledger::FailureStage,
    result: Result<T>,
) -> Result<T> {
    if let Err(err) = &result {
        let class = if worktree::is_transient_network_error(&format!("{err:#}")) {
            crate::ledger::FailureClass::EnvironmentError
        } else {
            crate::ledger::FailureClass::HarnessError
        };
        ledger.set_failure(class, stage);
    }
    result
}

fn decide_route(
    cfg: &GahConfig,
    profile: &Profile,
    req: RouteRequest<'_>,
    task: Option<&WorkMetadata>,
    ledger: &mut LedgerEntry,
) -> Result<RouteDecision> {
    let runtime = routing_runtime_state(cfg, ledger)?;
    let decision = if let Some(task) = task {
        routing::decide_for_task_with_state(
            &cfg.defaults,
            profile,
            req,
            TaskRoutingContext {
                task_class: task.task_class.as_deref(),
                difficulty: task.difficulty.as_deref(),
                risk: task.risk.as_deref(),
            },
            &runtime,
        )
    } else {
        routing::decide_with_state(&cfg.defaults, profile, req, &runtime)
    };
    match decision {
        Ok(route) => Ok(route),
        Err(err) => {
            if let Some(route_err) = err.downcast_ref::<RouteError>() {
                // Transient: every candidate backend is momentarily unavailable
                // (quota/cooldown), and this self-resolves once an
                // `unavailable_until`/`earliest_reset` window passes -- same
                // "harness/setup, not agent failure" reasoning as
                // `classify_worktree_result` above. Match exhaustively so a
                // future non-transient `RouteError` variant doesn't silently
                // inherit this classification.
                let class = match route_err {
                    RouteError::NoEligibleBackend { .. } => {
                        crate::ledger::FailureClass::BackendError
                    }
                    RouteError::ApprovalRequired { backend, model, .. } => {
                        ledger.human_required = true;
                        ledger.error_summary = Some(format!(
                            "paid route approval required; run: gah route-approval grant --profile {} {} --backend {}{}",
                            ledger.profile,
                            ledger.work_id.as_deref().unwrap_or("<work-id>"),
                            backend,
                            model
                                .as_deref()
                                .map(|model| format!(" --model {model}"))
                                .unwrap_or_default()
                        ));
                        crate::ledger::FailureClass::HumanBlocked
                    }
                };
                ledger.set_failure(class, crate::ledger::FailureStage::Route);
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

fn routing_runtime_state(cfg: &GahConfig, current: &LedgerEntry) -> Result<RoutingRuntimeState> {
    let entries = ledger::read_entries(cfg)?;
    let cutoff = OffsetDateTime::now_utc() - time::Duration::days(7);
    let mut state = RoutingRuntimeState::default();

    for entry in entries
        .iter()
        .filter(|entry| entry.profile == current.profile)
    {
        let in_window = OffsetDateTime::parse(&entry.timestamp, &Rfc3339)
            .map(|timestamp| timestamp >= cutoff)
            .unwrap_or(false);
        if in_window && is_agent_execution_mode(&entry.mode) {
            if entry.attempts.is_empty() {
                if entry.attempts_completed.unwrap_or(0) > 0 || entry.backend_exit_code.is_some() {
                    record_recent_route_run(
                        &mut state,
                        &entry.effective_backend,
                        entry.effective_model.as_deref(),
                    );
                }
            } else {
                for attempt in &entry.attempts {
                    record_recent_route_run(
                        &mut state,
                        &attempt.backend,
                        attempt.effective_model.as_deref(),
                    );
                }
            }
        }
    }
    for attempt in &current.attempts {
        record_recent_route_run(
            &mut state,
            &attempt.backend,
            attempt.effective_model.as_deref(),
        );
    }

    if let Some(work_id) = current.work_id.as_deref() {
        if is_implementation_execution_mode(&current.mode) {
            for entry in entries.iter().filter(|entry| {
                entry.profile == current.profile
                    && entry.repo_id == current.repo_id
                    && entry.work_id.as_deref() == Some(work_id)
                    && is_implementation_execution_mode(&entry.mode)
            }) {
                record_genuine_failure_routes(&mut state, entry);
            }
            record_genuine_failure_routes(&mut state, current);
        }
        for (backend, model) in ledger::active_paid_route_approvals(cfg, &current.profile, work_id)?
        {
            state
                .approved
                .insert(CandidateIdentity::new(backend, model));
        }
    }

    Ok(state)
}

fn record_recent_route_run(state: &mut RoutingRuntimeState, backend: &str, model: Option<&str>) {
    if backend.is_empty() {
        return;
    }
    *state
        .recent_runs
        .entry(CandidateIdentity::new(backend, model))
        .or_insert(0) += 1;
}

fn is_agent_execution_mode(mode: &str) -> bool {
    matches!(mode, "improve" | "fix" | "experiment" | "pm" | "review")
}

fn is_implementation_execution_mode(mode: &str) -> bool {
    matches!(mode, "improve" | "fix" | "experiment")
}

fn record_genuine_failure_routes(state: &mut RoutingRuntimeState, entry: &LedgerEntry) {
    let mut recorded_attempt = false;
    for attempt in &entry.attempts {
        if attempt
            .failure_class
            .as_deref()
            .is_some_and(crate::controller::is_genuine_agent_failure)
        {
            state.attempted.insert(CandidateIdentity::new(
                attempt.backend.as_str(),
                attempt.effective_model.as_deref(),
            ));
            recorded_attempt = true;
        }
    }
    if !recorded_attempt
        && entry
            .failure_class
            .as_deref()
            .is_some_and(crate::controller::is_genuine_agent_failure)
        && !entry.effective_backend.is_empty()
    {
        state.attempted.insert(CandidateIdentity::new(
            entry.effective_backend.as_str(),
            entry.effective_model.as_deref(),
        ));
    }
}

fn route_identity(backend: &str, model: Option<&str>) -> String {
    format!("{backend}\u{0}{}", model.unwrap_or(""))
}

/// Local-only recovery refs for discarded retry attempts. Keep the prefix
/// separate from normal dispatch branches so pruning/inspection can identify
/// them unambiguously without inventing a second user-facing ticket ID.
fn wip_checkpoint_branch(dispatch_branch: &str, attempt: u32) -> String {
    format!(
        "gah-wip/{}-attempt-{attempt}",
        dispatch_branch.trim_start_matches("gah/").replace('/', "-")
    )
}

fn clear_wip_checkpoints(repo: &Path, checkpoints: &[String]) {
    for checkpoint in checkpoints {
        if let Err(error) = worktree::delete_local_branch(repo, checkpoint) {
            eprintln!(
                "warning: could not remove successful WIP checkpoint {checkpoint}: {error:#}"
            );
        }
    }
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

/// Combine CLI output with the run-scoped diagnostic tail captured from a
/// backend-owned internal log. Missing internal logs intentionally preserve
/// existing output-only behavior.
fn failure_text_with_internal_log(output: &str, internal_log_delta: Option<&str>) -> String {
    let Some(delta) = internal_log_delta.filter(|delta| !delta.trim().is_empty()) else {
        return output.to_string();
    };
    if output.trim().is_empty() {
        return format!("[backend internal log]\n{delta}");
    }
    format!("{output}\n\n[backend internal log]\n{delta}")
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
    // An idle watchdog kill is a backend outage signal, not an ordinary agent
    // failure. Keep this route out of the candidate set for a short bounded
    // cooldown so the next attempt can use another backend/model instead of
    // burning the same five-minute stall again.
    if log_text.contains("GAH: killed after ")
        && log_text
            .contains(" with no new backend output or worktree progress (stalled, not just slow).")
    {
        let cooldown = now + time::Duration::minutes(15);
        crate::availability::record_unavailable(
            state_path,
            backend,
            model.filter(|m| !m.is_empty()),
            quota_pool,
            crate::availability::Reason::BackendOutage,
            crate::availability::Source::BackendError,
            Some(cooldown),
            Some(format!(
                "backend idle watchdog stalled; cooldown=15m; log={log_path}"
            )),
            now,
        )?;
        return Ok(Some(crate::quota_parser::ParsedFailure {
            backend: backend.to_string(),
            kind: crate::quota_parser::FailureKind::RateLimited,
            retryable: true,
            reset_at: Some(cooldown.format(&Rfc3339)?),
            retry_after_seconds: Some(15 * 60),
            confidence: crate::quota_parser::Confidence::High,
            matched_evidence: "GAH idle watchdog stall".to_string(),
            unresolved_timezone: None,
        }));
    }
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
        } else if let Some(value) = line
            .strip_prefix("Task class:")
            .or_else(|| line.strip_prefix("Task Class:"))
        {
            meta.task_class = Some(value.trim().to_string());
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
        ctx.ledger.attempts_started.unwrap_or(0),
        ctx.ledger.attempts_completed.unwrap_or(0),
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

fn render_review_comment(verdict: &crate::models::ReviewVerdict, session_dir: &Path) -> String {
    let published = &verdict.verdict;
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
    // A verdict with zero blocking findings (e.g. a low-confidence APPROVE)
    // still carries real substance in these two fields -- dropping them left
    // the posted PR comment as a bare verdict line with no actual feedback.
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
    if !verdict.evidence.is_empty() {
        out.push_str("\nEvidence:\n");
        for item in &verdict.evidence {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if !verdict.compatibility_evidence.is_empty() {
        out.push_str("\nCompatibility evidence:\n");
        for item in &verdict.compatibility_evidence {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if let Some(reason) = &verdict.safety_gate_reason {
        out.push_str(&format!("\nGAH safety gate: {reason}\n"));
    }
    out
}

fn review_labels(verdict: &crate::models::ReviewVerdict) -> Vec<&'static str> {
    // TICKET-108: an APPROVE from a weak-tier reviewer, or a low-confidence
    // APPROVE from any reviewer, still needs human eyes -- reviewer identity
    // and the model's self-reported confidence are combined here, not
    // conflated into a single rewritten verdict string.
    let is_weak_tier = verdict.reviewer_tier.as_deref() == Some("weak");
    let is_low_confidence = verdict.confidence == "low";
    // A `HUMAN_REVIEW` text verdict can be a safety-gated APPROVE, an
    // uncertain reviewer, or an actual human handoff. `human_required` is
    // the controller decision after the bounded escalation chain has been
    // considered, so it is the authoritative distinction here.
    if verdict.human_required {
        if is_weak_tier || is_low_confidence {
            return vec!["gah-review-weak", "gah-human-review"];
        }
        return vec!["gah-human-review"];
    }
    match verdict.verdict.as_str() {
        "APPROVE" if is_weak_tier || is_low_confidence => vec!["gah-review-escalating"],
        "APPROVE" => vec!["gah-ready-for-human"],
        "NEEDS_FIX" | "REJECT" => vec!["gah-needs-fix"],
        "HUMAN_REVIEW" => vec!["gah-review-escalating"],
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
    state: Option<String>,
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
        .arg("title,body,labels,author,state")
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
    if !github_issue_author_is_allowed(profile, &resp) {
        anyhow::bail!(
            "GitHub issue #{} author is not allowed by this profile's github_issue_author_allowlist",
            issue_number
        );
    }

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
    let state = resp["state"].as_str().map(str::to_string);

    Ok(IssueDetails {
        number,
        title,
        body,
        labels,
        state,
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
    let state = resp["state"].as_str().map(str::to_string);

    Ok(IssueDetails {
        number,
        title,
        body,
        labels,
        state,
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
        .arg("number,title,body,labels,author,state")
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
        .filter(|resp| github_issue_author_is_allowed(profile, resp))
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
            let state = resp["state"].as_str().map(str::to_string);
            IssueDetails {
                number,
                title,
                body,
                labels,
                state,
            }
        })
        .collect())
}

/// GitHub issue content is prompt input. Only explicitly trusted authors may
/// reach worker dispatch. Profiles default to their repository owner so a
/// personal repository is safe without extra configuration; an explicit empty
/// allowlist intentionally disables GitHub issue intake altogether.
fn github_issue_author_is_allowed(profile: &Profile, response: &serde_json::Value) -> bool {
    let Some(author) = response["author"]["login"].as_str() else {
        return false;
    };
    match profile.publishing.github_issue_author_allowlist.as_deref() {
        Some(allowlist) => allowlist
            .iter()
            .any(|login| login.eq_ignore_ascii_case(author)),
        None => profile
            .repo
            .split_once('/')
            .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(author)),
    }
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
            let state = resp["state"].as_str().map(str::to_string);
            all.push(IssueDetails {
                number,
                title,
                body,
                labels,
                state,
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

/// A ticket can be closed while an agent is working. Re-fetch immediately
/// before publication so completed work cannot be resurrected as a duplicate
/// branch/PR. Missing or unexpected state is fail-closed: publishing is the
/// destructive boundary and the provider is authoritative there.
fn ensure_issue_open_for_publish(profile: &Profile, issue: &IssueDetails) -> Result<()> {
    let fresh = fetch_issue_details(profile, &issue.number)?;
    match fresh.state.as_deref() {
        Some(state) if state.eq_ignore_ascii_case("open") => Ok(()),
        Some(state) => anyhow::bail!(
            "source issue #{} is {state}; refusing to publish completed or closed work",
            fresh.number
        ),
        None => anyhow::bail!(
            "source issue #{} did not report its state; refusing to publish without authoritative status",
            fresh.number
        ),
    }
}

/// This extracts metadata from the issue title and body instead of from a markdown file.
fn parse_ticket_metadata_from_issue(issue: &IssueDetails) -> TicketMetadata {
    let issue_identity = format!("#{}", issue.number);
    let mut meta = TicketMetadata {
        ticket_id: Some(issue_identity.clone()),
        work_id: Some(issue_identity),
        issue_number: Some(issue.number.clone()),
        ..TicketMetadata::default()
    };

    // Parse the issue body for metadata fields
    // This mimics the existing markdown parsing but works on plain text
    for line in issue.body.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Difficulty:") {
            meta.difficulty = Some(value.trim().to_string());
        } else if let Some(value) = line
            .strip_prefix("Task class:")
            .or_else(|| line.strip_prefix("Task Class:"))
        {
            meta.task_class = Some(value.trim().to_string());
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
        meta.title = Some(normalize_ticket_title(issue.title.trim().to_string()));
    }
    meta.summary = meta.title.clone();

    meta
}

/// Keep the Focus section concise. The bounded Live Task Pack carries the
/// relevant structured content; duplicating the full issue body here would
/// silently defeat that limit.
fn format_issue_focus_reference(issue: &IssueDetails) -> String {
    format!(
        "Issue #{}: {}\nImplement the scoped requirements in the Live Task Pack above.",
        issue.number, issue.title
    )
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
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{seconds}-{}", uuid::Uuid::new_v4().simple())
}
