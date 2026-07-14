use super::*;
use crate::config::{GahConfig, Profile};
use crate::ledger::{self, LedgerEntry};
use crate::models::AvailableTicket;
use crate::models::CandidateArtifact;
use crate::provider;
use anyhow::{Context, Result};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

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
pub(super) fn check_duplicate_work(
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

pub(super) fn ensure_dispatch_capacity(profile: &Profile, worktree_base: &Path) -> Result<()> {
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

#[cfg(test)]
#[path = "claims/tests.rs"]
mod tests;
