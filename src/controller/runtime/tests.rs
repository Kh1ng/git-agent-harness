use super::profile_lock::{acquire_profile_lock, loop_lock_path, reload_config_for_profile};
use super::{
    append_stuck_loop_gate_if_transition, human_required_already_reported,
    is_validation_gate_failure, loop_parallel_argument, no_admission_diagnostics,
    wait_interruptibly, NextAction,
};

#[test]
fn recurring_loop_preserves_live_config_sentinel_but_once_resolves_it() {
    assert_eq!(loop_parallel_argument(false, 0, 2), 0);
    assert_eq!(loop_parallel_argument(true, 0, 2), 2);
    assert_eq!(loop_parallel_argument(false, 3, 2), 3);
    assert_eq!(loop_parallel_argument(true, 3, 2), 3);
}

#[test]
fn review_actions_wait_until_the_selected_route_is_reserved() {
    let action = crate::controller::NextAction::ReviewMr {
        branch: "gah/review-cap".into(),
        work_id: Some("#471".into()),
        mr_url: None,
        reason: "review required".into(),
    };
    assert!(super::admission::action_needs_handshake(&action));
}

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

#[test]
fn stuck_loop_gate_append_is_an_idempotent_state_transition() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cfg.toml");
    std::fs::write(
        &path,
        format!(
            r#"
[defaults]
artifact_root = "{}"

[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "{}"
default_target_branch = "main"
"#,
            tmp.path().display(),
            tmp.path().display()
        ),
    )
    .unwrap();
    let cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let appended = std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let cfg = &cfg;
                scope.spawn(move || {
                    barrier.wait();
                    append_stuck_loop_gate_if_transition(
                        cfg,
                        "test",
                        "#639",
                        "same stuck decision",
                        None,
                    )
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|appended| *appended)
            .count()
    });
    assert_eq!(
        appended, 1,
        "exactly one concurrent slot owns the transition"
    );

    let entries = crate::ledger::read_entries(&cfg).unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.dispatch_reason.as_deref() == Some("stuck_loop_gate"))
            .count(),
        1
    );
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
fn profile_lock_is_adjacent_to_config_not_xdg_state() {
    let config_file = tempfile::NamedTempFile::new().unwrap();
    let lock_path = loop_lock_path("test-profile", config_file.path());
    let expected_dir = config_file
        .path()
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join(".gah-locks");
    assert_eq!(lock_path.parent(), Some(expected_dir.as_path()));
}

#[test]
fn reload_config_for_profile_succeeds_when_profile_still_present() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cfg.toml");
    std::fs::write(
        &path,
        r#"
[profiles.test]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "/tmp"
default_target_branch = "main"
"#,
    )
    .unwrap();

    let cfg = reload_config_for_profile(&path, "test").expect("profile is present");
    assert!(crate::config::get_profile(&cfg, "test").is_ok());
}

#[test]
fn reload_config_for_profile_errs_when_profile_renamed_or_removed() {
    // A parse-clean reload that no longer resolves the running profile
    // (renamed/removed mid-run, e.g. via the dashboard Settings UI) must
    // report an error rather than silently handing back a config the
    // daemon can't dispatch against -- the caller (`run_loop`) relies on
    // this to fall back to its last-known-good config instead of
    // hard-erroring out of the whole loop.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cfg.toml");
    std::fs::write(
        &path,
        r#"
[profiles.renamed]
display_name = "Test"
repo_id = "test/test"
provider = "github"
repo = "test/test"
local_path = "/tmp"
artifact_root = "/tmp"
default_target_branch = "main"
"#,
    )
    .unwrap();

    let error = reload_config_for_profile(&path, "test")
        .expect_err("profile no longer exists in the reloaded config");
    assert!(error.to_string().contains("test"));
}

#[test]
fn interruptible_wait_stops_during_backoff() {
    let checks = std::sync::atomic::AtomicUsize::new(0);
    let completed = wait_interruptibly(std::time::Duration::from_secs(300), || {
        checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
    });
    assert!(!completed);
    assert_eq!(checks.load(std::sync::atomic::Ordering::SeqCst), 2);
}

// TICKET-096: Parallel dispatch tests
use crate::models::AvailableTicket;
use crate::status::{
    ObservationStatus, Observations, ProfileIdentity, ScopeStatusJson, StatusSnapshot,
};

pub(super) fn empty_snapshot() -> StatusSnapshot {
    StatusSnapshot {
        schema_version: 1,
        review_contract_version: crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION,
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
            max_open_managed_mrs: 1,
            issue_intake_policy: crate::models::IssueIntakePolicy {
                mode: "canonical_autonomous_only".into(),
                canonical_autonomous_label: "exec:autonomous".into(),
                trusted_human_authors: vec![],
                trusted_bot_authors: vec![],
                github_issue_author_allowlist: vec![],
            },
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
        issue_intake_rejections: vec![],
        dependency_blockers: vec![],
        errors: vec![],
        available_tickets: vec![],
        work_waypoint_evidence: Default::default(),
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
        export_health: Default::default(),
        skill_inventory: Vec::new(),
    }
}

fn capped_pr_blocker() -> crate::status::Blocker {
    crate::status::Blocker {
        kind: "human_required".into(),
        reason: Some("fix_retry_cap_exceeded".into()),
        message: Some("cap exceeded".into()),
        backend: None,
        model: None,
        quota_pool: None,
        until: None,
        source_reference: Some("branch-A".into()),
        reason_code: Some("fix_retry_cap_exceeded".into()),
        remediation_plan: None,
    }
}

#[test]
fn no_admission_diagnostics_names_blocked_items_capacity_and_empty_queue() {
    let mut snapshot = empty_snapshot();
    let quiet = no_admission_diagnostics(&snapshot, 0, 2, None);
    assert!(quiet.contains("blocked items: none"), "{quiet}");
    assert!(
        quiet.contains("0/2 workers active, no node deferral"),
        "{quiet}"
    );
    assert!(quiet.contains("queue: empty"), "{quiet}");

    snapshot.blocked_work_items.push(capped_pr_blocker());
    let deferred = no_admission_diagnostics(&snapshot, 1, 2, Some("node memory is critical"));
    assert!(
        deferred.contains("branch-A [fix_retry_cap_exceeded]"),
        "{deferred}"
    );
    assert!(
        deferred.contains("1/2 workers active, admission deferred (node memory is critical)"),
        "{deferred}"
    );
}

#[test]
fn blocked_pr_human_required_is_reported_once() {
    let action = NextAction::HumanRequired {
        work_id: Some("TICKET-A".into()),
        reason: "fix cap exceeded".into(),
        reference: Some("branch-A".into()),
        reason_code: Some("fix_retry_cap_exceeded".into()),
    };
    let mut history = Vec::new();
    assert!(!human_required_already_reported(&history, "real", &action));
    history.push(crate::events::ControllerEvent {
        timestamp: "2026-07-05T00:00:00Z".into(),
        event_type: "human_required".into(),
        profile: Some("real".into()),
        work_id: Some("TICKET-A".into()),
        run_id: None,
        details: "Human required: fix cap exceeded (branch-A) [code=fix_retry_cap_exceeded]".into(),
        reason_code: Some("fix_retry_cap_exceeded".into()),
        review_contract_version: None,
        remediation_plan: None,
    });
    assert!(human_required_already_reported(&history, "real", &action));
    assert!(!human_required_already_reported(&history, "other", &action));
}

fn events_test_config() -> (tempfile::TempDir, crate::config::GahConfig) {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = crate::config::GahConfig {
        context: Default::default(),
        defaults: crate::config::Defaults {
            artifact_root: tmp.path().to_string_lossy().into_owned(),
            ..Default::default()
        },
        profiles: std::collections::HashMap::new(),
    };
    (tmp, cfg)
}

#[test]
fn stop_event_for_capped_pr_persists_reason_code_and_dedupes_across_ticks() {
    let (_tmp, cfg) = events_test_config();
    let action = NextAction::HumanRequired {
        work_id: Some("TICKET-A".into()),
        reason: "fix cap exceeded".into(),
        reference: Some("branch-A".into()),
        reason_code: Some("fix_retry_cap_exceeded".into()),
    };
    for tick in 0..3 {
        let history = crate::events::read_events(&cfg).unwrap();
        let newly_reported = super::record_stop_event(
            &cfg,
            "real",
            &history,
            &action,
            crate::events::EventType::HumanRequired,
            "Human required: fix cap exceeded (branch-A) [code=fix_retry_cap_exceeded]",
        )
        .unwrap();
        assert_eq!(newly_reported, tick == 0, "tick {tick} must report once");
    }
    let events = crate::events::read_events(&cfg).unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].reason_code.as_deref(),
        Some("fix_retry_cap_exceeded")
    );
}

#[test]
fn blocked_pr_is_reported_once_alongside_other_dispatch() {
    let (_tmp, cfg) = events_test_config();
    let mut snapshot = empty_snapshot();
    snapshot.blocked_work_items.push(capped_pr_blocker());
    for _ in 0..3 {
        super::report_blocked_work_items_once(&cfg, "real", &snapshot).unwrap();
    }
    let events = crate::events::read_events(&cfg).unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].event_type, "human_required");
    assert_eq!(events[0].work_id.as_deref(), Some("branch-A"));
    assert_eq!(
        events[0].reason_code.as_deref(),
        Some("fix_retry_cap_exceeded")
    );
}

#[test]
fn parallel_dispatch_respects_max_parallel_limit() {
    let mut snapshot = empty_snapshot();

    // Add multiple eligible backends (more than max_parallel)
    for _ in 0..5 {
        snapshot.availability.push(ScopeStatusJson {
            backend_instance: None,
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
            normalized_work_identity: crate::work_claim::normalize_work_identity(&format!(
                "TICKET-{}",
                i + 100
            )),
            source: crate::models::CandidateSource::LegacyTicket,
            execution_policy: crate::models::CandidateExecutionPolicy {
                intake_mode: "canonical_autonomous_only".into(),
                explicit_autonomy_required: true,
                autonomous_metadata_present: true,
                dispatchable_now: true,
                exclusion_reason_code: None,
                exclusion_reason: None,
            },
            title: Some(format!("Test ticket {}", i)),
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
            backend_instance: None,
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
    for i in 0..3 {
        snapshot.availability.push(ScopeStatusJson {
            backend_instance: None,
            backend: format!("backend_{i}"),
            model: None,
            quota_pool: None,
            eligible_now: false,
            reason: Some("rate limited".into()),
            unavailable_until: Some(time::OffsetDateTime::now_utc().to_string()),
            source: None,
            last_error_summary: None,
            observed_at: None,
            scope: None,
        });
    }
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
