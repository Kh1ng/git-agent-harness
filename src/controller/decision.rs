//! TICKET-078: pure, deterministic decision policy. Consumes an
//! already-observed `StatusSnapshot` and returns exactly one `NextAction`.
//! No LLM, no I/O, no provider/backend/systemd execution -- see
//! `controller::runtime` for everything that actually executes an action.

use super::NextAction;
use crate::status::StatusSnapshot;
use std::collections::HashSet;
use std::time::Duration;

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

/// Issue #156: produce an RFC3339 timestamp `offset` from "now" for a
/// `WaitUntil` re-check. Used when a READY_FOR_HUMAN MR's CI is pending so the
/// controller records a visible, observable deferral instead of a silent no-op.
fn now_plus(offset: Duration) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        + offset;
    let secs = now.as_secs();
    let dt =
        chrono::DateTime::from_timestamp(secs as i64, 0).unwrap_or(chrono::DateTime::UNIX_EPOCH);
    format!("{}", dt.format("%Y-%m-%dT%H:%M:%SZ"))
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
    // A final review handoff is recorded against a work item in the ledger.
    // The provider label deliberately still classifies the MR as NEEDS_REVIEW
    // so provisional escalations can continue, but a *final* ledger handoff
    // must not cause the loop to re-run the same review every tick.
    let final_review_holds: HashSet<&str> = snapshot
        .blocked_work_items
        .iter()
        .filter(|blocker| blocker.kind == "human_required")
        .filter_map(|blocker| blocker.source_reference.as_deref())
        .collect();
    // Issue #156: READY_FOR_HUMAN MRs whose CI is non-terminal / unknown
    // (GitLab head_pipeline gap). They wait for CI to resolve and are not
    // silently dropped.
    let mut wait_and_recheck_mrs: Vec<&crate::sync::SyncMrJson> = Vec::new();

    for mr in &mrs {
        match mr.classification.as_str() {
            "NEEDS_REVIEW" => {
                if mr
                    .work_id
                    .as_deref()
                    .is_some_and(|work_id| final_review_holds.contains(work_id))
                {
                    human_blocked_mrs.push(mr);
                } else {
                    review_candidates.push(mr);
                }
            }
            "CI_FAILED" | "NEEDS_FIX" => {
                let fix_attempts = snapshot
                    .fix_attempt_counts
                    .get(&mr.branch)
                    .copied()
                    .unwrap_or(0);
                if fix_attempts >= snapshot.profile.max_fix_attempts_per_mr as usize {
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
    let implementation_failure_cap = snapshot.profile.max_implementation_failures_per_ticket;
    let exhausted: HashSet<_> = failed_tickets
        .iter()
        .filter(|t| t.genuine_agent_failure_count >= implementation_failure_cap as usize)
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
            .find(|t| t.genuine_agent_failure_count >= implementation_failure_cap as usize)
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

#[cfg(test)]
#[path = "decision/tests.rs"]
mod tests;
