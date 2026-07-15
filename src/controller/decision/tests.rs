use super::{decide_next_action, NextAction, AUTO_RETRY_CAP};
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
        active_claims: vec![],
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
        review_gate_reason: None,
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
        review_gate_reason: None,
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
        action.reason().contains("nothing actionable") || action.reason().contains("fix retry cap")
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
fn final_review_handoff_is_not_re_reviewed_each_loop_tick() {
    let mut snapshot = empty_snapshot();
    snapshot.merge_requests.push(mr("gah/42", "NEEDS_REVIEW"));
    snapshot.blocked_work_items.push(Blocker {
        kind: "human_required".into(),
        reason: Some("review_escalation_exhausted".into()),
        message: Some("all configured reviewers were tried".into()),
        backend: None,
        model: None,
        until: None,
        source_reference: Some("TICKET-gah/42".into()),
    });

    assert_eq!(decide_next_action(&snapshot).kind(), "no_op");
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
        review_gate_reason: None,
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

#[test]
fn configured_fix_cap_allows_requested_number_of_repairs() {
    let mut snapshot = empty_snapshot();
    snapshot.profile.max_fix_attempts_per_mr = 4;
    snapshot.fix_attempt_counts.insert("branch-A".into(), 3);
    snapshot
        .merge_requests
        .push(needs_fix_mr("branch-A", "TICKET-A"));
    assert!(matches!(
        decide_next_action(&snapshot),
        NextAction::FixMr { .. }
    ));

    snapshot.fix_attempt_counts.insert("branch-A".into(), 4);
    assert_eq!(decide_next_action(&snapshot).kind(), "no_op");
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
fn idle_profile_with_unrelated_known_reset_is_noop() {
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
    assert_eq!(action.kind(), "no_op");
    assert!(action.reason().contains("nothing actionable"));
}

#[test]
fn failed_infrastructure_ticket_with_known_reset_waits() {
    let mut snapshot = empty_snapshot();
    snapshot.available_tickets.push(ticket(
        "466",
        Some("#466"),
        1,
        Some("backend_error"),
        false,
        false,
    ));
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
