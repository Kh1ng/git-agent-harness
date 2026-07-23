use super::{
    route_state_fingerprint, suppress_recent_capacity_deferrals, update_parallel_refill_budget,
};
use crate::controller::recovery::retain_snapshot_candidates;
use crate::models::AvailableTicket;
use std::collections::HashSet;

fn controller_config() -> crate::config::GahConfig {
    toml::from_str(
        r#"
[profiles.real]
display_name = "Real"
repo_id = "real"
provider = "github"
repo = "owner/real"
local_path = "/tmp/real"
artifact_root = "/tmp/real-artifacts"
default_target_branch = "main"
"#,
    )
    .unwrap()
}

fn empty_snapshot() -> crate::status::StatusSnapshot {
    crate::status::StatusSnapshot {
        schema_version: 1,
        review_contract_version: crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION,
        generated_at: "2026-07-17T00:00:00Z".into(),
        profile: crate::status::ProfileIdentity {
            profile: "real".into(),
            display_name: "Real".into(),
            repo_id: "real".into(),
            provider: "github".into(),
            local_path: "/tmp/real".into(),
            default_target_branch: "main".into(),
            merge_policy: crate::config::MergePolicy::default(),
            max_fix_attempts_per_mr: 2,
            max_implementation_failures_per_ticket: 8,
            max_open_managed_mrs: 1,
            issue_intake_policy: crate::models::IssueIntakePolicy {
                mode: "canonical_autonomous_only".into(),
                canonical_autonomous_label: "exec:autonomous".into(),
                trusted_human_authors: vec![],
                trusted_bot_authors: vec![],
                github_issue_author_allowlist: vec![],
            },
        },
        observations: crate::status::Observations {
            sync: crate::status::ObservationStatus { status: "ok" },
            availability: crate::status::ObservationStatus { status: "ok" },
            ledger: crate::status::ObservationStatus { status: "ok" },
        },
        merge_requests: vec![],
        availability: vec![],
        recent_ledger: None,
        constraints: vec![],
        blockers: vec![],
        blocked_work_items: vec![],
        issue_intake_rejections: vec![],
        dependency_blockers: vec![],
        errors: vec![],
        available_tickets: vec![],
        active_claims: vec![],
        pm_parent_states: vec![],
        pm_decomposition_attempt_counts: std::collections::HashMap::new(),
        pm_max_attempts: 2,
        fix_attempt_counts: std::collections::HashMap::new(),
        merge_attempt_counts: std::collections::HashMap::new(),
        review_held_work_ids: std::collections::HashSet::new(),
        publishing_allow_pr: true,
        generated_artifact_deny_patterns: vec![],
        max_parallel_workers: 1,
        open_managed_mr_count: 0,
        inflight_implementation_count: 0,
        implementation_intake_paused: false,
        backend_configured: std::collections::HashMap::new(),
        backend_instances: vec![],
    }
}

fn available_ticket(work_id: &str) -> AvailableTicket {
    AvailableTicket {
        ticket_path: work_id.trim_start_matches('#').into(),
        work_id: Some(work_id.into()),
        normalized_work_identity: crate::work_claim::normalize_work_identity(work_id),
        source: crate::models::CandidateSource::LegacyTicket,
        execution_policy: crate::models::CandidateExecutionPolicy {
            intake_mode: "canonical_autonomous_only".into(),
            explicit_autonomy_required: true,
            autonomous_metadata_present: true,
            dispatchable_now: true,
            exclusion_reason_code: None,
            exclusion_reason: None,
        },
        title: Some(format!("Work {work_id}")),
        has_active_mr: false,
        priority: crate::models::TicketPriority::Unspecified,
        prior_attempt_count: 0,
        genuine_agent_failure_count: 0,
        last_failure_class: None,
        recommended_backend: None,
        recommended_model: None,
        human_required: false,
        human_required_reason_code: None,
        has_active_claim: false,
    }
}

fn needs_fix_mr(work_id: &str) -> crate::sync::SyncMrJson {
    crate::sync::SyncMrJson {
        profile: None,
        branch: format!("gah/{work_id}"),
        work_id: Some(work_id.into()),
        id: Some(work_id.trim_start_matches('#').into()),
        url: Some(format!("https://gitlab.example/mr/{work_id}")),
        state: Some("opened".into()),
        draft: true,
        merge_status: Some("can_be_merged".into()),
        merged: false,
        merged_at: None,
        ci_passed: false,
        ci_pending: false,
        title: Some(format!("Repair {work_id}")),
        effective_backend: None,
        effective_model: None,
        review_verdict: Some("NEEDS_FIX".into()),
        review_gate_reason: None,
        source_sha: None,
        review_contract_version: crate::ledger::REVIEW_CONTRACT_VERSION,
        review_generation: None,
        review_generation_status: None,
        classification: "NEEDS_FIX".into(),
        recommended_action: crate::sync::RecommendedAction::ReuseBranch,
    }
}

fn needs_review_mr(work_id: &str) -> crate::sync::SyncMrJson {
    let mut mr = needs_fix_mr(work_id);
    mr.review_verdict = None;
    mr.classification = "NEEDS_REVIEW".into();
    mr.recommended_action = crate::sync::RecommendedAction::RunReview;
    mr
}

fn capacity_event(work_id: &str, details: &str) -> crate::events::ControllerEvent {
    crate::events::ControllerEvent {
        timestamp: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap(),
        event_type: "dispatch_finished".into(),
        profile: Some("real".into()),
        work_id: Some(work_id.into()),
        run_id: Some(format!("capacity-{work_id}")),
        reason_code: None,
        review_contract_version: Some(crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION),
        details: details.into(),
    }
}

#[test]
fn capacity_deferred_github_ticket_does_not_starve_another_ticket() {
    let cfg = controller_config();
    let mut snapshot = empty_snapshot();
    snapshot.available_tickets = vec![available_ticket("#155"), available_ticket("#200")];
    let events = vec![capacity_event(
        "#155",
        "dispatch_ticket: deferred_capacity: unchanged routes",
    )];

    suppress_recent_capacity_deferrals(&cfg, &mut snapshot, &events, &[], "real", "real");

    assert_eq!(snapshot.available_tickets.len(), 1);
    assert_eq!(
        snapshot.available_tickets[0].work_id.as_deref(),
        Some("#200")
    );
    assert_eq!(super::decide_next_action(&snapshot).work_id(), Some("#200"));
}

#[test]
fn capacity_deferred_gitlab_mr_does_not_starve_another_repair() {
    let mut cfg = controller_config();
    cfg.profiles.get_mut("real").unwrap().provider = "gitlab".into();
    let mut snapshot = empty_snapshot();
    snapshot.profile.provider = "gitlab".into();
    snapshot.merge_requests = vec![needs_fix_mr("#155"), needs_fix_mr("#200")];
    let events = vec![capacity_event(
        "#155",
        "fix_existing: deferred_capacity: all subscription routes unavailable",
    )];

    suppress_recent_capacity_deferrals(&cfg, &mut snapshot, &events, &[], "real", "real");

    assert_eq!(snapshot.merge_requests.len(), 1);
    assert_eq!(snapshot.merge_requests[0].work_id.as_deref(), Some("#200"));
    assert_eq!(super::decide_next_action(&snapshot).work_id(), Some("#200"));
}

#[test]
fn capacity_deferral_survives_stuck_action_redispatch() {
    let cfg = controller_config();
    let mut initial = empty_snapshot();
    initial.merge_requests = vec![needs_review_mr("#64"), needs_fix_mr("#155")];
    initial.available_tickets = vec![available_ticket("#200")];
    let events = vec![capacity_event(
        "#155",
        "fix_existing: deferred_capacity: all subscription routes unavailable",
    )];

    let capacity_deferred =
        suppress_recent_capacity_deferrals(&cfg, &mut initial, &events, &[], "real", "real");
    assert_eq!(super::decide_next_action(&initial).work_id(), Some("#64"));

    // A stuck-loop transition rebuilds the snapshot before selecting the next
    // item. The rebuilt view contains #155 again, so both the original action
    // and inherited capacity deferrals must be filtered before re-deciding.
    let mut rebuilt = empty_snapshot();
    rebuilt.merge_requests = vec![needs_review_mr("#64"), needs_fix_mr("#155")];
    rebuilt.available_tickets = vec![available_ticket("#200")];
    let mut inherited_exclusions = capacity_deferred;
    inherited_exclusions.insert("#64".into());
    retain_snapshot_candidates(&mut rebuilt, &inherited_exclusions, &HashSet::new());

    assert_eq!(super::decide_next_action(&rebuilt).work_id(), Some("#200"));
}

#[test]
fn terminal_parallel_result_suppresses_refill_for_the_rest_of_the_iteration() {
    let mut remaining = 0;
    let mut suppressed = false;

    assert!(!update_parallel_refill_budget(
        "Deferred escalate because configured route capacity is busy; no backend launched",
        2,
        &mut remaining,
        &mut suppressed,
    ));
    assert!(suppressed);
    assert_eq!(remaining, 0);

    // A later successful sibling must not reopen the refill slot.
    assert!(!update_parallel_refill_budget(
        "Dispatched review for branch 'gah/example'",
        2,
        &mut remaining,
        &mut suppressed,
    ));
    assert_eq!(remaining, 0);
}

#[test]
fn failed_parallel_result_is_distinct_from_successful_refill() {
    let mut remaining = 0;
    let mut suppressed = false;
    assert!(!update_parallel_refill_budget(
        "Dispatched ticket #1",
        2,
        &mut remaining,
        &mut suppressed,
    ));
    assert_eq!(remaining, 2);

    assert!(update_parallel_refill_budget(
        "Error: repair preflight failed",
        2,
        &mut remaining,
        &mut suppressed,
    ));
    assert!(suppressed);
    assert_eq!(remaining, 0);
}

#[test]
fn operator_shutdown_suppresses_refill_without_becoming_a_worker_failure() {
    let mut remaining = 2;
    let mut suppressed = false;
    assert!(!update_parallel_refill_budget(
        "Error: shutdown requested while codex was running",
        2,
        &mut remaining,
        &mut suppressed,
    ));
    assert!(suppressed);
    assert_eq!(remaining, 0);
}

#[test]
fn route_state_fingerprint_changes_with_effective_configuration() {
    let mut cfg = controller_config();
    let now = time::OffsetDateTime::now_utc();
    let before = route_state_fingerprint(&cfg, "real", now).unwrap();
    cfg.profiles.get_mut("real").unwrap().routing.default_model = Some("a-different-model".into());
    let after = route_state_fingerprint(&cfg, "real", now).unwrap();
    assert_ne!(before, after);
}

#[test]
fn route_state_fingerprint_changes_when_a_cooldown_expires() {
    let tmp = tempfile::tempdir().unwrap();
    let availability_path = tmp.path().join("availability.json");
    let _guard = crate::test_support::AvailabilityEnvGuard::set(&availability_path);
    let state = crate::availability::AvailabilityState {
        version: crate::availability::CURRENT_VERSION,
        records: vec![crate::availability::AvailabilityRecord {
            backend: "agy".into(),
            backend_instance: None,
            model: Some("Claude Sonnet 4.8 (Thinking)".into()),
            quota_pool: Some("agy-1".into()),
            status: crate::availability::Status::Unavailable,
            reason: crate::availability::Reason::QuotaExhausted,
            observed_at: "2026-07-17T08:00:00Z".into(),
            unavailable_until: Some("2026-07-17T08:10:00Z".into()),
            source: crate::availability::Source::BackendError,
            last_error_summary: None,
        }],
    };
    std::fs::write(&availability_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let parse = |timestamp| {
        time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    let cfg = controller_config();
    let blocked = route_state_fingerprint(&cfg, "real", parse("2026-07-17T08:09:00Z")).unwrap();
    let recovered = route_state_fingerprint(&cfg, "real", parse("2026-07-17T08:11:00Z")).unwrap();
    assert_ne!(blocked, recovered);
}

#[test]
fn route_state_fingerprint_is_stable_across_config_reloads_with_hash_maps() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_support::AvailabilityEnvGuard::set(
        tmp.path().join("missing-availability.json"),
    );
    let source = r#"
[profiles.real]
display_name = "Real"
repo_id = "real"
provider = "github"
repo = "owner/real"
local_path = "/tmp/real"
artifact_root = "/tmp/real-artifacts"
default_target_branch = "main"

[profiles.real.max_concurrent_per_model]
"agy/model-a" = 1
"agy-second/model-a" = 1
"codex/model-b" = 2
"opencode/model-c" = 1
"vibe/model-d" = 1

[profiles.real.opencode_idle_timeout_seconds_by_model]
"model-a" = 60
"model-b" = 120
"model-c" = 180
"model-d" = 240
"model-e" = 300
"#;
    let now = time::OffsetDateTime::now_utc();
    let fingerprints = (0..32)
        .map(|_| {
            let cfg: crate::config::GahConfig = toml::from_str(source).unwrap();
            route_state_fingerprint(&cfg, "real", now).unwrap()
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(fingerprints.len(), 1);
}
