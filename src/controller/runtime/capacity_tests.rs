use super::{route_state_fingerprint, suppress_recent_capacity_deferrals};
use crate::models::AvailableTicket;

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
        fix_attempt_counts: std::collections::HashMap::new(),
        merge_attempt_counts: std::collections::HashMap::new(),
        review_held_work_ids: std::collections::HashSet::new(),
        publishing_allow_pr: true,
        generated_artifact_deny_patterns: vec![],
        max_parallel_workers: 1,
        backend_configured: std::collections::HashMap::new(),
    }
}

fn available_ticket(work_id: &str) -> AvailableTicket {
    AvailableTicket {
        ticket_path: work_id.trim_start_matches('#').into(),
        work_id: Some(work_id.into()),
        title: Some(format!("Work {work_id}")),
        has_active_mr: false,
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
        classification: "NEEDS_FIX".into(),
        recommended_action: crate::sync::RecommendedAction::ReuseBranch,
    }
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
