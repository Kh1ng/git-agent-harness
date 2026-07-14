use super::super::test_util::{init_repo, profile};
use super::*;
use crate::ledger::LedgerEntry;
use crate::test_support::PathGuard;
use std::fs;
use std::path::Path;
use std::process::Command;

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
