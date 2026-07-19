use super::*;
use crate::availability::{AvailabilityRecord, AvailabilityState, Reason, Source, Status};
use crate::ledger::{LedgerEntry, RoutingCandidateDiagnostic, RoutingDiagnostics};
use crate::test_support::{ClaimStateEnvGuard, ExecGuard, PathGuard};
use std::fs;
use tempfile::TempDir;

#[test]
fn pm_parent_projection_uses_provider_child_state() {
    use std::os::unix::fs::PermissionsExt;

    let _exec_guard = ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        r#"#!/bin/sh
case "$*" in
  *"/issues?state=all"*) printf '%s\n' '[{"id":600,"number":600,"html_url":"https://example/issues/600","state":"closed","title":"Done","body":"","labels":[]},{"id":601,"number":601,"html_url":"https://example/issues/601","state":"open","title":"Open","body":"","labels":[]}]' ;;
  *) echo "unexpected gh invocation: $*" >&2; exit 1 ;;
esac
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&gh).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&gh, permissions).unwrap();
    let _path = PathGuard::set(&bin);
    let profile = crate::ledger::test_util::profile();
    let mut entry = LedgerEntry::new(
        "real",
        &profile,
        "control-plane",
        "pm_publish",
        "#561",
        None,
        None,
    );
    entry.work_id = Some("#561".into());
    entry.source_issue_number = Some("561".into());
    entry.pm_plan_fingerprint = Some("plan-a".into());
    entry.pm_publication_status = Some("published".into());
    entry.pm_child_issue_numbers = vec!["600".into(), "601".into()];

    let (states, attempts, error) = pm::project(&profile, "real", &[entry]);

    assert!(error.is_none());
    assert!(attempts.is_empty());
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].open_child_count, 1);
    assert!(!states[0].completed);
    assert!(!states[0].reconciled);
}

fn make_test_cfg(tmp: &TempDir) -> GahConfig {
    let path = tmp.path().join("cfg.toml");
    fs::write(
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
    let mut cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();
    // Keep every status test's ledger inside its own temp directory
    // without mutating the process-global GAH_LEDGER_PATH override.
    cfg.defaults.artifact_root = tmp.path().to_string_lossy().into_owned();
    cfg
}

#[test]
fn empty_clean_profile_snapshot() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    // Force availability and ledger to be read from temp
    let _availability_guard =
        crate::test_support::AvailabilityEnvGuard::set(tmp.path().join("avail.json"));

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "test", now).unwrap();

    assert_eq!(snap.schema_version, 1);
    assert_eq!(snap.profile.profile, "test");
    assert_eq!(snap.observations.ledger.status, "ok");
    assert_eq!(snap.observations.availability.status, "ok");
    assert!(snap.merge_requests.is_empty());
    assert!(snap.availability.is_empty());
    assert!(snap.recent_ledger.is_none());
    assert!(snap.blockers.is_empty());
    assert!(snap.constraints.is_empty());
}

#[test]
fn effective_intake_policy_does_not_invent_a_gitlab_owner_allowlist() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = make_test_cfg(&tmp);
    let profile = cfg.profiles.get_mut("test").unwrap();
    profile.provider = "gitlab".into();
    profile.repo = "group/project".into();
    profile.publishing.github_issue_author_allowlist = Some(vec!["github-only".into()]);

    let policy = effective_issue_intake_policy(profile);

    assert!(policy.trusted_human_authors.is_empty());
    assert_eq!(policy.github_issue_author_allowlist, vec!["github-only"]);
}

#[test]
fn build_snapshot_reads_ledger_once() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let _availability_guard =
        crate::test_support::AvailabilityEnvGuard::set(tmp.path().join("avail.json"));

    crate::ledger::reset_read_entries_call_count(&cfg);
    let snap = build_snapshot(&cfg, "test", OffsetDateTime::now_utc()).unwrap();
    assert_eq!(crate::ledger::read_entries_call_count(&cfg), 1);
    assert!(snap.blockers.is_empty());
}

#[test]
fn malformed_ledger_is_reported_in_a_partial_snapshot() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let _availability_guard =
        crate::test_support::AvailabilityEnvGuard::set(tmp.path().join("avail.json"));
    fs::write(cfg.defaults.ledger_path(), "not valid json\n").unwrap();

    let snap = build_snapshot(&cfg, "test", OffsetDateTime::now_utc()).unwrap();

    assert_eq!(snap.observations.ledger.status, "error");
    assert!(snap.errors.iter().any(|error| {
        error.subsystem == "ledger"
            && error.incomplete_snapshot
            && error.message.contains("parsing ledger entry 1")
    }));
}

#[test]
fn canonical_claim_scope_marks_active_ticket_and_claims() {
    let tmp = TempDir::new().unwrap();
    let local_path = tmp.path().join("repo");
    fs::create_dir_all(local_path.join("docs").join("tickets")).unwrap();
    let cfg_path = tmp.path().join("status-canonical.toml");
    fs::write(
        &cfg_path,
        r#"
[defaults]
artifact_root = "{artifact_root}"
worktree_base = "{artifact_root}"
llm_base_url  = "http://localhost:4000"
llm_model_local = "local/test"
llm_model_cloud = "cloud/test"

[profiles.gah]
display_name          = "Profile gah"
repo_id               = "gah"
provider              = ""
repo                  = "owner/gah"
local_path            = "{local_path}"
artifact_root         = "{artifact_root}/profiles/gah"
default_target_branch = "main"
"#
        .replace("{artifact_root}", &tmp.path().to_string_lossy())
        .replace("{local_path}", &local_path.to_string_lossy()),
    )
    .unwrap();
    let mut cfg = crate::config::load(Some(cfg_path.to_str().unwrap())).unwrap();
    cfg.defaults.artifact_root = tmp.path().to_string_lossy().into_owned();
    let _availability_guard =
        crate::test_support::AvailabilityEnvGuard::set(tmp.path().join("avail.json"));
    let _claim_guard = ClaimStateEnvGuard::set(tmp.path().join("claims.json"));

    let ticket_dir = local_path.join("docs/tickets");
    fs::write(
        ticket_dir.join("TICKET-436.md"),
        "# TICKET-436: canonical scope test\n\nGoal: keep canonical claim scope stable\n\nRecommended backend: codex\n",
    )
    .unwrap();

    let claim_state = serde_json::json!({
        "version": 2u32,
        "claims": {
            "gah@gah": [
                {
                    "work_id": "TICKET-436",
                    "pid": std::process::id(),
                    "hostname": "localhost",
                    "claimed_at": "2026-07-14T00:00:00Z"
                }
            ]
        }
    });
    std::fs::write(
        tmp.path().join("claims.json"),
        serde_json::to_string(&claim_state).unwrap(),
    )
    .unwrap();

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "gah", now).unwrap();

    assert_eq!(snap.active_claims.len(), 1);
    assert_eq!(snap.active_claims[0].work_id, "TICKET-436");
    assert_eq!(snap.active_claims[0].scope, "gah@gah");
    assert_eq!(snap.available_tickets.len(), 1);
    assert_eq!(
        snap.available_tickets[0].work_id.as_deref(),
        Some("TICKET-436")
    );
    assert!(snap.available_tickets[0].has_active_claim);
}

#[test]
fn malformed_claim_state_is_reported_in_snapshot_errors() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let _availability_guard =
        crate::test_support::AvailabilityEnvGuard::set(tmp.path().join("avail.json"));
    let _claim_guard = ClaimStateEnvGuard::set(tmp.path().join("claims.json"));
    std::fs::write(tmp.path().join("claims.json"), "not valid json\n").unwrap();

    let snap = build_snapshot(&cfg, "test", OffsetDateTime::now_utc()).unwrap();
    assert!(snap
        .errors
        .iter()
        .any(|error| error.subsystem == "claims" && error.incomplete_snapshot));
}

#[test]
fn active_backend_wide_block() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let avail_path = tmp.path().join("avail.json");
    let _availability_guard = crate::test_support::AvailabilityEnvGuard::set(&avail_path);

    let state = AvailabilityState {
        version: 1,
        records: vec![AvailabilityRecord {
            backend: "claude".into(),
            model: None,
            quota_pool: None,
            status: Status::Unavailable,
            reason: Reason::RateLimited,
            observed_at: "2026-07-04T00:00:00Z".into(),
            unavailable_until: Some("2099-01-01T00:00:00Z".into()),
            source: Source::BackendError,
            last_error_summary: None,
        }],
    };
    fs::write(&avail_path, serde_json::to_string(&state).unwrap()).unwrap();

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "test", now).unwrap();

    assert_eq!(snap.availability.len(), 1);
    assert_eq!(
        snap.availability[0].observed_at.as_deref().unwrap(),
        "2026-07-04T00:00:00Z"
    );
    assert_eq!(snap.constraints.len(), 1);
    assert_eq!(snap.constraints[0].kind, "backend_unavailable");
    assert_eq!(snap.constraints[0].backend.as_deref().unwrap(), "claude");
}

#[test]
fn model_specific_availability_block_preserves_scope() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let avail_path = tmp.path().join("avail.json");
    let _availability_guard = crate::test_support::AvailabilityEnvGuard::set(&avail_path);

    let state = AvailabilityState {
        version: 1,
        records: vec![AvailabilityRecord {
            backend: "claude".into(),
            model: Some("claude-3-5".into()),
            quota_pool: None,
            status: Status::Unavailable,
            reason: Reason::RateLimited,
            observed_at: "2026-07-04T00:00:00Z".into(),
            unavailable_until: Some("2099-01-01T00:00:00Z".into()),
            source: Source::BackendError,
            last_error_summary: None,
        }],
    };
    fs::write(&avail_path, serde_json::to_string(&state).unwrap()).unwrap();

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "test", now).unwrap();

    assert_eq!(
        snap.availability[0].scope.as_deref().unwrap(),
        "model_specific"
    );
    assert_eq!(snap.availability[0].model.as_deref().unwrap(), "claude-3-5");
    assert_eq!(snap.constraints[0].model.as_deref().unwrap(), "claude-3-5");
}

#[test]
fn expired_availability_record_skipped() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let avail_path = tmp.path().join("avail.json");
    let _availability_guard = crate::test_support::AvailabilityEnvGuard::set(&avail_path);

    let state = AvailabilityState {
        version: 1,
        records: vec![AvailabilityRecord {
            backend: "claude".into(),
            model: None,
            quota_pool: None,
            status: Status::Unavailable,
            reason: Reason::RateLimited,
            observed_at: "2026-07-04T00:00:00Z".into(),
            unavailable_until: Some("2020-01-01T00:00:00Z".into()), // Past
            source: Source::BackendError,
            last_error_summary: None,
        }],
    };
    fs::write(&avail_path, serde_json::to_string(&state).unwrap()).unwrap();

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "test", now).unwrap();

    assert_eq!(snap.availability.len(), 1);
    assert!(snap.availability[0].eligible_now);
    assert!(snap.constraints.is_empty());
}

#[test]
fn human_required_state_becomes_blocker() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let ledger_path = tmp.path().join("ledger.jsonl");

    let mut entry = LedgerEntry::new(
        "test",
        &cfg.profiles["test"],
        "test",
        "test",
        "test",
        None,
        None,
    );
    entry.human_required = true;
    entry.timestamp = "2026-07-04T00:00:00Z".into();
    fs::write(&ledger_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "test", now).unwrap();

    assert!(snap.recent_ledger.unwrap().human_required);
    // TICKET-human-required-scoping: an unassociated historical entry is
    // informational only; blockers are emitted only for current work.
    assert!(snap.blockers.is_empty());
    assert!(snap.blocked_work_items.is_empty());
}

#[test]
fn work_item_reason_code_reaches_the_status_blocker() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = make_test_cfg(&tmp);
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    fs::write(
        repo.join("docs/tickets/TICKET-300-test.md"),
        "# TICKET-300: Test ticket\n\nGoal: test\n",
    )
    .unwrap();
    cfg.profiles.get_mut("test").unwrap().local_path = repo.display().to_string();
    cfg.profiles.get_mut("test").unwrap().provider.clear();

    let mut entry = LedgerEntry::new(
        "test",
        &cfg.profiles["test"],
        "claude",
        "review",
        "test",
        None,
        None,
    );
    entry.work_id = Some("TICKET-300".into());
    entry.mode = "fix".into();
    entry.human_required = true;
    entry.human_required_reason_code = Some("stuck_loop_gate".into());
    crate::ledger::append(&cfg, &entry).unwrap();

    let snap = build_snapshot(&cfg, "test", OffsetDateTime::now_utc()).unwrap();
    let blocker = snap
        .blocked_work_items
        .iter()
        .find(|blocker| blocker.source_reference.as_deref() == Some("TICKET-300"))
        .expect("ticket-scoped human hold must be visible");
    assert_eq!(blocker.reason.as_deref(), Some("stuck_loop_gate"));
    assert_eq!(blocker.reason_code.as_deref(), Some("stuck_loop_gate"));
}

#[test]
fn partial_subsystem_error_is_in_errors() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let avail_path = tmp.path().join("avail.json");
    let _availability_guard = crate::test_support::AvailabilityEnvGuard::set(&avail_path);

    // Write garbage JSON to force parsing error
    fs::write(&avail_path, "{garbage").unwrap();

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "test", now).unwrap();

    assert_eq!(snap.observations.availability.status, "error");

    let avail_error = snap
        .errors
        .iter()
        .find(|e| e.subsystem == "availability")
        .unwrap();
    assert!(avail_error.incomplete_snapshot);
}

#[test]
fn ledger_failure_and_attempt_fields_are_populated() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let ledger_path = tmp.path().join("ledger.jsonl");

    let mut entry = LedgerEntry::new(
        "test",
        &cfg.profiles["test"],
        "test",
        "test",
        "test",
        None,
        None,
    );
    entry.failure_class = Some("backend_error".into());
    entry.failure_stage = Some("agent_run".into());
    entry.attempts_started = Some(3);
    entry.attempts_completed = Some(2);
    entry.review_timeout_class = Some("idle".into());
    entry.review_idle_timeout_seconds = Some(300);
    entry.review_hard_timeout_seconds = Some(3600);
    entry.review_last_progress_secs = Some(42.5);
    entry.timestamp = "2026-07-04T00:00:00Z".into();
    fs::write(&ledger_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "test", now).unwrap();

    let summary = snap.recent_ledger.unwrap();
    assert_eq!(
        summary.most_recent_failure_class.as_deref(),
        Some("backend_error")
    );
    assert_eq!(
        summary.most_recent_failure_stage.as_deref(),
        Some("agent_run")
    );
    assert_eq!(summary.attempts_started, Some(3));
    assert_eq!(summary.attempts_completed, Some(2));
    assert_eq!(summary.review_timeout_class.as_deref(), Some("idle"));
    assert_eq!(summary.review_idle_timeout_seconds, Some(300));
    assert_eq!(summary.review_hard_timeout_seconds, Some(3600));
    assert_eq!(summary.review_last_progress_secs, Some(42.5));
}

#[test]
fn recent_ledger_exposes_work_id() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let ledger_path = tmp.path().join("ledger.jsonl");

    let mut entry = LedgerEntry::new(
        "test",
        &cfg.profiles["test"],
        "codex",
        "fix",
        "docs/tickets/TICKET-095-ledger-work-identity.md",
        None,
        None,
    );
    entry.work_id = Some("TICKET-095".into());
    entry.timestamp = "2026-07-04T00:00:00Z".into();
    fs::write(&ledger_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();

    let now = OffsetDateTime::now_utc();
    let snap = build_snapshot(&cfg, "test", now).unwrap();

    assert_eq!(
        snap.recent_ledger.unwrap().most_recent_work_id.as_deref(),
        Some("TICKET-095")
    );
}

#[test]
fn recent_ledger_exposes_routing_diagnostics() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_test_cfg(&tmp);
    let ledger_path = tmp.path().join("ledger.jsonl");

    let mut entry = LedgerEntry::new(
        "test",
        &cfg.profiles["test"],
        "codex",
        "fix",
        "test",
        None,
        None,
    );
    entry.timestamp = "2026-07-04T00:00:00Z".into();
    entry.routing_diagnostics = Some(RoutingDiagnostics {
        policy_reordered_candidates: true,
        selected_backend: Some("codex".into()),
        selected_model: Some("gpt-5.4".into()),
        selected_quota_pool: Some("codex-main".into()),
        selected_pace_band: Some("aggressive_burn".into()),
        selected_cost_class: Some("included_quota".into()),
        selected_over: vec!["openhands/gpt-5.4 (paid $0.2500)".into()],
        candidates: vec![RoutingCandidateDiagnostic {
            backend: "codex".into(),
            model: Some("gpt-5.4".into()),
            quota_pool: Some("codex-main".into()),
            default_order: Some(1),
            consideration_order: Some(0),
            pace_band: Some("aggressive_burn".into()),
            cost_class: Some("included_quota".into()),
            skip_reason: None,
            unavailable_until: None,
        }],
        human_summary: Some("selected codex/gpt-5.4".into()),
    });
    fs::write(&ledger_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();

    let snap = build_snapshot(&cfg, "test", OffsetDateTime::now_utc()).unwrap();
    let diagnostics = snap.recent_ledger.unwrap().routing_diagnostics.unwrap();
    assert!(diagnostics.policy_reordered_candidates);
    assert_eq!(
        diagnostics.selected_quota_pool.as_deref(),
        Some("codex-main")
    );
}

#[test]
fn mr_classification_and_recommended_action_stable() {
    let mr = sync::SyncMr {
        title: "Test PR".into(),
        body: None,
        branch: "gah/test-branch".into(),
        labels: vec!["gah-ready-for-human".into()],
        url: Some("https://github.com/owner/repo/pull/1".into()),
        id: Some("1".into()),
        state: Some("OPEN".into()),
        draft: false,
        source_sha: None,
        merge_status: Some("CLEAN".into()),
        merged: false,
        updated_at: None,
        merged_at: None,
        ci_failed: false,
        ci_passed: false,
        ci_pending: false,
        work_id: None,
    };
    let class = sync::classify(&mr);
    assert_eq!(class, "READY_FOR_HUMAN");
    let action = sync::RecommendedAction::from_class(class);
    assert_eq!(action, sync::RecommendedAction::HumanMergeDecision);
}

#[test]
fn mr_closed_unmerged_is_terminal_in_snapshot() {
    let mr = sync::SyncMr {
        title: "Closed PR".into(),
        body: None,
        branch: "gah/closed-branch".into(),
        labels: vec!["gah-human-review".into()],
        url: Some("https://github.com/owner/repo/pull/2".into()),
        id: Some("2".into()),
        state: Some("closed".into()),
        draft: true,
        source_sha: None,
        merge_status: Some("DIRTY".into()),
        merged: false,
        updated_at: None,
        merged_at: None,
        ci_failed: true,
        ci_passed: false,
        ci_pending: false,
        work_id: None,
    };
    let class = sync::classify(&mr);
    assert_eq!(class, "CLOSED_UNMERGED");
    let action = sync::RecommendedAction::from_class(class);
    assert_eq!(action, sync::RecommendedAction::None);
}
