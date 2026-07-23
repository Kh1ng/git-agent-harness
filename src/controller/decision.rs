//! TICKET-078: pure, deterministic decision policy. Consumes an
//! already-observed `StatusSnapshot` and returns exactly one `NextAction`.
//! No LLM, no I/O, no provider/backend/systemd execution -- see
//! `controller::runtime` for everything that actually executes an action.

use super::{HumanRequiredReason, NextAction};
use crate::status::StatusSnapshot;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::time::Duration;

/// TICKET-078: how many times the controller will automatically
/// Retry/Escalate the same work_id before giving up and requiring a human.
/// Deliberately small and inline (not configurable) -- this is a safety
/// floor, not a policy knob; see TICKET-081 for the broader stuck-loop
/// detector this complements.
pub(crate) const AUTO_RETRY_CAP: usize = 2;

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

fn ticket_consumes_worker_slot(ticket: &crate::models::AvailableTicket) -> bool {
    ticket.execution_policy.dispatchable_now
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

fn issue_path_identity_key(path: &str) -> IssuePathIdentity {
    let trimmed = path.trim();
    if let Some(stripped) = trimmed.strip_prefix('#') {
        if stripped.bytes().all(|b| b.is_ascii_digit()) {
            return IssuePathIdentity::Numeric(stripped.parse().unwrap_or(u64::MAX));
        }
    }
    if trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return IssuePathIdentity::Numeric(trimmed.parse().unwrap_or(u64::MAX));
    }
    IssuePathIdentity::Lexical(trimmed.to_string())
}

#[derive(Eq, PartialEq)]
enum IssuePathIdentity {
    Numeric(u64),
    Lexical(String),
}

impl Ord for IssuePathIdentity {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(left), Self::Numeric(right)) => left.cmp(right),
            (Self::Lexical(left), Self::Lexical(right)) => left.cmp(right),
            (Self::Numeric(_), Self::Lexical(_)) => Ordering::Less,
            (Self::Lexical(_), Self::Numeric(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for IssuePathIdentity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn ticket_order_key(ticket: &crate::models::AvailableTicket) -> (u8, IssuePathIdentity) {
    (
        ticket.priority.to_sort_rank(),
        issue_path_identity_key(&ticket.ticket_path),
    )
}

fn ticket_order(
    a: &crate::models::AvailableTicket,
    b: &crate::models::AvailableTicket,
) -> Ordering {
    ticket_order_key(a).cmp(&ticket_order_key(b))
}

/// TICKET-078: pure, deterministic, no LLM, no I/O -- consumes an
/// already-built `StatusSnapshot` and returns exactly one action. First
/// matching rule wins:
///
/// 1. incomplete critical observation -> stop safely (NoOp)
/// 2. a recorded blocker (today: ledger human_required) -> HumanRequired
/// 3. an MR classified READY_FOR_HUMAN with draft=true and conclusively-green
///    CI -> MarkReadyForReview
/// 4. an MR classified READY_FOR_HUMAN with draft=false and conclusively-green
///    CI -> MergeMr (or HumanRequired if merge policy forbids auto-merge)
/// 5. an MR classified NEEDS_REVIEW -> ReviewMr
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
/// 11. a retryable infrastructure-failed ticket is capacity-blocked until a
///     known backend reset -> WaitUntil
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
            work_id: None,
            reason: blocker
                .message
                .clone()
                .unwrap_or_else(|| blocker.kind.clone()),
            reference: blocker.source_reference.clone(),
            reason_code: Some(HumanRequiredReason::ConfigurationInfra.as_str().to_string()),
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
    // Track human-blocked MRs along with their specific reason code
    let mut human_blocked_mrs: Vec<(&crate::sync::SyncMrJson, HumanRequiredReason)> = Vec::new();
    // Issue #156: READY_FOR_HUMAN MRs whose CI is non-terminal / unknown
    // (GitLab head_pipeline gap). They wait for CI to resolve and are not
    // silently dropped.
    let mut wait_and_recheck_mrs: Vec<&crate::sync::SyncMrJson> = Vec::new();

    for mr in &mrs {
        // A durable work-item hold applies to every lifecycle state for the
        // MR, not only NEEDS_REVIEW. In particular, a stuck-loop gate written
        // while an MR is NEEDS_FIX must prevent the same repair from being
        // selected and re-gated on every recurring tick. Match both canonical
        // work identity and branch because retry-cap projections are
        // branch-scoped while ledger gates are work-id-scoped.
        if let Some(blocker) = snapshot.blocked_work_items.iter().find(|blocker| {
            blocker.kind == "human_required"
                && blocker
                    .source_reference
                    .as_deref()
                    .is_some_and(|reference| {
                        mr.work_id.as_deref() == Some(reference) || mr.branch == reference
                    })
        }) {
            let mut reason_code = blocker
                .reason_code
                .as_deref()
                .or(blocker.reason.as_deref())
                .map(HumanRequiredReason::from_code)
                .unwrap_or(HumanRequiredReason::Unknown);
            // Historical review holds predate stable reason codes. Preserve
            // their established review-evidence classification instead of
            // degrading those records to unknown while applying the broader
            // all-MR-state hold behavior above.
            if reason_code == HumanRequiredReason::Unknown && mr.classification == "NEEDS_REVIEW" {
                reason_code = HumanRequiredReason::ReviewEvidenceGate;
            }
            human_blocked_mrs.push((mr, reason_code));
            continue;
        }

        match mr.classification.as_str() {
            "NEEDS_REVIEW" => review_candidates.push(mr),
            "CI_FAILED" | "NEEDS_FIX" => {
                let fix_attempts = snapshot
                    .fix_attempt_counts
                    .get(&mr.branch)
                    .copied()
                    .unwrap_or(0);
                if fix_attempts >= snapshot.profile.max_fix_attempts_per_mr as usize {
                    // Exhausted fix attempts -> work-item block, not a profile freeze.
                    human_blocked_mrs.push((mr, HumanRequiredReason::FixRetryCapExceeded));
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
                            human_blocked_mrs
                                .push((mr, HumanRequiredReason::MergeRetryCapExceeded));
                        }
                    }
                } else if merge_policy == crate::config::MergePolicy::StopForHuman {
                    // TICKET-127: under stop_for_human, a READY_FOR_HUMAN
                    // MR without CI passed still defers to the human
                    // immediately — no CI gate needed.
                    human_blocked_mrs.push((mr, HumanRequiredReason::MergePolicy));
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
                    human_blocked_mrs.push((mr, HumanRequiredReason::MergePolicy));
                }
            }
            _ => {}
        }
    }

    // Drain cheap terminal lifecycle actions before starting another costly
    // review. Otherwise a large NEEDS_REVIEW queue can starve a strong-approved
    // green MR forever. Reviews still stay ahead of repair work once ready and
    // merge candidates are drained.
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
                work_id: mr.work_id.clone(),
                reason: format!(
                    "MR on branch '{}' strong-reviewed with CI passing; merge policy is 'stop_for_human' -- awaiting human merge",
                    mr.branch
                ),
                reference: mr.url.clone(),
                reason_code: Some(HumanRequiredReason::MergePolicy.as_str().to_string()),
            };
        }
        // TICKET-128: a restricted profile (allow_pull_request_creation
        // == false) must never enter the auto-merge path. The reviewer
        // verdict and CI status remain authoritative; the work simply
        // stays at a human handoff instead of auto-merging. This is an
        // independent axis from reviewer routing and merge policy.
        if !snapshot.publishing_allow_pr {
            return NextAction::HumanRequired {
                work_id: mr.work_id.clone(),
                reason: format!(
                    "MR on branch '{}' approved with CI passing, but profile publishing policy forbids PR/MR creation (human handoff)",
                    mr.branch
                ),
                reference: mr.url.clone(),
                reason_code: Some(HumanRequiredReason::PublishingRestriction.as_str().to_string()),
            };
        }
        return NextAction::MergeMr {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            review_generation: mr.review_generation.clone(),
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
    if let Some(mr) = review_candidates.first() {
        return NextAction::ReviewMr {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            reason: format!("MR on branch '{}' classified NEEDS_REVIEW", mr.branch),
        };
    }
    if let Some(mr) = fix_candidates.first() {
        return NextAction::FixMr {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            review_generation: mr.review_generation.clone(),
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
        let (mr, reason_code) = human_blocked_mrs[0];
        return NextAction::HumanRequired {
            work_id: mr.work_id.clone(),
            reason: format!(
                "MR on branch '{}' classified {} (human decision required)",
                mr.branch, mr.classification
            ),
            reference: mr.url.clone(),
            reason_code: Some(reason_code.as_str().to_string()),
        };
    }

    if let Some(parent) = snapshot
        .pm_parent_states
        .iter()
        .find(|parent| parent.completed && !parent.reconciled)
    {
        return NextAction::ReconcilePmParent {
            work_id: parent.work_id.clone(),
            source_issue_number: parent.source_issue_number.clone(),
            plan_fingerprint: parent.plan_fingerprint.clone(),
            child_issue_numbers: parent.child_issue_numbers.clone(),
            reason: format!(
                "all {} provider-native PM child issue(s) are terminal",
                parent.child_issue_numbers.len()
            ),
        };
    }

    // Native issue discovery/dependency resolution is independent from MR
    // sync. A provider failure must fail closed for new ticket dispatch while
    // still allowing all review, merge, and repair actions above to proceed.
    if let Some(error) = snapshot
        .errors
        .iter()
        .find(|error| error.subsystem == "issue_intake")
    {
        return NextAction::NoOp {
            reason: format!("ticket intake incomplete: {}", error.message),
        };
    }

    let published_pm_work_ids = snapshot
        .pm_parent_states
        .iter()
        .map(|parent| parent.work_id.as_str())
        .collect::<HashSet<_>>();
    let active_claim_work_ids = snapshot
        .active_claims
        .iter()
        .map(|claim| claim.work_id.as_str())
        .collect::<HashSet<_>>();
    let mut planning_candidates = snapshot
        .issue_intake_rejections
        .iter()
        .filter(|issue| issue.reason_code == "planning")
        .filter_map(|issue| {
            let work_id = issue.work_id.as_deref()?;
            (!published_pm_work_ids.contains(work_id)
                && !active_claim_work_ids.contains(work_id)
                && snapshot
                    .pm_decomposition_attempt_counts
                    .get(work_id)
                    .copied()
                    .unwrap_or_default()
                    < snapshot.pm_max_attempts as usize)
                .then_some(issue)
        })
        .collect::<Vec<_>>();
    planning_candidates.sort_by(|a, b| a.ticket_path.cmp(&b.ticket_path));
    if let Some(issue) = planning_candidates.first() {
        return NextAction::DecomposeIssue {
            ticket_path: issue.ticket_path.clone(),
            work_id: issue
                .work_id
                .clone()
                .unwrap_or_else(|| issue.ticket_path.clone()),
            title: issue.title.clone(),
            reason: format!(
                "trusted provider issue carries configured PM decomposition label ({})",
                issue.labels.join(", ")
            ),
        };
    }

    // Priority is not backpressure. Lifecycle candidates above always drain,
    // but a temporarily unavailable reviewer may be removed by capacity
    // filtering for this tick. Preserve the pre-filter status projection and
    // refuse every action that could publish another managed MR until the
    // profile falls below its configured limit.
    if snapshot.implementation_intake_paused {
        return NextAction::NoOp {
            reason: format!(
                "implementation intake paused: {} open managed MR(s) + {} in-flight implementation(s) reached limit {}; draining review/fix/merge work",
                snapshot.open_managed_mr_count,
                snapshot.inflight_implementation_count,
                snapshot.profile.max_open_managed_mrs
            ),
        };
    }

    let some_backend_eligible = snapshot.availability.iter().any(|a| a.eligible_now);
    let mut failed_tickets: Vec<_> = snapshot
        .available_tickets
        .iter()
        .filter(|t| {
            ticket_consumes_worker_slot(t)
                && !t.has_active_mr
                && !t.has_active_claim
                && t.prior_attempt_count > 0
        })
        // TICKET-human-required-scoping: a work-item-scoped human_required
        // ticket is blocked at the item level. Skip it so it is neither
        // retried, escalated, nor redispatched -- but unrelated eligible
        // tickets keep flowing.
        .filter(|t| !t.human_required)
        .collect();
    failed_tickets.sort_by(|a, b| ticket_order(a, b));

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
        ticket_consumes_worker_slot(t)
            && !t.has_active_mr
            && !t.has_active_claim
            && t.prior_attempt_count == 0
            && !t.human_required
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
                work_id: first_exhausted.work_id.clone(),
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
                reason_code: Some(HumanRequiredReason::RetryBudgetExhausted.as_str().to_string()),
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
        .filter(|t| {
            ticket_consumes_worker_slot(t)
                && !t.has_active_mr
                && !t.has_active_claim
                && t.prior_attempt_count == 0
        })
        // TICKET-human-required-scoping: skip work-item-scoped
        // human_required tickets; they await human action, not dispatch.
        .filter(|t| !t.human_required)
        .collect();
    undispatched.sort_by(|a, b| ticket_order(a, b));
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

    // Availability records are global historical state, not evidence that an
    // idle profile currently needs that backend. Only turn a cooldown into a
    // controller wait when real retryable work is blocked on capacity. Without
    // this guard, an empty queue appears stalled until the first unrelated
    // quota reset (production incident #466).
    let has_capacity_blocked_retry = failed_tickets.iter().any(|ticket| {
        !exhausted.contains(ticket.work_id.as_ref().unwrap_or(&ticket.ticket_path))
            && ticket
                .last_failure_class
                .as_deref()
                .is_some_and(is_infra_failure)
    });
    if has_capacity_blocked_retry {
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
    }

    if let Some(issue) = snapshot.issue_intake_rejections.iter().find(|issue| {
        issue.reason_code == "planning"
            && issue.work_id.as_ref().is_some_and(|work_id| {
                !published_pm_work_ids.contains(work_id.as_str())
                    && snapshot
                        .pm_decomposition_attempt_counts
                        .get(work_id)
                        .copied()
                        .unwrap_or_default()
                        >= snapshot.pm_max_attempts as usize
            })
    }) {
        return NextAction::HumanRequired {
            work_id: issue.work_id.clone(),
            reason: format!(
                "{} exhausted {} bounded PM decomposition attempt(s)",
                issue.work_id.as_deref().unwrap_or(&issue.ticket_path),
                snapshot.pm_max_attempts
            ),
            reference: issue.work_id.clone(),
            reason_code: Some(
                HumanRequiredReason::RetryBudgetExhausted
                    .as_str()
                    .to_string(),
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
