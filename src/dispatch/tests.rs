use super::preflight;
use super::publish::{build_metadata_rich_mr_body, build_mr_title, build_standard_mr_body};
use super::run_auto_fix_commands;
use super::self_check_validation_gate;
use super::ValidationGateError;
use super::{
    apply_authoritative_work_identity, apply_diff_stats, apply_pm_plan, apply_route_to_ledger,
    attempt_usage, build_experiment_mr_body, build_fix_or_improve_mr_body, build_pm_plan_task,
    check_review_budget, classify_git_operation_result, classify_validation_failure_progress,
    classify_worktree_result, collect_pm_preflight, collect_ticket_summaries, decide_route,
    derive_reviewer_tier, first_markdown_heading, mark_backend_unavailable_from_output_at,
    nearest_existing_ancestor, next_escalatory_reviewer, next_ticket_id, parse_pm_plan,
    parse_review_verdict, parse_review_verdict_with_context, render_review_comment,
    review_escalation_reason, review_labels, review_preflight, review_usage, reviewer_dedup_class,
    routing_runtime_state, run_backend, scan_available_tickets, should_skip_per_dispatch_baseline,
    validation_failure_fingerprint, validation_failure_no_progress_reason,
    ExperimentMrRenderContext, MrRenderContext, ReviewDiffBundle, ReviewGateContext, ReviewerTier,
    RouteDecision, TicketMetadata, UsageAttribution, ValidationFailureProgress,
};
use crate::availability::{availability_for, load_state, Reason};
use crate::config::{CandidateConfig, Defaults, GahConfig, Profile, RoutingPolicy};
use crate::ledger::LedgerEntry;
use crate::models::PmPlan;
use crate::routing::{CandidateIdentity, RouteError, RouteRequest};
use crate::test_support::PathGuard;
use std::fs;
use std::path::Path;
use std::process::Command;
use time::OffsetDateTime;

const CODEX_FULL_RESET: &str =
    include_str!("../../tests/fixtures/quota-logs/codex_usage_exhausted_full_reset.txt");
const OPENCODE_HY3_RATE_LIMIT: &str =
    include_str!("../../tests/fixtures/quota-logs/opencode_hy3_rate_limit.log");

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

    let usage = attempt_usage(
        path.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("vibe"), None),
        None,
        None,
    );
    assert_eq!(usage.input_tokens, Some(500));
    assert_eq!(usage.output_tokens, Some(120));
    assert_eq!(usage.total_tokens, Some(620));
}

#[test]
fn attempt_usage_attributes_missing_artifact_without_fabricating_tokens() {
    let usage = attempt_usage(
        "/definitely/does/not/exist/backend-output.log",
        None,
        UsageAttribution::backend(Some("codex"), Some("gpt-5.4-mini")),
        None,
        None,
    );
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
    assert_eq!(usage.provider.as_deref(), Some("openai"));
    assert_eq!(usage.usage_classification.as_deref(), Some("quota_backed"));
    assert!(usage.actual_model_unknown_reason.is_some());
}

#[test]
fn attempt_usage_is_empty_when_log_has_no_usage_info() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("backend-output.log");
    fs::write(&path, "agent made some edits, no usage reported\n").unwrap();

    let usage = attempt_usage(
        path.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("vibe"), None),
        None,
        None,
    );
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
        UsageAttribution::backend(Some("agy"), Some("Gemini 3.5 Flash (Medium)")),
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
        UsageAttribution::backend(Some("agy"), Some("Claude Sonnet 4.6 (Thinking)")),
        None,
    );

    assert_eq!(usage.usage_source.as_deref(), Some("execution_observed"));
    assert_eq!(usage.usage_classification.as_deref(), Some("quota_backed"));
    assert_eq!(usage.backend_instance.as_deref(), Some("agy"));
    assert_eq!(usage.provider.as_deref(), Some("anthropic"));
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
        UsageAttribution::backend(Some("codex"), None),
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

    let mut implementation = LedgerEntry::new("test", &prof, "codex", "improve", "x", None, None);
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
fn harness_idle_watchdog_marks_backend_outage_not_rate_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("availability.json");
    let parsed = mark_backend_unavailable_from_output_at(
            &state,
            "vibe",
            Some("mistral-medium-3.5"),
            Some("vibe-monthly"),
            "GAH: killed after 600s with no new backend output or worktree progress (stalled, not just slow).",
            "/tmp/vibe.log",
        )
        .unwrap()
        .unwrap();

    assert_eq!(
        parsed.kind,
        crate::quota_parser::FailureKind::BackendStalled
    );
    let decision = availability_for(
        &state,
        "vibe",
        Some("mistral-medium-3.5"),
        Some("vibe-monthly"),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(!decision.eligible);
    assert_eq!(decision.reason, Some(Reason::BackendOutage));
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
    let progress = classify_validation_failure_progress(Some("same failure"), None, "same failure");
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
    let progress =
        classify_validation_failure_progress(None, Some(TICKET_154_ATTEMPT_1), CARGO_TEST_FAILURE);
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
