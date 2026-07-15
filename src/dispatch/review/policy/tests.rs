use super::*;
use crate::config::{CandidateConfig, Profile, RoutingPolicy};
use crate::dispatch::publish::review_labels;
use crate::dispatch::test_util::{gah_config, gah_config_with_ledger, profile};
use crate::ledger::LedgerEntry;

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
    entry.review_source_sha = Some("reviewed-sha".to_string());
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
fn clear_attempts_resets_review_cycle_budget_for_the_current_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            max_review_cycles_per_ticket: Some(2),
            ..RoutingPolicy::default()
        },
    );
    let prof = profile(tmp.path());
    for _ in 0..2 {
        let mut entry = review_ledger_entry("test", &prof, "gah/42", "NEEDS_FIX", "high");
        entry.work_id = Some("#42".into());
        crate::ledger::append(&cfg, &entry).unwrap();
    }
    crate::ledger::append(&cfg, &LedgerEntry::new_clear_attempts("test", &prof, "#42")).unwrap();

    let block = check_review_budget(
        &cfg,
        &prof,
        "test",
        Some("#42"),
        &route_decision("vibe", Some("reviewer"), false),
    )
    .unwrap();

    assert!(block.is_none(), "pre-tombstone reviews must be cleared");
}

#[test]
fn reviews_after_clear_attempts_still_consume_the_cycle_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            max_review_cycles_per_ticket: Some(1),
            ..RoutingPolicy::default()
        },
    );
    let prof = profile(tmp.path());
    let mut old = review_ledger_entry("test", &prof, "gah/42", "NEEDS_FIX", "high");
    old.work_id = Some("#42".into());
    crate::ledger::append(&cfg, &old).unwrap();
    crate::ledger::append(&cfg, &LedgerEntry::new_clear_attempts("test", &prof, "#42")).unwrap();
    let mut current = review_ledger_entry("test", &prof, "gah/42", "NEEDS_FIX", "high");
    current.work_id = Some("#42".into());
    crate::ledger::append(&cfg, &current).unwrap();

    let block = check_review_budget(
        &cfg,
        &prof,
        "test",
        Some("#42"),
        &route_decision("vibe", Some("reviewer"), false),
    )
    .unwrap()
    .expect("the post-tombstone review must consume the one-cycle budget");

    assert!(block.reason.contains("1/1 review cycles"));
}

#[test]
fn another_profiles_clear_attempts_does_not_reset_review_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            max_review_cycles_per_ticket: Some(1),
            ..RoutingPolicy::default()
        },
    );
    let prof = profile(tmp.path());
    let mut review = review_ledger_entry("test", &prof, "gah/42", "NEEDS_FIX", "high");
    review.work_id = Some("#42".into());
    crate::ledger::append(&cfg, &review).unwrap();
    crate::ledger::append(
        &cfg,
        &LedgerEntry::new_clear_attempts("other-profile", &prof, "#42"),
    )
    .unwrap();

    let block = check_review_budget(
        &cfg,
        &prof,
        "test",
        Some("#42"),
        &route_decision("vibe", Some("reviewer"), false),
    )
    .unwrap()
    .expect("another profile's tombstone must not reset this budget");

    assert!(block.reason.contains("1/1 review cycles"));
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
fn capacity_deferrals_do_not_consume_the_review_cycle_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            max_review_cycles_per_ticket: Some(1),
            ..RoutingPolicy::default()
        },
    );
    let prof = profile(tmp.path());
    let mut deferred = review_ledger_entry("test", &prof, "gah/44", "deferred_capacity", "high");
    deferred.work_id = Some("#44".into());
    deferred.attempts_started = Some(0);
    deferred.attempts_completed = Some(0);
    crate::ledger::append(&cfg, &deferred).unwrap();

    let block = check_review_budget(
        &cfg,
        &prof,
        "test",
        Some("#44"),
        &route_decision("claude", Some("sonnet"), false),
    )
    .unwrap();

    assert!(block.is_none(), "a deferred route launched no reviewer");
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
fn sha_less_legacy_reviews_are_retried_before_escalation() {
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
    for verdict in ["NEEDS_FIX", "REJECT"] {
        let mut legacy = review_ledger_entry("test", &prof, "gah/branch-1", verdict, "high");
        legacy.review_source_sha = None;
        legacy.effective_backend = "claude".into();
        legacy.effective_model = Some("sonnet".into());
        crate::ledger::append(&cfg, &legacy).unwrap();
    }

    assert_eq!(
        review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
        None
    );
    let next = next_escalatory_reviewer(&cfg, &prof, "test", "gah/branch-1", None).unwrap();
    assert_eq!(next.backend, "claude");
    assert_eq!(next.model.as_deref(), Some("sonnet"));
}

#[test]
fn cancelled_shutdown_is_not_a_low_confidence_verdict_or_spent_reviewer() {
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
    let mut cancelled =
        review_ledger_entry("test", &prof, "gah/branch-1", "cancelled_shutdown", "low");
    cancelled.effective_backend = "claude".into();
    cancelled.effective_model = Some("sonnet".into());
    cancelled.human_required = true;
    crate::ledger::append(&cfg, &cancelled).unwrap();

    assert_eq!(
        review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
        None
    );
    let next = next_escalatory_reviewer(&cfg, &prof, "test", "gah/branch-1", None).unwrap();
    assert_eq!(next.backend, "claude");
    assert_eq!(next.model.as_deref(), Some("sonnet"));
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

    let via_agy_second = route_decision("agy-second", Some("Claude Sonnet 4.6 (Thinking)"), true);
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
