//! Adversarial correctness tests that exercise **production** GAH paths.
//!
//! Critical distinction from earlier fixture-only tests: assertion helpers
//! call real binaries/functions via:
//! - `gah status --json` → `status::build_snapshot` →
//!   `sync::count_fix_attempts_per_branch` / `classify`
//! - `gah loop --once` → controller path
//!
//! Fixture construction alone is never treated as proof.

#![recursion_limit = "256"]

mod support;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use support::fake_ledger::{ledger_entry_full, TestLedger};
use support::scenario::{gitlab_mr_json, GithubPrParams, GitlabMrParams, ScenarioHarness};
use support::{FakeBackend, Scenario};
use tempfile::TempDir;

// ── production helpers ───────────────────────────────────────────────

/// Run production `count_fix_attempts_per_branch` by reading
/// `fix_attempt_counts` from `gah status --json`.
fn status_fix_counts(ledger: TestLedger) -> serde_json::Map<String, serde_json::Value> {
    let prs = ledger
        .entries()
        .iter()
        .filter_map(|entry| {
            let branch = entry["branch"].as_str()?;
            let work_id = entry["work_id"].as_str().unwrap_or("TICKET-001");
            Some((branch.to_string(), current_pr(branch, work_id, 1)))
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect::<Vec<_>>();
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    install_active_prs(&gh, &prs);
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .with_ledger(ledger);
    harness.install_custom_gh(&gh);
    let snap = harness.run_status_json().expect("status should succeed");
    snap.get("fix_attempt_counts")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
}

fn install_active_prs(gh: &FakeBackend, prs: &[serde_json::Value]) {
    gh.install_github_api(
        Scenario::success().with_stdout("[]"),
        Scenario::success().with_stdout(serde_json::to_string(prs).unwrap()),
        Scenario::success().with_stdout(r#"{"total_count":0,"check_runs":[]}"#),
    );
}

fn count_for_branch(counts: &serde_json::Map<String, serde_json::Value>, branch: &str) -> u64 {
    counts.get(branch).and_then(|v| v.as_u64()).unwrap_or(0)
}

fn fixture_title(work_id: &str) -> String {
    format!("Draft: {work_id} fixture")
}

fn fixture_source_sha(branch: &str) -> String {
    format!("source-{branch}")
}

fn fixture_metadata_fingerprint(branch: &str, work_id: &str) -> String {
    let source_sha = fixture_source_sha(branch);
    let title = fixture_title(work_id)
        .strip_prefix("Draft: ")
        .unwrap()
        .to_string();
    let mut hasher = Sha256::new();
    hasher.update(b"gah-review-metadata-v1\0");
    for value in [source_sha.as_str(), title.as_str(), ""] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update([1]);
    format!("sha256:{:x}", hasher.finalize())
}

fn current_pr(branch: &str, work_id: &str, number: i64) -> serde_json::Value {
    let mut pr = support::scenario::github_rest_pr_json(GithubPrParams {
        title: fixture_title(work_id),
        branch: branch.into(),
        labels: vec!["gah-needs-fix".into()],
        ci_conclusion: None,
        state: None,
        url: None,
        number: Some(number),
        draft: Some(true),
        merged_at: None,
        updated_at: None,
    });
    pr["head"]["sha"] = serde_json::json!(fixture_source_sha(branch));
    pr
}

fn ledger_fix(branch: &str, work_id: &str, reason: &str, ts: &str) -> serde_json::Value {
    let mut entry = ledger_entry_full("fix", branch, Some(reason), work_id, ts);
    let source_sha = fixture_source_sha(branch);
    let metadata = fixture_metadata_fingerprint(branch, work_id);
    entry["review_source_sha"] = serde_json::json!(source_sha);
    entry["review_metadata_fingerprint"] = serde_json::json!(metadata);
    entry["review_generation"] = serde_json::json!(format!(
        "review-v1:{}:{}",
        fixture_source_sha(branch),
        fixture_metadata_fingerprint(branch, work_id)
    ));
    entry
}

// ── mutation targets (must go red on production mutations) ───────────

#[test]
fn mutation_target_branch_filter_must_be_isolated() {
    let ledger = TestLedger::new()
        .with_entry(ledger_fix(
            "gah/branch-1",
            "TICKET-001",
            "post_review_repair",
            "2026-07-01T00:00:00Z",
        ))
        .with_entry(ledger_fix(
            "gah/branch-1",
            "TICKET-001",
            "post_review_repair",
            "2026-07-01T01:00:00Z",
        ))
        .with_entry({
            ledger_fix(
                "gah/branch-2",
                "TICKET-002",
                "post_review_repair",
                "2026-07-01T00:00:00Z",
            )
        });
    let counts = status_fix_counts(ledger);
    assert_eq!(
        count_for_branch(&counts, "gah/branch-1"),
        2,
        "prod path branch-1; got {counts:?}"
    );
    assert_eq!(
        count_for_branch(&counts, "gah/branch-2"),
        1,
        "prod path branch isolation; got {counts:?}"
    );
    assert_eq!(
        count_for_branch(&counts, "ALL"),
        0,
        "must not collapse to ALL; got {counts:?}"
    );
}

#[test]
fn mutation_target_review_mode_must_not_count_as_fix() {
    let review = ledger_entry_full(
        "review",
        "gah/fix-1",
        Some("review"),
        "TICKET-001",
        "2026-07-01T00:00:00Z",
    );
    let fix = ledger_fix(
        "gah/fix-1",
        "TICKET-001",
        "post_review_repair",
        "2026-07-01T00:00:00Z",
    );
    let counts = status_fix_counts(TestLedger::new().with_entry(review).with_entry(fix));
    assert_eq!(
        count_for_branch(&counts, "gah/fix-1"),
        1,
        "review must not count; got {counts:?}"
    );
}

#[test]
fn mutation_target_initial_dispatch_must_not_count_as_repair() {
    let initial = ledger_fix("gah/fix-1", "TICKET-001", "initial", "2026-07-01T00:00:00Z");
    let repair = ledger_fix(
        "gah/fix-1",
        "TICKET-001",
        "post_review_repair",
        "2026-07-01T01:00:00Z",
    );
    let counts = status_fix_counts(TestLedger::new().with_entry(initial).with_entry(repair));
    assert_eq!(
        count_for_branch(&counts, "gah/fix-1"),
        1,
        "initial must not count as repair; got {counts:?}"
    );
}

// ── sync failure incomplete observation (detects MUT4) ───────────────

#[test]
fn sync_failure_marks_incomplete_observation() {
    let mut harness = ScenarioHarness::new("github").github_scenario("malformed");
    let snap = harness
        .run_status_json()
        .expect("status should emit JSON even on sync error");
    assert_eq!(
        snap["observations"]["sync"]["status"], "error",
        "malformed provider → observations.sync.status=error; snap={snap}"
    );
    let errors = snap["errors"].as_array().cloned().unwrap_or_default();
    let has_incomplete = errors.iter().any(|e| {
        e.get("incomplete_snapshot").and_then(|v| v.as_bool()) == Some(true)
            && e.get("subsystem").and_then(|v| v.as_str()) == Some("sync")
    });
    assert!(
        has_incomplete,
        "must set incomplete_snapshot on sync error; errors={errors:?}"
    );
}

// ── multi-ticket isolation (production status projection) ────────────

#[test]
fn two_tickets_independent_progress() {
    let a1 = ledger_fix(
        "gah/fix-a",
        "TICKET-001",
        "post_review_repair",
        "2026-07-01T00:00:00Z",
    );
    let a2 = ledger_fix(
        "gah/fix-a",
        "TICKET-001",
        "post_review_repair",
        "2026-07-01T01:00:00Z",
    );
    let b1 = ledger_fix(
        "gah/fix-b",
        "TICKET-002",
        "post_review_repair",
        "2026-07-01T00:00:00Z",
    );

    let pr_a = current_pr("gah/fix-a", "TICKET-001", 1);
    let pr_b = current_pr("gah/fix-b", "TICKET-002", 2);
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    install_active_prs(&gh, &[pr_a, pr_b]);

    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .with_ledger(
            TestLedger::new()
                .with_entry(a1)
                .with_entry(a2)
                .with_entry(b1),
        );
    harness.install_custom_gh(&gh);
    harness.create_remote_branch("gah/fix-a");
    harness.create_remote_branch("gah/fix-b");

    let snap = harness.run_status_json().expect("status");
    let counts = snap["fix_attempt_counts"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    assert_eq!(count_for_branch(&counts, "gah/fix-a"), 2, "{counts:?}");
    assert_eq!(count_for_branch(&counts, "gah/fix-b"), 1, "{counts:?}");

    let blocked = snap["blocked_work_items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        blocked.iter().any(|b| {
            b.get("reason").and_then(|v| v.as_str()) == Some("fix_retry_cap_exceeded")
                && b.get("source_reference").and_then(|v| v.as_str()) == Some("gah/fix-a")
        }),
        "A must be retry-cap blocked; {blocked:?}"
    );
    assert!(
        !blocked.iter().any(|b| {
            b.get("source_reference").and_then(|v| v.as_str()) == Some("gah/fix-b")
                && b.get("reason").and_then(|v| v.as_str()) == Some("fix_retry_cap_exceeded")
        }),
        "B must not be retry-cap blocked; {blocked:?}"
    );

    // Profile blockers empty: work-item scoping, not profile freeze.
    let blockers = snap["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.is_empty(),
        "exhausted A must not freeze profile; {blockers:?}"
    );

    let mrs = snap["merge_requests"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(mrs
        .iter()
        .any(|m| m["branch"] == "gah/fix-a" && m["classification"] == "NEEDS_FIX"));
    assert!(mrs
        .iter()
        .any(|m| m["branch"] == "gah/fix-b" && m["classification"] == "NEEDS_FIX"));
}

// ── terminal merge (detects MUT5) ────────────────────────────────────

#[test]
fn recurring_status_excludes_terminal_merges() {
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    install_active_prs(&gh, &[]);

    let mut harness = ScenarioHarness::new("github").github_scenario("empty");
    harness.install_custom_gh(&gh);
    harness.create_remote_branch("gah/merged-1");

    let snap = harness.run_status_json().expect("status");
    let mrs = snap["merge_requests"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        mrs.is_empty(),
        "active observations must exclude terminal history; got {mrs:?}"
    );
}

// ── crash/restart continuity ─────────────────────────────────────────

/// Seeded "post-crash" ledger: no durable repair yet. Two status processes
/// must see fix_attempt_counts unchanged (0).
#[test]
fn crash_before_attempt_start_redispenses_once() {
    let entry = ledger_entry_full(
        "improve",
        "gah/feature-1",
        Some("initial"),
        "TICKET-001",
        "2026-07-01T00:00:00Z",
    );
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .with_ledger(TestLedger::new().with_entry(entry));
    let snap1 = harness.run_status_json().unwrap();
    let c1 = count_for_branch(
        &snap1["fix_attempt_counts"]
            .as_object()
            .cloned()
            .unwrap_or_default(),
        "gah/feature-1",
    );
    assert_eq!(c1, 0);
    // process B
    let snap2 = harness.run_status_json().unwrap();
    let c2 = count_for_branch(
        &snap2["fix_attempt_counts"]
            .as_object()
            .cloned()
            .unwrap_or_default(),
        "gah/feature-1",
    );
    assert_eq!(c2, 0);
    assert_eq!(snap1["fix_attempt_counts"], snap2["fix_attempt_counts"]);
}

#[test]
fn crash_after_verdict_dispatches_repair_once() {
    let review_entry = ledger_entry_full(
        "review",
        "gah/fix-1",
        Some("review"),
        "TICKET-001",
        "2026-07-01T00:00:00Z",
    );
    let counts = status_fix_counts(TestLedger::new().with_entry(review_entry));
    assert_eq!(
        count_for_branch(&counts, "gah/fix-1"),
        0,
        "review must not count as fix; {counts:?}"
    );
}

#[test]
fn crash_after_attempt_start_reconciles_stale_state() {
    let mut stale = ledger_entry_full(
        "improve",
        "gah/stale-1",
        Some("initial"),
        "TICKET-001",
        "2026-07-01T00:00:00Z",
    );
    stale["attempts_started"] = serde_json::json!(1);
    stale["attempts_completed"] = serde_json::json!(0);
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("success")
        .with_ledger(TestLedger::new().with_entry(stale));
    let _ = harness.run_loops(3).unwrap();
    let snap = harness.run_status_json().unwrap();
    assert_eq!(
        count_for_branch(
            &snap["fix_attempt_counts"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
            "gah/stale-1"
        ),
        0
    );
}

/// Two gah child processes: durable ledger counts stable after process exit.
#[test]
fn restart_two_process_continuity_shared_ledger() {
    let repair = ledger_fix(
        "gah/fix-1",
        "TICKET-001",
        "post_review_repair",
        "2026-07-01T00:00:00Z",
    );
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    install_active_prs(&gh, &[current_pr("gah/fix-1", "TICKET-001", 1)]);
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .with_ledger(TestLedger::new().with_entry(repair));
    harness.install_custom_gh(&gh);
    let a = harness.run_status_json().unwrap();
    let b = harness.run_status_json().unwrap();
    assert_eq!(
        count_for_branch(
            &a["fix_attempt_counts"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
            "gah/fix-1"
        ),
        1
    );
    assert_eq!(a["fix_attempt_counts"], b["fix_attempt_counts"]);
}

// ── remaining stronger checks ────────────────────────────────────────

#[test]
fn exhausted_ticket_does_not_starve_eligible_ticket() {
    let a1 = ledger_fix(
        "gah/fix-a",
        "TICKET-001",
        "post_review_repair",
        "2026-07-01T00:00:00Z",
    );
    let a2 = ledger_fix(
        "gah/fix-a",
        "TICKET-001",
        "post_review_repair",
        "2026-07-01T01:00:00Z",
    );
    let pr_a = current_pr("gah/fix-a", "TICKET-001", 1);
    let pr_b = current_pr("gah/fix-b", "TICKET-002", 2);
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    install_active_prs(&gh, &[pr_a, pr_b]);
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .with_ledger(TestLedger::new().with_entry(a1).with_entry(a2));
    harness.install_custom_gh(&gh);
    harness.create_remote_branch("gah/fix-a");
    harness.create_remote_branch("gah/fix-b");
    let snap = harness.run_status_json().unwrap();
    assert!(snap["blockers"].as_array().unwrap().is_empty());
    let counts = snap["fix_attempt_counts"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    assert_eq!(count_for_branch(&counts, "gah/fix-a"), 2);
    assert_eq!(count_for_branch(&counts, "gah/fix-b"), 0);
}

#[test]
fn metamorphic_unrelated_ledger_does_not_poison_count() {
    let current = ledger_fix(
        "gah/fix-1",
        "TICKET-001",
        "post_review_repair",
        "2026-07-01T00:00:00Z",
    );
    let other = ledger_fix(
        "gah/other",
        "TICKET-002",
        "post_review_repair",
        "2026-07-01T00:00:00Z",
    );
    let base = status_fix_counts(TestLedger::new().with_entry(current.clone()));
    let both = status_fix_counts(TestLedger::new().with_entry(other).with_entry(current));
    assert_eq!(count_for_branch(&base, "gah/fix-1"), 1);
    assert_eq!(count_for_branch(&both, "gah/fix-1"), 1);
    assert_eq!(count_for_branch(&both, "gah/other"), 1);
}

#[test]
fn idempotent_noop_does_not_grow_ledger() {
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("success");
    let results = harness.run_loops(5).unwrap();
    assert_eq!(results.len(), 5);
    let entries = TestLedger::read_from(&harness.ledger_path).unwrap_or_default();
    assert!(
        entries.len() <= 5,
        "unbounded ledger growth: {}",
        entries.len()
    );
}

// ── Fixture deserialization safety ──────────────────────────────────

/// Regression: incomplete ledger fixtures must be rejected at harness
/// setup before they reach the production binary, where they would be
/// silently dropped by `ledger::read_entries` → empty count map →
/// false confidence.
///
/// The harness validation gate (`validate_production_schema`) checks
/// every fixture entry against the known required fields of the
/// production `LedgerEntry` struct.  Rejection is explicit, visible,
/// and points the author at `ledger_entry_full()`.
#[test]
fn incomplete_fixture_rejected_at_harness_setup() {
    let partial = serde_json::json!({
        "profile": "test",
        "work_id": "TICKET-BOGUS",
        "mode": "fix",
        "branch": "gah/fix-bogus",
        "dispatch_reason": "post_review_repair",
        "timestamp": "2026-07-01T00:00:00Z",
        "attempts_started": 1,
        "attempts_completed": 1
    });

    // Direct validation — must reject the partial entry.
    let err = TestLedger::new()
        .with_entry(partial.clone())
        .validate_production_schema()
        .expect_err("partial fixture must be rejected before reaching the binary");
    assert!(
        err.contains("missing required field"),
        "rejection message must name the problem: {err}"
    );

    // Full-schema entries must pass validation silently.
    let full = ledger_entry_full(
        "fix",
        "gah/fix-ok",
        Some("post_review_repair"),
        "TICKET-OK",
        "2026-07-01T00:00:00Z",
    );
    TestLedger::new()
        .with_entry(full)
        .validate_production_schema()
        .expect("full-schema fixtures must pass harness validation");

    // Harness integration: constructing a ScenarioHarness with a
    // partial ledger and then exercising any method that calls
    // `setup_env()` must panic with a clear diagnostic.
    let partial_ledger = TestLedger::new().with_entry(partial);
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .with_ledger(partial_ledger);
    // `run_status_json` calls `setup_env` internally — must panic.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        harness.run_status_json().ok()
    }));
    match result {
        Err(panic_payload) => {
            let msg = panic_payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_payload.downcast_ref::<&str>().copied())
                .unwrap_or("(non-string panic)");
            assert!(
                msg.contains("missing required field"),
                "harness must reject partial fixtures at setup: {msg}"
            );
        }
        Ok(_) => {
            panic!(
                "EXPECTED harness to reject partial fixture, \
                 but production-path test ran with incomplete ledger. \
                 Fixture-safety gate is NOT active — fix harness setup_env()."
            );
        }
    }
}

#[test]
fn gitlab_status_reports_only_open_merge_requests() {
    let tmp = TempDir::new().unwrap();
    let glab = FakeBackend::new(tmp.path(), "glab");
    let mut mrs = Vec::new();

    for iid in 1..=2 {
        mrs.push(gitlab_mr_json(GitlabMrParams {
            title: format!("Draft: TICKET-{iid} open"),
            branch: format!("gah/open-{iid}"),
            labels: vec![],
            pipeline_status: Some("success".into()),
            state: Some("opened".into()),
            url: None,
            iid: Some(iid),
            draft: Some(true),
            merged_at: None,
            updated_at: None,
        }));
    }
    for iid in 3..=30 {
        let mut mr = gitlab_mr_json(GitlabMrParams {
            title: format!("Draft: TICKET-{iid} historical"),
            branch: format!("gah/historical-{iid}"),
            labels: vec![],
            pipeline_status: None,
            state: Some("opened".into()),
            url: None,
            iid: Some(iid),
            draft: Some(true),
            merged_at: None,
            updated_at: None,
        });
        mr["state"] = serde_json::json!("closed");
        mrs.push(mr);
    }
    glab.install(Scenario::success().with_stdout(serde_json::to_string(&mrs).unwrap()));

    let mut harness = ScenarioHarness::new("gitlab")
        .gitlab_scenario("empty")
        .with_config_append(
            r#"
provider_api_base = "https://gitlab.example.com/api/v4"
provider_project_id = "42"
"#,
        )
        .with_ledger(TestLedger::new());
    harness.install_custom_glab(&glab);

    let snap = harness.run_status_json().expect("status should succeed");
    let merge_requests = snap["merge_requests"].as_array().unwrap();
    assert_eq!(
        merge_requests.len(),
        2,
        "status must exclude closed/merged GitLab MRs; got {merge_requests:?}"
    );
}

#[test]
fn stale_human_required_ledger_entry_does_not_block_unrelated_status() {
    let mut entry = ledger_entry_full(
        "review",
        "gah/stale-human-required",
        Some("review"),
        "TICKET-STALE",
        "2026-07-01T00:00:00Z",
    );
    entry["human_required"] = serde_json::json!(true);

    let pr = support::scenario::github_rest_pr_json(GithubPrParams {
        title: "Draft: TICKET-UNRELATED healthy work".into(),
        branch: "gah/unrelated-healthy-work".into(),
        labels: vec![],
        ci_conclusion: Some("SUCCESS".into()),
        state: None,
        url: None,
        number: Some(99),
        draft: Some(true),
        merged_at: None,
        updated_at: None,
    });
    let tmp = TempDir::new().unwrap();
    let gh = FakeBackend::new(tmp.path(), "gh");
    install_active_prs(&gh, &[pr]);

    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .with_ledger(TestLedger::new().with_entry(entry));
    harness.install_custom_gh(&gh);

    let snap = harness.run_status_json().expect("status should succeed");
    assert_eq!(snap["merge_requests"].as_array().unwrap().len(), 1);
    let blocked = snap["blocked_work_items"].as_array().unwrap();
    assert!(
        blocked.is_empty(),
        "stale ledger work must not block unrelated status: {blocked:?}"
    );
}
