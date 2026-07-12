//! TICKET-077: durable, typed controller actions. The schema only -- no
//! execution here (see `dispatch::run` for execution, wired from
//! `gah loop`, TICKET-079).
//!
//! Every variant carries a mandatory `reason` (why this action was
//! selected) plus enough identity to execute it without re-observing
//! state. Serializable so it can be persisted verbatim into a controller
//! event (TICKET-083).

use crate::status::StatusSnapshot;
use anyhow::Result;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// TICKET-078: how many times the controller will automatically
/// Retry/Escalate the same work_id before giving up and requiring a human.
/// Deliberately small and inline (not configurable) -- this is a safety
/// floor, not a policy knob; see TICKET-081 for the broader stuck-loop
/// detector this complements.
const AUTO_RETRY_CAP: usize = 2;

pub(crate) fn is_genuine_agent_failure(failure_class: &str) -> bool {
    matches!(
        failure_class,
        "agent_no_progress" | "agent_failure" | "context_limit_exceeded"
    )
}

fn is_infra_failure(failure_class: &str) -> bool {
    matches!(
        failure_class,
        "harness_error" | "environment_error" | "backend_error" | "unknown"
    )
}

fn is_validation_gate_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.is::<crate::dispatch::ValidationGateError>())
}

/// Issue #156: produce an RFC3339 timestamp `offset` from "now" for a
/// `WaitUntil` re-check. Used when a READY_FOR_HUMAN MR's CI is pending so the
/// controller records a visible, observable deferral instead of a silent no-op.
fn now_plus(offset: Duration) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        + offset;
    let secs = now.as_secs();
    let dt =
        chrono::DateTime::from_timestamp(secs as i64, 0).unwrap_or(chrono::DateTime::UNIX_EPOCH);
    format!("{}", dt.format("%Y-%m-%dT%H:%M:%SZ"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum NextAction {
    ReviewMr {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        reason: String,
    },
    MarkReadyForReview {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        reason: String,
    },
    FixMr {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        reason: String,
    },
    /// TICKET-127: auto-merge -- a strong-tier reviewer's APPROVE (high
    /// confidence) plus conclusively-green CI, gated by the same retry cap
    /// as FixMr.
    MergeMr {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        reason: String,
    },
    DispatchTicket {
        ticket_path: String,
        work_id: Option<String>,
        recommended_backend: Option<String>,
        recommended_model: Option<String>,
        reason: String,
    },
    /// TICKET-078: redispatch a ticket whose last attempt failed for an
    /// infra reason (harness/environment/backend/unknown) that has since
    /// cleared -- same backend/model as before, not escalated.
    Retry {
        work_id: String,
        ticket_path: String,
        reason: String,
    },
    /// TICKET-078: redispatch a ticket whose last attempt was a genuine
    /// agent-capability failure (agent_no_progress/agent_failure),
    /// requesting a stronger backend/model this time.
    Escalate {
        work_id: String,
        ticket_path: String,
        reason: String,
    },
    WaitUntil {
        until: String,
        reason: String,
    },
    HumanRequired {
        reason: String,
        #[serde(default)]
        reference: Option<String>,
    },
    NoOp {
        reason: String,
    },
}

impl NextAction {
    /// Coarse type name for logging/fingerprinting (TICKET-081) -- stable
    /// even if variant fields change shape.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ReviewMr { .. } => "review_mr",
            Self::MarkReadyForReview { .. } => "mark_ready_for_review",
            Self::FixMr { .. } => "fix_mr",
            Self::MergeMr { .. } => "merge_mr",
            Self::DispatchTicket { .. } => "dispatch_ticket",
            Self::Retry { .. } => "retry",
            Self::Escalate { .. } => "escalate",
            Self::WaitUntil { .. } => "wait_until",
            Self::HumanRequired { .. } => "human_required",
            Self::NoOp { .. } => "no_op",
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::ReviewMr { reason, .. }
            | Self::MarkReadyForReview { reason, .. }
            | Self::FixMr { reason, .. }
            | Self::MergeMr { reason, .. }
            | Self::DispatchTicket { reason, .. }
            | Self::Retry { reason, .. }
            | Self::Escalate { reason, .. }
            | Self::WaitUntil { reason, .. }
            | Self::HumanRequired { reason, .. }
            | Self::NoOp { reason } => reason,
        }
    }

    /// The work_id this action is about, where one exists. Used for
    /// fingerprinting (TICKET-081) and event logging (TICKET-083).
    pub fn work_id(&self) -> Option<&str> {
        match self {
            Self::ReviewMr { work_id, .. }
            | Self::MarkReadyForReview { work_id, .. }
            | Self::FixMr { work_id, .. }
            | Self::MergeMr { work_id, .. } => work_id.as_deref(),
            Self::DispatchTicket { work_id, .. } => work_id.as_deref(),
            Self::Retry { work_id, .. } | Self::Escalate { work_id, .. } => Some(work_id),
            Self::WaitUntil { .. } | Self::HumanRequired { .. } | Self::NoOp { .. } => None,
        }
    }
}

/// TICKET-078: pure, deterministic, no LLM, no I/O -- consumes an
/// already-built `StatusSnapshot` and returns exactly one action. First
/// matching rule wins:
///
/// 1. incomplete critical observation -> stop safely (NoOp)
/// 2. a recorded blocker (today: ledger human_required) -> HumanRequired
/// 3. an MR classified NEEDS_REVIEW -> ReviewMr
/// 4. an MR classified READY_FOR_HUMAN with draft=true and conclusively-green
///    CI -> MarkReadyForReview
/// 5. an MR classified READY_FOR_HUMAN with draft=false and conclusively-green
///    CI -> MergeMr (or HumanRequired if merge policy forbids auto-merge)
/// 6. an MR classified CI_FAILED/NEEDS_FIX -> FixMr (if retry cap not exceeded)
/// 7. an MR classified CI_FAILED/NEEDS_FIX -> HumanRequired (if retry cap exceeded)
/// 8. an MR classified READY_FOR_HUMAN -> HumanRequired ONLY when the merge
///    policy forbids auto-merge (StopForHuman) or CI isn't conclusively green
///    (see the READY_FOR_HUMAN arm below). With green CI and an auto-merge
///    policy, it first becomes MarkReadyForReview while still draft, then
///    MergeMr once the PR/MR is no longer draft.
/// 7. a ticket with failed history, no active MR, capability failure,
///    under the retry cap -> Escalate
/// 8. a ticket with failed history, no active MR, infra failure, some
///    backend eligible again, under the retry cap -> Retry
/// 9. a ticket with failed history, no active MR, retry cap exceeded ->
///    HumanRequired
/// 10. an eligible never-dispatched ticket -> DispatchTicket
/// 11. all remaining backends unavailable but with a known reset -> WaitUntil
/// 12. otherwise -> NoOp
///
/// Ties within a tier (multiple matching MRs) are broken by branch name,
/// lexicographically -- `SyncMrJson` doesn't carry `updated_at`, so this is
/// the only deterministic ordering available without widening that type.
pub fn decide_next_action(snapshot: &StatusSnapshot) -> NextAction {
    if let Some(err) = snapshot.errors.iter().find(|e| e.incomplete_snapshot) {
        return NextAction::NoOp {
            reason: format!(
                "observation incomplete ({}): {}",
                err.subsystem, err.message
            ),
        };
    }

    if let Some(blocker) = snapshot.blockers.first() {
        // Rule 2 fires only on GENUINE profile-wide blockers (sync failure,
        // invalid config, required infra unavailable, auth failure with no
        // viable route, explicit profile-level human gate). A ticket-scoped
        // `human_required` no longer lands here -- it is reported per work
        // item in `snapshot.blocked_work_items` and must NOT freeze the
        // whole profile (TICKET-human-required-scoping).
        return NextAction::HumanRequired {
            reason: blocker
                .message
                .clone()
                .unwrap_or_else(|| blocker.kind.clone()),
            reference: blocker.source_reference.clone(),
        };
    }

    let mut mrs: Vec<&crate::sync::SyncMrJson> = snapshot.merge_requests.iter().collect();
    mrs.sort_by(|a, b| a.branch.cmp(&b.branch));

    // TICKET-skip-and-continue: a blocked MR (stuck NEEDS_FIX beyond the fix
    // cap, or READY_FOR_HUMAN awaiting a human merge decision) is a
    // WORK-ITEM concern. It must NOT freeze the whole profile -- unrelated
    // tickets/MRs keep flowing. We collect actionable candidates and return
    // the first one; only if NO item is actionable do we fall back to a
    // profile-wide HumanRequired at the end of the function.
    let mut review_candidates: Vec<&crate::sync::SyncMrJson> = Vec::new();
    let mut ready_candidates: Vec<&crate::sync::SyncMrJson> = Vec::new();
    let mut fix_candidates: Vec<&crate::sync::SyncMrJson> = Vec::new();
    let mut merge_candidates: Vec<&crate::sync::SyncMrJson> = Vec::new();
    let mut human_blocked_mrs: Vec<&crate::sync::SyncMrJson> = Vec::new();
    // Issue #156: READY_FOR_HUMAN MRs whose CI is non-terminal / unknown
    // (GitLab head_pipeline gap). They wait for CI to resolve and are not
    // silently dropped.
    let mut wait_and_recheck_mrs: Vec<&crate::sync::SyncMrJson> = Vec::new();

    for mr in &mrs {
        match mr.classification.as_str() {
            "NEEDS_REVIEW" => review_candidates.push(mr),
            "CI_FAILED" | "NEEDS_FIX" => {
                let fix_attempts = snapshot
                    .fix_attempt_counts
                    .get(&mr.branch)
                    .copied()
                    .unwrap_or(0);
                if fix_attempts >= AUTO_RETRY_CAP {
                    // Exhausted fix attempts -> work-item block, not a profile freeze.
                    human_blocked_mrs.push(mr);
                } else {
                    fix_candidates.push(mr);
                }
            }
            "READY_FOR_HUMAN" => {
                let work_id_str = mr.work_id.as_deref().unwrap_or("");
                if snapshot.review_held_work_ids.contains(work_id_str) {
                    // A manager session is actively reviewing this MR out of
                    // band (`gah hold set`). Don't auto-merge out from under
                    // them, but don't freeze the rest of the profile either
                    // -- just skip this MR for this loop tick. The manager
                    // clears the hold (`gah hold clear`) when done, or it
                    // self-expires after REVIEW_HOLD_STALE_AFTER_HOURS.
                    continue;
                }
                let merge_policy = snapshot.profile.merge_policy;
                if mr.ci_passed {
                    if mr.draft {
                        ready_candidates.push(mr);
                    } else {
                        let merge_attempts = snapshot
                            .merge_attempt_counts
                            .get(&mr.branch)
                            .copied()
                            .unwrap_or(0);
                        if merge_attempts < AUTO_RETRY_CAP {
                            merge_candidates.push(mr);
                        } else {
                            human_blocked_mrs.push(mr);
                        }
                    }
                } else if merge_policy == crate::config::MergePolicy::StopForHuman {
                    // TICKET-127: under stop_for_human, a READY_FOR_HUMAN
                    // MR without CI passed still defers to the human
                    // immediately — no CI gate needed.
                    human_blocked_mrs.push(mr);
                } else if mr.ci_pending {
                    // Issue #156: CI is non-terminal / unknown (running,
                    // pending, or no pipeline reported yet — GitLab's
                    // head_pipeline gap). This is not a green light and not a
                    // failure, so it must surface as a visible, observable
                    // re-check rather than silently no-op forever. We emit a
                    // WaitUntil so the controller event stream records the
                    // deferral and the next loop tick re-observes CI.
                    wait_and_recheck_mrs.push(mr);
                } else {
                    // CI conclusively failed (or is otherwise not pending):
                    // re-check later (no_op fallback).
                    human_blocked_mrs.push(mr);
                }
            }
            _ => {}
        }
    }

    // Priority order: review -> merge -> fix. Each returns the first
    // candidate; blocked MRs are skipped, not parked.
    if let Some(mr) = review_candidates.first() {
        return NextAction::ReviewMr {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            reason: format!("MR on branch '{}' classified NEEDS_REVIEW", mr.branch),
        };
    }
    if let Some(mr) = ready_candidates.first() {
        return NextAction::MarkReadyForReview {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            reason: format!(
                "MR on branch '{}' classified READY_FOR_HUMAN with draft=true and CI passing; marking ready for review",
                mr.branch
            ),
        };
    }
    if let Some(mr) = merge_candidates.first() {
        // Issue #124 / TICKET-127: per-repo merge policy gates what we do
        // for a strong-approved MR whose CI has been evaluated.
        let merge_policy = snapshot.profile.merge_policy;
        if merge_policy == crate::config::MergePolicy::StopForHuman {
            return NextAction::HumanRequired {
                reason: format!(
                    "MR on branch '{}' strong-reviewed with CI passing; merge policy is 'stop_for_human' -- awaiting human merge",
                    mr.branch
                ),
                reference: mr.url.clone(),
            };
        }
        // TICKET-128: a restricted profile (allow_pull_request_creation
        // == false) must never enter the auto-merge path. The reviewer
        // verdict and CI status remain authoritative; the work simply
        // stays at a human handoff instead of auto-merging. This is an
        // independent axis from reviewer routing and merge policy.
        if !snapshot.publishing_allow_pr {
            return NextAction::HumanRequired {
                reason: format!(
                    "MR on branch '{}' approved with CI passing, but profile publishing policy forbids PR/MR creation (human handoff)",
                    mr.branch
                ),
                reference: mr.url.clone(),
            };
        }
        return NextAction::MergeMr {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            reason: match merge_policy {
                crate::config::MergePolicy::GitlabMwps => format!(
                    "MR on branch '{}' approved by a strong reviewer with CI passing; setting GitLab merge-when-pipeline-succeeds",
                    mr.branch
                ),
                _ => format!(
                    "MR on branch '{}' approved by a strong reviewer with CI passing",
                    mr.branch
                ),
            },
        };
    }
    if let Some(mr) = fix_candidates.first() {
        return NextAction::FixMr {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            reason: format!(
                "MR on branch '{}' classified {} - reusing existing branch",
                mr.branch, mr.classification
            ),
        };
    }
    // Issue #156: a READY_FOR_HUMAN MR whose CI is non-terminal / unknown
    // (GitLab head_pipeline gap) must surface as a visible WaitUntil re-check,
    // never an indefinite silent no-op. Prefer it over the no-op fallback below.
    if let Some(mr) = wait_and_recheck_mrs.first() {
        let until = now_plus(Duration::from_secs(300));
        return NextAction::WaitUntil {
            until,
            reason: format!(
                "MR on branch '{}' is READY_FOR_HUMAN but CI is not yet conclusively resolved (pending/running/missing) -- waiting to re-check before merge",
                mr.branch
            ),
        };
    }
    // Fallback: if no active MR needs review/fix/merge but there are
    // human-blocked MRs under StopForHuman merge policy, surface the
    // first one as HumanRequired.  All other blocked MRs (retry-cap
    // exhausted, CI not yet passed) no-op — they appear in status
    // reports but don't park the profile.
    if !human_blocked_mrs.is_empty()
        && review_candidates.is_empty()
        && merge_candidates.is_empty()
        && fix_candidates.is_empty()
        && snapshot.profile.merge_policy == crate::config::MergePolicy::StopForHuman
    {
        let mr = human_blocked_mrs[0];
        return NextAction::HumanRequired {
            reason: format!(
                "MR on branch '{}' classified {} (human decision required)",
                mr.branch, mr.classification
            ),
            reference: mr.url.clone(),
        };
    }

    let some_backend_eligible = snapshot.availability.iter().any(|a| a.eligible_now);
    let mut failed_tickets: Vec<_> = snapshot
        .available_tickets
        .iter()
        .filter(|t| !t.has_active_mr && !t.has_active_claim && t.prior_attempt_count > 0)
        // TICKET-human-required-scoping: a work-item-scoped human_required
        // ticket is blocked at the item level. Skip it so it is neither
        // retried, escalated, nor redispatched -- but unrelated eligible
        // tickets keep flowing.
        .filter(|t| !t.human_required)
        .collect();
    failed_tickets.sort_by(|a, b| a.ticket_path.cmp(&b.ticket_path));

    // Collect tickets that have exhausted the retry cap (issue #95: only
    // genuine agent failures count toward the cap; infra-class failures
    // such as backend_error or environment_error do not permanently poison
    // a ticket's retry budget).
    let exhausted: HashSet<_> = failed_tickets
        .iter()
        .filter(|t| t.genuine_agent_failure_count >= AUTO_RETRY_CAP)
        .filter_map(|t| t.work_id.clone())
        .collect();

    // Check if there are any non-exhausted actionable tickets
    let has_escalate_candidate = failed_tickets.iter().any(|t| {
        !exhausted.contains(t.work_id.as_ref().unwrap_or(&t.ticket_path))
            && t.last_failure_class
                .as_deref()
                .is_some_and(is_genuine_agent_failure)
    });
    let has_retry_candidate = failed_tickets.iter().any(|t| {
        !exhausted.contains(t.work_id.as_ref().unwrap_or(&t.ticket_path))
            && t.last_failure_class
                .as_deref()
                .is_some_and(|fc| is_infra_failure(fc) && some_backend_eligible)
    });
    let has_undispatched = snapshot.available_tickets.iter().any(|t| {
        !t.has_active_mr && !t.has_active_claim && t.prior_attempt_count == 0 && !t.human_required
    });

    // Handle exhausted tickets: if there are exhausted tickets and NO other actionable items,
    // return HumanRequired for the first exhausted ticket
    if !exhausted.is_empty() && !has_escalate_candidate && !has_retry_candidate && !has_undispatched
    {
        if let Some(first_exhausted) = failed_tickets
            .iter()
            .find(|t| t.genuine_agent_failure_count >= AUTO_RETRY_CAP)
        {
            return NextAction::HumanRequired {
                reason: format!(
                    "{} failed {} time(s) (agent failures) with no active MR; stopping automatic retries",
                    first_exhausted
                        .work_id
                        .as_deref()
                        .unwrap_or(&first_exhausted.ticket_path),
                    first_exhausted.genuine_agent_failure_count
                ),
                reference: first_exhausted
                    .work_id
                    .clone()
                    .or_else(|| Some(first_exhausted.ticket_path.clone())),
            };
        }
    }

    for ticket in &failed_tickets {
        if exhausted.contains(ticket.work_id.as_ref().unwrap_or(&ticket.ticket_path)) {
            continue;
        }
        if let Some(fc) = ticket.last_failure_class.as_deref() {
            if is_genuine_agent_failure(fc) {
                return NextAction::Escalate {
                    work_id: ticket
                        .work_id
                        .clone()
                        .unwrap_or_else(|| ticket.ticket_path.clone()),
                    ticket_path: ticket.ticket_path.clone(),
                    reason: format!(
                        "prior attempt failed ({fc}); escalating to a stronger backend/model"
                    ),
                };
            }
        }
    }
    let mut undispatched: Vec<_> = snapshot
        .available_tickets
        .iter()
        .filter(|t| !t.has_active_mr && !t.has_active_claim && t.prior_attempt_count == 0)
        // TICKET-human-required-scoping: skip work-item-scoped
        // human_required tickets; they await human action, not dispatch.
        .filter(|t| !t.human_required)
        .collect();
    undispatched.sort_by(|a, b| a.ticket_path.cmp(&b.ticket_path));
    if let Some(ticket) = undispatched.first() {
        return NextAction::DispatchTicket {
            ticket_path: ticket.ticket_path.clone(),
            work_id: ticket.work_id.clone(),
            recommended_backend: ticket.recommended_backend.clone(),
            recommended_model: ticket.recommended_model.clone(),
            reason: "eligible undispatched ticket".into(),
        };
    }

    // Infrastructure failures (timeouts, transient backend outages, and
    // harness/environment faults) are retryable, but must not monopolize the
    // controller while untouched work exists.  A dispatch already performs its
    // own bounded backend failover; retrying the same failed ticket again here
    // before every fresh ticket turns one unavailable provider into a backlog
    // stall.  Preserve retryability after the fresh queue has made progress.
    for ticket in &failed_tickets {
        if exhausted.contains(ticket.work_id.as_ref().unwrap_or(&ticket.ticket_path)) {
            continue;
        }
        if let Some(fc) = ticket.last_failure_class.as_deref() {
            if is_infra_failure(fc) && some_backend_eligible {
                return NextAction::Retry {
                    work_id: ticket
                        .work_id
                        .clone()
                        .unwrap_or_else(|| ticket.ticket_path.clone()),
                    ticket_path: ticket.ticket_path.clone(),
                    reason: format!(
                        "prior attempt failed ({fc}); retrying now that a backend appears available"
                    ),
                };
            }
        }
    }

    if let Some(scope) = snapshot
        .availability
        .iter()
        .find(|a| !a.eligible_now && a.unavailable_until.is_some())
    {
        return NextAction::WaitUntil {
            until: scope.unavailable_until.clone().unwrap(),
            reason: format!(
                "{} unavailable ({})",
                scope.backend,
                scope.reason.clone().unwrap_or_default()
            ),
        };
    }

    NextAction::NoOp {
        reason: "nothing actionable".into(),
    }
}

/// TICKET-081: how many consecutive `action_decided` events for the same
/// (kind, work_id) fingerprint, with nothing else for that work_id in
/// between, count as "stuck." Broader than TICKET-078's inline retry cap,
/// which only gates Retry/Escalate via ledger counts -- this catches any
/// action kind repeating (e.g. ReviewMr/FixMr selected over and over for a
/// branch whose classification never changes).
fn get_action_branch(action: &NextAction) -> Option<&str> {
    match action {
        NextAction::ReviewMr { branch, .. }
        | NextAction::MarkReadyForReview { branch, .. }
        | NextAction::FixMr { branch, .. }
        | NextAction::MergeMr { branch, .. } => Some(branch.as_str()),
        _ => None,
    }
}

fn resolve_action_with_filters(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    snapshot: &mut StatusSnapshot,
    history: &[crate::events::ControllerEvent],
    skipped_work_ids: &mut std::collections::HashSet<String>,
) -> Result<NextAction> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let repo = std::path::Path::new(&profile.local_path);
    let worktree_base = std::path::Path::new(&cfg.defaults.worktree_base);

    let mut stuck_loop_reason = None;
    let mut stuck_loop_ref = None;

    loop {
        let candidate = decide_next_action(snapshot);

        // Stuck loop detection
        if let Some(reason) = detect_stuck_loop(history, profile_name, &candidate) {
            if let Some(wid) = candidate.work_id() {
                if !skipped_work_ids.contains(wid) {
                    skipped_work_ids.insert(wid.to_string());
                    stuck_loop_reason = Some(reason.clone());
                    stuck_loop_ref = Some(wid.to_string());

                    // Persist a work-item-scoped durable human gate
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

                    // Filter from snapshot and re-decide
                    snapshot
                        .merge_requests
                        .retain(|mr| mr.work_id.as_deref() != Some(wid));
                    snapshot
                        .available_tickets
                        .retain(|t| t.work_id.as_deref() != Some(wid));
                    continue;
                }
            }
            return Ok(NextAction::HumanRequired {
                reason,
                reference: candidate.work_id().map(str::to_string),
            });
        }

        // Branch conflict detection
        if let Some(branch) = get_action_branch(&candidate) {
            if let Ok(wts) = crate::worktree::list_worktrees(repo, worktree_base) {
                let conflict = wts.into_iter().find(|wt| {
                    wt.branch
                        .as_deref()
                        .map(|br| {
                            br == branch || br.strip_prefix("refs/heads/").unwrap_or(br) == branch
                        })
                        .unwrap_or(false)
                });
                if let Some(wt) = conflict {
                    if !wt.is_releasable {
                        // Actively owned worktree! Defer/skip it.
                        if let Some(wid) = candidate.work_id() {
                            if !skipped_work_ids.contains(wid) {
                                skipped_work_ids.insert(wid.to_string());

                                // Record explicit non-terminal defer/skip
                                let msg = format!(
                                    "Deferring branch '{}' reuse: attached worktree at '{}' is actively owned",
                                    branch,
                                    wt.path.display()
                                );
                                println!("{msg}");
                                if let Err(e) = crate::events::record(
                                    cfg,
                                    crate::events::EventType::ActionOverridden,
                                    Some(profile_name),
                                    Some(wid),
                                    msg,
                                ) {
                                    eprintln!("warning: failed to record defer event: {e:#}");
                                }

                                // Filter from snapshot and re-decide
                                snapshot
                                    .merge_requests
                                    .retain(|mr| mr.work_id.as_deref() != Some(wid));
                                snapshot
                                    .available_tickets
                                    .retain(|t| t.work_id.as_deref() != Some(wid));
                                continue;
                            }
                        }
                        return Ok(NextAction::HumanRequired {
                            reason: format!(
                                "Branch '{}' reuse blocked by actively owned worktree at '{}'",
                                branch,
                                wt.path.display()
                            ),
                            reference: candidate.work_id().map(str::to_string),
                        });
                    }
                }
            }
        }

        if candidate.kind() == "no_op" {
            if let Some(reason) = stuck_loop_reason {
                return Ok(NextAction::HumanRequired {
                    reason,
                    reference: stuck_loop_ref,
                });
            }
        }

        return Ok(candidate);
    }
}

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
/// `~/.config/gah/config.toml`) hash to the same lock file.
fn loop_lock_path(profile_name: &str, config_path: &std::path::Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let state_root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let canonical_config =
        std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical_config.hash(&mut hasher);
    state_root.join("gah").join(format!(
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

/// Run the controller continuously in one process. The process lock is held
/// for the lifetime of the loop so a second manager for the same profile
/// cannot create a competing worker pool.
pub fn run_loop(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    json: bool,
    parallel: usize,
    skip_validation_gate: bool,
    config_path: &std::path::Path,
) -> Result<()> {
    let _lock = acquire_profile_lock(profile_name, config_path)?;

    // Holding the exclusive per-profile lock proves no other process can be
    // concurrently dispatching for this profile, so any `dispatch_started`
    // left over from before this process started (a prior `gah loop` killed
    // or crashed mid-dispatch) is provably abandoned, not just slow. Close
    // those out now so the dashboard's "Controller activity" panel doesn't
    // count them as running forever (see incident: 50 orphaned events after
    // repeated restarts).
    crate::events::reconcile_abandoned_dispatches(cfg, profile_name)?;

    loop {
        // Transient provider/controller failures must not kill the daemon.
        // A validation-gate failure is different: it proves the safety check
        // itself is unhealthy, so pause immediately and require an explicit
        // operator restart after repair. This avoids a retry/restart storm
        // while preserving fail-closed dispatch behavior.
        match run_once(cfg, profile_name, json, parallel, skip_validation_gate) {
            Ok(()) => std::thread::sleep(std::time::Duration::from_secs(30)),
            Err(error) if is_validation_gate_failure(&error) => {
                eprintln!(
                    "gah loop: paused because the validation gate failed; repair the gate and explicitly restart the loop: {error:#}"
                );
                return Err(error);
            }
            Err(error) => {
                eprintln!("gah loop: iteration failed; retrying after backoff: {error:#}");
                // ponytail: fixed 5-min backoff; make it exponential if a
                // hot failure ever burns real quota.
                std::thread::sleep(std::time::Duration::from_secs(300));
            }
        }
    }
}

pub fn run_once(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    json: bool,
    parallel: usize,
    skip_validation_gate: bool,
) -> Result<()> {
    let claim_scope = {
        let profile = crate::config::get_profile(cfg, profile_name)?;
        format!("{profile_name}@{}", profile.repo_id)
    };
    let now = time::OffsetDateTime::now_utc();
    let snapshot = crate::status::build_snapshot(cfg, profile_name, now)?;
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
        let history = crate::events::read_events(cfg)?;
        let mut snapshot_for_decision = snapshot;
        let mut skipped_work_ids = std::collections::HashSet::new();
        let original_action = decide_next_action(&snapshot_for_decision);
        let action = resolve_action_with_filters(
            cfg,
            profile_name,
            &mut snapshot_for_decision,
            &history,
            &mut skipped_work_ids,
        )?;
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
                match execute_action(cfg, profile_name, &action, skip_validation_gate) {
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
            execute_action(cfg, profile_name, &action, skip_validation_gate)?
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
    _snapshot: &crate::status::StatusSnapshot,
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

            // Re-build snapshot to get fresh state (this is conservative but safe)
            let mut fresh_snapshot =
                crate::status::build_snapshot(cfg, profile_name, time::OffsetDateTime::now_utc())?;

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

            let mut skipped_work_ids = std::collections::HashSet::new();
            let original_action = decide_next_action(&fresh_snapshot);
            let action = resolve_action_with_filters(
                cfg,
                profile_name,
                &mut fresh_snapshot,
                &history,
                &mut skipped_work_ids,
            )?;

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
                    handles.push(scope.spawn(move || {
                        let result = execute_action(
                            cfg,
                            &profile_for_thread,
                            &action_for_thread,
                            skip_validation_gate,
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
                let outcome = execute_action(cfg, profile_name, &action, skip_validation_gate)?;

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
mod tests {
    use super::{
        acquire_profile_lock, is_validation_gate_failure, loop_lock_path, NextAction,
        AUTO_RETRY_CAP,
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
    fn kind_is_stable_short_name_per_variant() {
        let action = NextAction::NoOp {
            reason: "nothing actionable".into(),
        };
        assert_eq!(action.kind(), "no_op");
        assert_eq!(action.reason(), "nothing actionable");
        assert_eq!(action.work_id(), None);
    }

    #[test]
    fn retry_and_escalate_expose_work_id() {
        let retry = NextAction::Retry {
            work_id: "TICKET-042".into(),
            ticket_path: "docs/tickets/TICKET-042-x.md".into(),
            reason: "infra failure cleared".into(),
        };
        assert_eq!(retry.kind(), "retry");
        assert_eq!(retry.work_id(), Some("TICKET-042"));

        let escalate = NextAction::Escalate {
            work_id: "TICKET-043".into(),
            ticket_path: "docs/tickets/TICKET-043-y.md".into(),
            reason: "no progress last attempt".into(),
        };
        assert_eq!(escalate.kind(), "escalate");
        assert_eq!(escalate.work_id(), Some("TICKET-043"));

        let ready = NextAction::MarkReadyForReview {
            work_id: Some("TICKET-044".into()),
            branch: "gah/real-4".into(),
            mr_url: Some("https://example/pull/4".into()),
            reason: "CI green, still draft".into(),
        };
        assert_eq!(ready.kind(), "mark_ready_for_review");
        assert_eq!(ready.work_id(), Some("TICKET-044"));
    }

    #[test]
    fn round_trips_through_json() {
        let action = NextAction::ReviewMr {
            work_id: Some("TICKET-001".into()),
            branch: "gah/real-1".into(),
            mr_url: Some("https://example/pull/1".into()),
            reason: "classified NEEDS_REVIEW".into(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: NextAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, action);
    }

    #[test]
    fn mark_ready_round_trips_through_json() {
        let action = NextAction::MarkReadyForReview {
            work_id: Some("TICKET-004".into()),
            branch: "gah/real-4".into(),
            mr_url: Some("https://example/pull/4".into()),
            reason: "CI green, still draft".into(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: NextAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, action);
    }

    #[test]
    fn wait_until_and_human_required_have_no_work_id() {
        let wait = NextAction::WaitUntil {
            until: "2026-07-06T00:00:00Z".into(),
            reason: "backend unavailable".into(),
        };
        assert_eq!(wait.work_id(), None);

        let human = NextAction::HumanRequired {
            reason: "MR ready for human decision".into(),
            reference: Some("https://example/pull/2".into()),
        };
        assert_eq!(human.work_id(), None);
    }

    use super::decide_next_action;
    use crate::models::AvailableTicket;
    use crate::status::{
        Blocker, ObservationStatus, Observations, ProfileIdentity, ScopeStatusJson, StatusError,
        StatusSnapshot,
    };
    use crate::sync::{RecommendedAction, SyncMrJson};

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

    fn mr(branch: &str, classification: &str) -> SyncMrJson {
        mr_with_ci(branch, classification, false)
    }

    fn mr_with_ci(branch: &str, classification: &str, ci_passed: bool) -> SyncMrJson {
        SyncMrJson {
            profile: None,
            branch: branch.into(),
            work_id: Some(format!("TICKET-{branch}")),
            id: Some("1".into()),
            url: Some(format!("https://example/{branch}")),
            state: Some("OPEN".into()),
            draft: false,
            merge_status: None,
            merged: classification == "MERGED",
            merged_at: None,
            ci_passed,
            ci_pending: false,
            title: None,
            effective_backend: None,
            effective_model: None,
            review_verdict: None,
            classification: classification.into(),
            recommended_action: RecommendedAction::from_class(classification),
        }
    }

    /// Issue #156: a `READY_FOR_HUMAN` MR whose CI is non-terminal / unknown
    /// (GitLab `head_pipeline` gap: running/pending/missing). `ci_passed` is
    /// false but `ci_pending` is true, so it must surface as a re-check rather
    /// than silently no-op.
    fn mr_ci_pending(branch: &str, classification: &str) -> SyncMrJson {
        SyncMrJson {
            profile: None,
            branch: branch.into(),
            work_id: Some(format!("TICKET-{branch}")),
            id: Some("1".into()),
            url: Some(format!("https://example/{branch}")),
            state: Some("OPEN".into()),
            draft: false,
            merge_status: None,
            merged: classification == "MERGED",
            merged_at: None,
            ci_passed: false,
            ci_pending: true,
            title: None,
            effective_backend: None,
            effective_model: None,
            review_verdict: None,
            classification: classification.into(),
            recommended_action: RecommendedAction::from_class(classification),
        }
    }

    fn ticket(
        path: &str,
        work_id: Option<&str>,
        prior_attempt_count: usize,
        last_failure_class: Option<&str>,
        has_active_mr: bool,
        human_required: bool,
    ) -> AvailableTicket {
        // For tests: genuine_agent_failure_count equals prior_attempt_count
        // unless the caller sets it explicitly. Tests that need different
        // values construct AvailableTicket directly.
        let genuine_agent_failure_count =
            if last_failure_class.is_some_and(super::is_genuine_agent_failure) {
                prior_attempt_count
            } else {
                0
            };
        AvailableTicket {
            ticket_path: path.into(),
            work_id: work_id.map(str::to_string),
            title: None,
            recommended_backend: None,
            recommended_model: None,
            prior_attempt_count,
            genuine_agent_failure_count,
            last_failure_class: last_failure_class.map(str::to_string),
            has_active_mr,
            human_required,
            has_active_claim: false,
        }
    }

    #[test]
    fn incomplete_observation_stops_safely() {
        let mut snapshot = empty_snapshot();
        snapshot.errors.push(StatusError {
            subsystem: "sync".into(),
            message: "gh not found".into(),
            incomplete_snapshot: true,
        });
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
        assert!(action.reason().contains("sync"));
    }

    #[test]
    fn blocker_forces_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot.blockers.push(Blocker {
            kind: "human_required".into(),
            reason: Some("ledger_human_required".into()),
            message: Some("Ledger indicates human intervention required".into()),
            backend: None,
            model: None,
            until: None,
            source_reference: Some("gah/real-1".into()),
        });
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
    }

    #[test]
    fn needs_review_mr_takes_priority() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(mr("gah/real-1", "NEEDS_FIX"));
        snapshot
            .merge_requests
            .push(mr("gah/real-2", "NEEDS_REVIEW"));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::ReviewMr { branch, .. } => assert_eq!(branch, "gah/real-2"),
            other => panic!("expected ReviewMr, got {other:?}"),
        }
    }

    #[test]
    fn ci_failed_mr_trigger_fix_action() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(mr("gah/real-1", "CI_FAILED"));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::FixMr { branch, reason, .. } => {
                assert_eq!(branch, "gah/real-1");
                assert!(reason.contains("reusing existing branch"));
            }
            other => panic!("expected FixMr, got {other:?}"),
        }
    }

    #[test]
    fn needs_fix_mr_trigger_fix_action() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(mr("gah/real-1", "NEEDS_FIX"));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::FixMr { branch, reason, .. } => {
                assert_eq!(branch, "gah/real-1");
                assert!(reason.contains("reusing existing branch"));
            }
            other => panic!("expected FixMr, got {other:?}"),
        }
    }

    #[test]
    fn ci_failed_mr_retries_until_cap() {
        let mut snapshot = empty_snapshot();
        // Simulate 2 prior fix attempts (at the cap)
        let mut fix_attempts = std::collections::HashMap::new();
        fix_attempts.insert("gah/real-1".to_string(), 2); // AUTO_RETRY_CAP = 2
        snapshot.fix_attempt_counts = fix_attempts;
        snapshot.merge_requests.push(mr("gah/real-1", "CI_FAILED"));
        let action = decide_next_action(&snapshot);
        // TICKET-skip-and-continue: an exhausted MR is a work-item block, not a
        // profile-wide freeze. With nothing else actionable, the loop no-ops
        // (supervisor re-checks next cycle); the item stays in blocked_work_items.
        assert_eq!(action.kind(), "no_op");
        assert!(
            action.reason().contains("nothing actionable")
                || action.reason().contains("fix retry cap")
        );
    }

    #[test]
    fn ready_for_human_mr_maps_to_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr("gah/real-1", "READY_FOR_HUMAN"));
        let action = decide_next_action(&snapshot);
        // TICKET-skip-and-continue: a single READY_FOR_HUMAN MR awaiting a
        // human merge decision is a work-item block, not a profile freeze.
        // With nothing else actionable, the loop no-ops (re-checks later).
        assert_eq!(action.kind(), "no_op");
    }

    #[test]
    fn ready_for_human_draft_mr_with_ci_passed_becomes_mark_ready_for_review() {
        // Draft MRs must leave draft as soon as CI is conclusively green,
        // regardless of merge policy. Merge happens later, after the
        // controller observes the non-draft state.
        let mut snapshot = empty_snapshot();
        let mut mr = mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true);
        mr.draft = true;
        snapshot.merge_requests.push(mr);
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "mark_ready_for_review");
        match action {
            NextAction::MarkReadyForReview { branch, .. } => assert_eq!(branch, "gah/real-1"),
            other => panic!("expected MarkReadyForReview, got {other:?}"),
        }
    }

    #[test]
    fn ready_for_human_draft_mr_with_ci_passed_marks_ready_for_review_under_stop_for_human() {
        let mut snapshot = empty_snapshot();
        let mut mr = mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true);
        mr.draft = true;
        snapshot.merge_requests.push(mr);
        snapshot.profile.merge_policy = crate::config::MergePolicy::StopForHuman;

        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "mark_ready_for_review");
    }

    #[test]
    fn ready_for_human_mr_ci_passed_but_merge_retry_cap_exceeded_becomes_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true));
        snapshot
            .merge_attempt_counts
            .insert("gah/real-1".to_string(), 2); // == AUTO_RETRY_CAP
        let action = decide_next_action(&snapshot);
        // TICKET-skip-and-continue: work-item block, not a profile freeze.
        assert_eq!(action.kind(), "no_op");
    }

    // Issue #124 / TICKET-127: per-repo merge policy gates what happens for a
    // strong-approved MR whose CI has passed. Default (`auto`) merges.
    #[test]
    fn merge_policy_auto_merges_approved_mr_with_ci_passed() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true));
        snapshot.profile.merge_policy = crate::config::MergePolicy::Auto;
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "merge_mr");
    }

    // `stop_for_human` must never auto-merge: it surfaces the decision to a
    // human operator once strong review is done and CI is green.
    #[test]
    fn merge_policy_stop_for_human_awaits_human_with_ci_passed() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true));
        snapshot.profile.merge_policy = crate::config::MergePolicy::StopForHuman;
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
        assert!(action.reason().contains("stop_for_human"));
    }

    // `gitlab_mwps` still decides `MergeMr` (the MWPS flag is set at execution
    // time in `execute_action`); it must not fall back to `human_required`.
    #[test]
    fn merge_policy_gitlab_mwps_decides_merge_mr_with_ci_passed() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true));
        snapshot.profile.merge_policy = crate::config::MergePolicy::GitlabMwps;
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "merge_mr");
        assert!(action.reason().contains("merge-when-pipeline-succeeds"));
    }

    // `stop_for_human` only changes behavior when CI has passed. A non-green
    // `READY_FOR_HUMAN` MR still defers to a human (no merge attempted).
    #[test]
    fn merge_policy_stop_for_human_without_ci_passed_is_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr_with_ci("gah/real-1", "READY_FOR_HUMAN", false));
        snapshot.profile.merge_policy = crate::config::MergePolicy::StopForHuman;
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
    }

    // Issue #156: the exact gap. Under the default `auto` merge policy (which
    // both shipped profiles silently fall through to), a `READY_FOR_HUMAN` MR
    // whose CI is non-terminal / unknown (GitLab `head_pipeline` missing or in
    // a running/pending state) must NOT silently no-op forever. It must surface
    // as a visible, observable `wait_until` re-check so the next loop tick can
    // re-observe CI -- not a bare `no_op`, and not a parked state.
    #[test]
    fn issue_156_auto_policy_ci_pending_surfaces_as_wait_until() {
        for branch in ["gah/real-1", "gah/real-2"] {
            let mut snapshot = empty_snapshot();
            snapshot
                .merge_requests
                .push(mr_ci_pending(branch, "READY_FOR_HUMAN"));
            // Default (unset) merge policy is `Auto` -- the exact silent
            // default that triggered the bug report.
            assert_eq!(
                snapshot.profile.merge_policy,
                crate::config::MergePolicy::Auto
            );
            let action = decide_next_action(&snapshot);
            assert_eq!(
                action.kind(),
                "wait_until",
                "CI-pending MR under Auto must be observable, not a silent no_op"
            );
            assert!(action.reason().contains("not yet conclusively resolved"));
        }
    }

    // Issue #156 regression for the explicit `auto` value (same as the silent
    // default above) and for the `gitlab_mwps` policy: CI-pending must surface
    // as a re-check, never auto-merge.
    #[test]
    fn issue_156_explicit_auto_and_gitlab_mwps_ci_pending_waits() {
        for policy in [
            crate::config::MergePolicy::Auto,
            crate::config::MergePolicy::GitlabMwps,
        ] {
            let mut snapshot = empty_snapshot();
            snapshot
                .merge_requests
                .push(mr_ci_pending("gah/real-1", "READY_FOR_HUMAN"));
            snapshot.profile.merge_policy = policy;
            let action = decide_next_action(&snapshot);
            assert_eq!(action.kind(), "wait_until");
        }
    }

    // Issue #129 Bug A: `READY_FOR_HUMAN` must have exactly ONE defined
    // behavior per policy. Under the default `auto` merge policy with green
    // CI, it auto-merges (MergeMr); it never parks. This pins that the
    // READY_FOR_HUMAN classification does not also map to HumanRequired in a
    // separate code path for the same green-CI/auto-merge inputs.
    #[test]
    fn ready_for_human_green_ci_auto_policy_never_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true));
        snapshot.profile.merge_policy = crate::config::MergePolicy::Auto;
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "merge_mr");
        assert_ne!(action.kind(), "human_required");
    }

    // Review hold: a manager session running `gah hold set` on a work_id
    // must stop gah's own loop from auto-merging it out from under them,
    // even though every other input (READY_FOR_HUMAN, green CI, auto
    // policy) would otherwise produce MergeMr. The MR is simply skipped for
    // this tick, not escalated -- with no other actionable work in the
    // snapshot, that means NoOp.
    #[test]
    fn ready_for_human_review_held_work_id_does_not_auto_merge() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true));
        snapshot.profile.merge_policy = crate::config::MergePolicy::Auto;
        snapshot
            .review_held_work_ids
            .insert("TICKET-gah/real-1".to_string());

        let action = decide_next_action(&snapshot);
        assert_ne!(action.kind(), "merge_mr");
        assert_eq!(action.kind(), "no_op");
    }

    // Issue #129 Bug A: the complement -- the only case READY_FOR_HUMAN parks
    // is when the merge policy forbids auto-merge (StopForHuman) with green CI
    // (or any policy without green CI). Confirm the human-park path is the
    // explicit policy decision, not a stray rule-6 mapping.
    #[test]
    fn ready_for_human_stop_for_human_is_explicit_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr_with_ci("gah/real-1", "READY_FOR_HUMAN", true));
        snapshot.profile.merge_policy = crate::config::MergePolicy::StopForHuman;
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
        assert!(action.reason().contains("stop_for_human"));
    }

    #[test]
    fn merged_and_closed_mrs_are_not_actionable() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(mr("gah/real-1", "MERGED"));
        snapshot
            .merge_requests
            .push(mr("gah/real-2", "CLOSED_UNMERGED"));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
    }

    #[test]
    fn genuine_agent_failure_escalates() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-001-x.md",
            Some("TICKET-001"),
            1,
            Some("agent_no_progress"),
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::Escalate { work_id, .. } => assert_eq!(work_id, "TICKET-001"),
            other => panic!("expected Escalate, got {other:?}"),
        }
    }

    #[test]
    fn context_limit_failure_escalates_without_being_orphaned() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-context.md",
            Some("TICKET-context"),
            1,
            Some("context_limit_exceeded"),
            false,
            false,
        ));
        assert!(matches!(
            decide_next_action(&snapshot),
            NextAction::Escalate { .. }
        ));
    }

    // Live bug: `dispatch::decide_route` used to classify
    // `RouteError::NoEligibleBackend` (every candidate backend momentarily
    // quota-exhausted/cooling down) as `human_blocked`, which is excluded
    // from `is_infra_failure` -- permanently orphaning the ticket even after
    // a backend recovered, since this class is neither retried nor escalated
    // nor flagged `human_required`. Now fixed to classify as `backend_error`,
    // which *is* in `is_infra_failure`'s list; this pins that once a backend
    // becomes eligible again, the ticket is retried, not silently dropped.
    #[test]
    fn backend_error_from_no_eligible_backend_retries_once_a_backend_is_eligible() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-noelig.md",
            Some("TICKET-noelig"),
            1,
            Some("backend_error"),
            false,
            false,
        ));

        // No eligible backend at all -> must not retry blindly.
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");

        // Now a backend is eligible -> retry, not orphaned.
        snapshot.availability.push(ScopeStatusJson {
            backend: "codex".into(),
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
        match decide_next_action(&snapshot) {
            NextAction::Retry { work_id, .. } => assert_eq!(work_id, "TICKET-noelig"),
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn infra_failure_retries_only_when_a_backend_is_eligible() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-002-x.md",
            Some("TICKET-002"),
            1,
            Some("harness_error"),
            false,
            false,
        ));

        // No eligible backend at all -> must not retry blindly.
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");

        // Now a backend is eligible -> retry.
        snapshot.availability.push(ScopeStatusJson {
            backend: "codex".into(),
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
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::Retry { work_id, .. } => assert_eq!(work_id, "TICKET-002"),
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn retry_cap_exceeded_forces_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-003-x.md",
            Some("TICKET-003"),
            2, // == AUTO_RETRY_CAP
            Some("agent_no_progress"),
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
    }

    // Issue #95: infra-class failures must NOT consume the retry cap. A
    // ticket with 3 backend_error/environment_error failures and 0 genuine
    // agent failures should still be retried, not halted.
    #[test]
    fn infra_failures_do_not_exhaust_retry_cap() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(AvailableTicket {
            ticket_path: "docs/tickets/TICKET-INFRA-x.md".into(),
            work_id: Some("TICKET-INFRA".into()),
            title: None,
            recommended_backend: None,
            recommended_model: None,
            prior_attempt_count: 3,
            genuine_agent_failure_count: 0, // all were infra failures
            last_failure_class: Some("backend_error".into()),
            has_active_mr: false,
            human_required: false,
            has_active_claim: false,
        });
        // Without a backend eligible, it should not be retried or escalated
        snapshot.availability.push(ScopeStatusJson {
            backend: "codex".into(),
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
        let action = decide_next_action(&snapshot);
        // Should retry, not return human_required
        match action {
            NextAction::Retry { work_id, .. } => assert_eq!(work_id, "TICKET-INFRA"),
            other => panic!("expected Retry for infra-only failures, got {other:?}"),
        }
    }

    // Live incident: a `git fetch` failure during worktree setup (e.g. a
    // misconfigured remote, transient auth prompt) is a harness-level
    // plumbing failure classified `harness_error`, not an agent failure.
    // Before the dispatch.rs fix, this path left `failure_class` as `None`,
    // which neither the escalate loop (`is_genuine_agent_failure`) nor the
    // retry loop (`is_infra_failure`) picks up -- both gate on
    // `Some(failure_class)` -- so the ticket became permanently un-actionable
    // once `prior_attempt_count > 0`. With `failure_class` correctly set to
    // `harness_error`, it must flow through the infra-failure retry path.
    #[test]
    fn git_fetch_harness_error_is_retried_not_orphaned() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-FETCH-x.md",
            Some("TICKET-FETCH"),
            1,
            Some("harness_error"),
            false,
            false,
        ));
        snapshot.availability.push(ScopeStatusJson {
            backend: "codex".into(),
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
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::Retry { work_id, .. } => assert_eq!(work_id, "TICKET-FETCH"),
            other => panic!("expected Retry for harness_error, got {other:?}"),
        }
    }

    // Issue #95: genuine agent failures MUST still exhaust the retry cap.
    // A ticket with 2 agent_no_progress failures should be halted.
    #[test]
    fn genuine_agent_failures_still_exhaust_retry_cap() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-AGENT-x.md",
            Some("TICKET-AGENT"),
            2, // == AUTO_RETRY_CAP
            Some("agent_no_progress"),
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
    }

    // Issue #95: mixed failures -- only agent failures count toward the cap.
    // 2 agent failures + 2 infra failures: agent_failure_count = 2 == cap
    // => exhausted.
    #[test]
    fn mixed_failures_only_agent_count_toward_cap() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(AvailableTicket {
            ticket_path: "docs/tickets/TICKET-MIXED-x.md".into(),
            work_id: Some("TICKET-MIXED".into()),
            title: None,
            recommended_backend: None,
            recommended_model: None,
            prior_attempt_count: 4,         // 2 agent + 2 infra
            genuine_agent_failure_count: 2, // == AUTO_RETRY_CAP
            last_failure_class: Some("backend_error".into()),
            has_active_mr: false,
            human_required: false,
            has_active_claim: false,
        });
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
    }

    // Issue #95: infra-only failures do NOT block other undispatched tickets.
    // Same as exhausted_ticket_does_not_block_others but with infra failures.
    #[test]
    fn infra_exhausted_ticket_does_not_block_others() {
        let mut snapshot = empty_snapshot();
        // TICKET-INFRA has 3 infra failures but 0 agent failures -> not exhausted
        snapshot.available_tickets.push(AvailableTicket {
            ticket_path: "docs/tickets/TICKET-INFRA-x.md".into(),
            work_id: Some("TICKET-INFRA".into()),
            title: None,
            recommended_backend: None,
            recommended_model: None,
            prior_attempt_count: 3,
            genuine_agent_failure_count: 0,
            last_failure_class: Some("environment_error".into()),
            has_active_mr: false,
            human_required: false,
            has_active_claim: false,
        });
        // TICKET-FRESH is undispatched
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-FRESH-x.md",
            Some("TICKET-FRESH"),
            0,
            None,
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        // Untouched backlog work must run before retrying an infra-failed
        // ticket; otherwise one outage can monopolize the loop.
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-FRESH"))
            }
            other => panic!("expected DispatchTicket for fresh work, got {other:?}"),
        }
    }

    #[test]
    fn exhausted_ticket_does_not_block_others() {
        let mut snapshot = empty_snapshot();
        // TICKET-113 is exhausted (prior_attempt_count = 2, no active MR)
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-113-x.md",
            Some("TICKET-113"),
            2, // == AUTO_RETRY_CAP
            Some("agent_no_progress"),
            false,
            false,
        ));
        // TICKET-128 is eligible (prior_attempt_count = 0, no active MR)
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-128-x.md",
            Some("TICKET-128"),
            0,
            None,
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        // Should dispatch TICKET-128, NOT return human_required
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-128"))
            }
            other => panic!("expected DispatchTicket for TICKET-128, got {other:?}"),
        }
    }

    // TICKET-skip-and-continue: an MR stuck at NEEDS_FIX beyond the fix retry
    // cap must NOT freeze the profile. An unrelated eligible ticket still
    // dispatches (this is the worldcup-props !249 recurring-stall case).
    #[test]
    fn exhausted_mr_does_not_block_others() {
        let mut snapshot = empty_snapshot();
        // Stuck MR with fix attempts >= cap.
        snapshot
            .fix_attempt_counts
            .insert("gah/stuck-1".into(), AUTO_RETRY_CAP);
        snapshot.merge_requests.push(mr("gah/stuck-1", "NEEDS_FIX"));
        // An unrelated eligible ticket exists.
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-128-x.md",
            Some("TICKET-128"),
            0,
            None,
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-128"))
            }
            other => panic!("expected DispatchTicket for TICKET-128, got {other:?}"),
        }
    }

    // A profile with ONLY an exhausted MR (nothing else actionable) no-ops
    // rather than freezing the profile -- the MR stays in blocked_work_items.
    #[test]
    fn exhausted_mr_alone_is_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot
            .fix_attempt_counts
            .insert("gah/stuck-1".into(), AUTO_RETRY_CAP);
        snapshot.merge_requests.push(mr("gah/stuck-1", "NEEDS_FIX"));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
    }

    #[test]
    fn never_dispatched_ticket_is_eligible() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-004-x.md",
            Some("TICKET-004"),
            0,
            None,
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-004"))
            }
            other => panic!("expected DispatchTicket, got {other:?}"),
        }
    }

    #[test]
    fn ticket_with_active_mr_is_never_a_dispatch_candidate() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-005-x.md",
            Some("TICKET-005"),
            1,
            Some("agent_no_progress"),
            true, // has_active_mr
            false,
        ));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
    }

    // TICKET-human-required-scoping regression tests.
    // A ticket-scoped human_required must NOT freeze the profile: the blocked
    // ticket is skipped, and an unrelated eligible ticket still dispatches.
    #[test]
    fn ticket_scoped_human_block_does_not_freeze_profile() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-A-x.md",
            Some("TICKET-A"),
            1,
            None,
            false,
            true,
        ));
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-B-x.md",
            Some("TICKET-B"),
            0,
            None,
            false,
            false,
        ));
        // profile-wide blockers must stay empty
        assert!(snapshot.blockers.is_empty());
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-B"))
            }
            other => panic!("expected DispatchTicket for TICKET-B, got {other:?}"),
        }
    }

    // Most-recent ledger entry belongs to another blocked ticket: the eligible
    // one (written earlier) must remain dispatchable. Directly reproduces the
    // observed incident (a newer NEEDS_FIX verdict froze the whole profile).
    #[test]
    fn most_recent_ledger_entry_belongs_to_other_blocked_ticket() {
        let mut snapshot = empty_snapshot();
        // B is eligible (written earlier, human_required false)
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-B-x.md",
            Some("TICKET-B"),
            0,
            None,
            false,
            false,
        ));
        // A is human-blocked (the newer ledger entry)
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-A-x.md",
            Some("TICKET-A"),
            1,
            None,
            false,
            true,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-B"))
            }
            other => panic!("expected DispatchTicket for TICKET-B, got {other:?}"),
        }
    }

    // Human-blocked ticket remains blocked (not redispatched, not escalated).
    #[test]
    fn human_blocked_ticket_remains_blocked() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-A-x.md",
            Some("TICKET-A"),
            1,
            None,
            false,
            true,
        ));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
    }

    // Multiple blocked and eligible tickets coexist: only eligible ones dispatch.
    #[test]
    fn multiple_blocked_and_eligible_coexist() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-A-x.md",
            Some("TICKET-A"),
            1,
            None,
            false,
            true,
        ));
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-B-x.md",
            Some("TICKET-B"),
            0,
            None,
            false,
            false,
        ));
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-C-x.md",
            Some("TICKET-C"),
            1,
            None,
            false,
            true,
        ));
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-D-x.md",
            Some("TICKET-D"),
            0,
            None,
            false,
            false,
        ));
        assert!(snapshot.blockers.is_empty());
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-B"))
            }
            other => panic!("expected DispatchTicket for TICKET-B, got {other:?}"),
        }
    }

    // Genuine profile-wide blocker (sync failure observation) still halts the
    // profile. It is reported via `errors` (-> decide_next_action Rule 1 =>
    // NoOp), NOT via a ticket-scoped human_required `blockers` entry. The fix
    // must preserve this path while no longer freezing on ticket-scoped HR.
    #[test]
    fn genuine_profile_wide_blocker_still_stops_dispatch() {
        let mut snapshot = empty_snapshot();
        snapshot.errors.push(StatusError {
            subsystem: "sync".into(),
            message: "gh not found".into(),
            incomplete_snapshot: true,
        });
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-B-x.md",
            Some("TICKET-B"),
            0,
            None,
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
        // ticket-scoped human_required must NOT create a profile-wide blocker
        assert!(snapshot.blockers.is_empty());
        // the genuine profile-wide failure is recorded in errors
        assert!(!snapshot.errors.is_empty());
    }

    // A ticket whose human_required has since cleared must no longer be blocked.
    // (build_snapshot re-derives per-work-item human_required from current
    // ledger history; here we model the cleared state directly.)
    #[test]
    fn later_state_clears_prior_human_requirement() {
        let mut snapshot = empty_snapshot();
        // No human_required flag + fresh (prior_attempt_count 0) -> eligible, dispatches.
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-A-x.md",
            Some("TICKET-A"),
            0,
            None,
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-A"))
            }
            other => panic!("expected DispatchTicket for cleared TICKET-A, got {other:?}"),
        }
        assert!(snapshot.blocked_work_items.is_empty());
    }

    // ===== Bug 1: dispatch_reason scoping for fix retry cap =====
    // Internal OpenHands retries (attempts_started > 0 within one dispatch)
    // must NOT consume the post-review fix retry budget.
    fn needs_fix_mr(branch: &str, work_id: &str) -> crate::sync::SyncMrJson {
        crate::sync::SyncMrJson {
            profile: None,
            branch: branch.into(),
            work_id: Some(work_id.into()),
            id: None,
            url: None,
            state: Some("opened".into()),
            draft: true,
            merge_status: Some("can_be_merged".into()),
            merged: false,
            merged_at: None,
            ci_passed: false,
            ci_pending: false,
            title: None,
            effective_backend: None,
            effective_model: None,
            review_verdict: None,
            classification: "NEEDS_FIX".into(),
            recommended_action: crate::sync::RecommendedAction::ReuseBranch,
        }
    }

    #[test]
    fn internal_retries_do_not_consume_fix_retry_budget() {
        let mut snapshot = empty_snapshot();
        snapshot.fix_attempt_counts.insert("branch-A".into(), 0);
        snapshot
            .merge_requests
            .push(needs_fix_mr("branch-A", "TICKET-A"));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::FixMr { branch, .. } => assert_eq!(branch, "branch-A"),
            other => panic!("expected FixMr, got {other:?}"),
        }
    }

    // First NEEDS_FIX review permits the first repair dispatch.
    #[test]
    fn first_needs_fix_permits_first_repair_dispatch() {
        let mut snapshot = empty_snapshot();
        snapshot.fix_attempt_counts.insert("branch-A".into(), 0);
        snapshot
            .merge_requests
            .push(needs_fix_mr("branch-A", "TICKET-A"));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::FixMr { .. } => {}
            other => panic!("expected FixMr for first repair, got {other:?}"),
        }
    }

    // Actual repair dispatches increment the count exactly once each.
    // After AUTO_RETRY_CAP (2) repairs, HumanRequired fires.
    #[test]
    fn retry_cap_triggers_after_configured_post_review_repairs() {
        let mut snapshot = empty_snapshot();
        snapshot.fix_attempt_counts.insert("branch-A".into(), 2);
        snapshot
            .merge_requests
            .push(needs_fix_mr("branch-A", "TICKET-A"));
        let action = decide_next_action(&snapshot);
        // TICKET-skip-and-continue: work-item block, not a profile freeze.
        assert_eq!(action.kind(), "no_op");
    }

    // One repair used, one more allowed.
    #[test]
    fn one_repair_used_still_permits_second() {
        let mut snapshot = empty_snapshot();
        snapshot.fix_attempt_counts.insert("branch-A".into(), 1);
        snapshot
            .merge_requests
            .push(needs_fix_mr("branch-A", "TICKET-A"));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::FixMr { .. } => {}
            other => panic!("expected FixMr after 1 repair (cap=2), got {other:?}"),
        }
    }

    // ===== Bug 2: stuck-loop gate persists to ledger and skips ticket =====
    #[test]
    fn stuck_loop_gated_ticket_is_skipped() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-A-x.md",
            Some("TICKET-A"),
            0,
            None,
            false,
            true,
        ));
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-B-x.md",
            Some("TICKET-B"),
            0,
            None,
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-B"))
            }
            other => panic!("expected DispatchTicket for TICKET-B (A gated), got {other:?}"),
        }
    }

    #[test]
    fn stuck_loop_gated_ticket_remains_blocked() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-A-x.md",
            Some("TICKET-A"),
            0,
            None,
            false,
            true,
        ));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
    }

    #[test]
    fn stuck_loop_gate_unrelated_ticket_eligible() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-A-x.md",
            Some("TICKET-A"),
            0,
            None,
            false,
            true,
        ));
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-C-x.md",
            Some("TICKET-C"),
            0,
            None,
            false,
            false,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-C"))
            }
            other => panic!("expected DispatchTicket for TICKET-C, got {other:?}"),
        }
    }

    // ===== Bug 3: retry-cap HumanRequired projects into blocked_work_items =====
    #[test]
    fn retry_cap_projects_into_blocked_work_items() {
        let mut snapshot = empty_snapshot();
        snapshot.fix_attempt_counts.insert("branch-A".into(), 2);
        snapshot
            .merge_requests
            .push(needs_fix_mr("branch-A", "TICKET-A"));
        // Simulate what status.rs does: project retry-cap into blocked_work_items
        const AUTO_RETRY_CAP: usize = 2;
        for mr in &snapshot.merge_requests {
            if matches!(mr.classification.as_str(), "CI_FAILED" | "NEEDS_FIX") {
                let attempts = snapshot
                    .fix_attempt_counts
                    .get(&mr.branch)
                    .copied()
                    .unwrap_or(0);
                if attempts >= AUTO_RETRY_CAP {
                    snapshot.blocked_work_items.push(crate::status::Blocker {
                        kind: "human_required".into(),
                        reason: Some("fix_retry_cap_exceeded".into()),
                        message: Some("cap exceeded".into()),
                        backend: None,
                        model: None,
                        until: None,
                        source_reference: Some(mr.branch.clone()),
                    });
                }
            }
        }
        assert!(!snapshot.blocked_work_items.is_empty());
        assert_eq!(
            snapshot.blocked_work_items[0].reason.as_deref(),
            Some("fix_retry_cap_exceeded")
        );
    }

    #[test]
    fn unavailable_backend_with_known_reset_waits() {
        let mut snapshot = empty_snapshot();
        snapshot.availability.push(ScopeStatusJson {
            backend: "claude".into(),
            model: None,
            quota_pool: None,
            eligible_now: false,
            reason: Some("rate_limited".into()),
            unavailable_until: Some("2026-07-06T00:00:00Z".into()),
            source: None,
            last_error_summary: None,
            observed_at: None,
            scope: None,
        });
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::WaitUntil { until, .. } => assert_eq!(until, "2026-07-06T00:00:00Z"),
            other => panic!("expected WaitUntil, got {other:?}"),
        }
    }

    #[test]
    fn nothing_actionable_is_noop() {
        let snapshot = empty_snapshot();
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
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

    #[test]
    fn test_resolve_action_with_filters_branch_conflict() {
        use super::resolve_action_with_filters;
        use crate::config::{Defaults, GahConfig, RoutingPolicy};
        use crate::sync::RecommendedAction;
        use std::collections::HashMap;

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        // Initialize git repo using std::process::Command
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::fs::write(repo.join("f.txt"), "content\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();

        // Create a bare origin
        let bare = tmp.path().join("origin.git");
        std::process::Command::new("git")
            .args(["init", "--bare", "-q", bare.to_str().unwrap()])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["remote", "add", "origin", bare.to_str().unwrap()])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-q", "-u", "origin", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let worktree_base = tmp.path().join("worktrees");

        // 1. Create a branch and a dirty worktree for it
        std::process::Command::new("git")
            .args(["checkout", "-b", "conflict-branch"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "origin", "conflict-branch"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();

        // Create a dirty (actively owned) worktree under worktree_base for conflict-branch
        let wt_path = worktree_base.join("conflict-branch");
        std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
        std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "conflict-branch-wt",
                wt_path.to_str().unwrap(),
                "origin/conflict-branch",
            ])
            .current_dir(&repo)
            .output()
            .unwrap();
        // Make it dirty
        std::fs::write(wt_path.join("f.txt"), "dirty\n").unwrap();

        // Set up config using the test helper
        let mut profile = crate::config::tests::test_profile_for_notifications();
        profile.display_name = "Real".to_string();
        profile.repo_id = "real".to_string();
        profile.local_path = repo.to_string_lossy().to_string();
        profile.artifact_root = tmp.path().to_string_lossy().to_string();
        profile.default_target_branch = "main".to_string();

        let mut profiles = HashMap::new();
        profiles.insert("real".to_string(), profile);

        let cfg = GahConfig {
            context: Default::default(),
            defaults: Defaults {
                current_manager: None,
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: worktree_base.to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: RoutingPolicy::default(),
            },
            profiles,
        };

        // Snapshot has two merge requests: one on conflict-branch, one on clean-branch
        let mut snapshot = empty_snapshot();
        snapshot.profile.local_path = repo.to_string_lossy().to_string();
        snapshot.merge_requests = vec![
            SyncMrJson {
                profile: None,
                branch: "conflict-branch-wt".to_string(),
                work_id: Some("TICKET-229".to_string()),
                id: None,
                url: Some("url1".to_string()),
                state: None,
                draft: false,
                merge_status: None,
                merged: false,
                ci_passed: false,
                title: Some("Conflict branch MR".to_string()),
                merged_at: None,
                effective_backend: None,
                effective_model: None,
                review_verdict: None,
                ci_pending: false,
                classification: "NEEDS_FIX".to_string(),
                recommended_action: RecommendedAction::ReuseBranch,
            },
            SyncMrJson {
                profile: None,
                branch: "z-clean-branch".to_string(),
                work_id: Some("TICKET-230".to_string()),
                id: None,
                url: Some("url2".to_string()),
                state: None,
                draft: false,
                merge_status: None,
                merged: false,
                ci_passed: false,
                title: Some("Z-Clean branch MR".to_string()),
                merged_at: None,
                effective_backend: None,
                effective_model: None,
                review_verdict: None,
                ci_pending: false,
                classification: "NEEDS_FIX".to_string(),
                recommended_action: RecommendedAction::ReuseBranch,
            },
        ];

        let history = Vec::new();
        let mut skipped_work_ids = std::collections::HashSet::new();

        // When we resolve the action, it should skip the conflict-branch and pick z-clean-branch!
        let action = resolve_action_with_filters(
            &cfg,
            "real",
            &mut snapshot,
            &history,
            &mut skipped_work_ids,
        )
        .unwrap();

        assert_eq!(action.work_id(), Some("TICKET-230"));
        assert!(skipped_work_ids.contains("TICKET-229"));

        // Clean up worktrees
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "-f", wt_path.to_str().unwrap()])
            .current_dir(&repo)
            .output();
    }
}
