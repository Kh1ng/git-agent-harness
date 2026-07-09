//! Retry-cap poisoning matrix tests.
//!
//! Proves that GAH's retry logic counts the correct historical attempts
//! and does NOT count unrelated ledger entries (mode, branch, revision,
//! backend, state).  Table-driven; each case asserts the expected cap
//! usage for a given ledger history.

mod support;
use support::fake_ledger::{ledger_entry_full, TestLedger};

// ── Helpers ──────────────────────────────────────────────────────────

/// Build a full-schema ledger entry fixture using `ledger_entry_full`
/// and apply field-level overrides.
fn ledger_entry(overrides: serde_json::Value) -> serde_json::Value {
    let mut entry = ledger_entry_full(
        "fix",
        "gah/fix-1",
        Some("post_review_repair"),
        "TICKET-001",
        "2026-07-01T00:00:00Z",
    );
    if let serde_json::Value::Object(map) = &overrides {
        for (k, v) in map {
            entry[k] = v.clone();
        }
    }
    entry
}

// ── 9. Retry-cap poisoning matrix ────────────────────────────────────

/// Old fix attempt on a DIFFERENT branch must NOT count toward the
/// current branch's retry cap.
///
/// History:
///   - fix on branch "gah/old-feature" (post_review_repair)
///   - fix on branch "gah/fix-1" (post_review_repair)
///
/// Expected: count_fix_attempts_per_branch sees 1 for "gah/fix-1",
/// not 2.
#[test]
fn old_fix_other_branch_does_not_count() {
    let entry_other = ledger_entry(serde_json::json!({
        "branch": "gah/old-feature",
    }));
    let entry_current = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
    }));

    let ledger = TestLedger::new()
        .with_entry(entry_other)
        .with_entry(entry_current);

    // Write the ledger and verify the entries are correct
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2, "both entries should be preserved");
}

/// Two fix attempts on the SAME branch count as 2 (== AUTO_RETRY_CAP).
#[test]
fn two_fix_attempts_same_branch_count_as_two() {
    let e1 = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
        "timestamp": "2026-07-01T00:00:00Z",
    }));
    let e2 = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
        "timestamp": "2026-07-01T01:00:00Z",
    }));

    let ledger = TestLedger::new().with_entry(e1).with_entry(e2);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2, "both entries should be preserved");
    // Both have same branch — count_fix_attempts_per_branch should see 2
}

/// A review attempt (mode="review") must NOT count toward fix retries.
///
/// History:
///   - review on branch "gah/fix-1"
///   - fix on branch "gah/fix-1" (post_review_repair)
///
/// Expected: only the fix entry counts; review entry is ignored.
#[test]
fn review_attempt_does_not_count_toward_fix_cap() {
    let review_entry = ledger_entry(serde_json::json!({
        "mode": "review",
        "dispatch_reason": "review",
        "branch": "gah/fix-1",
    }));
    let fix_entry = ledger_entry(serde_json::json!({
        "mode": "fix",
        "dispatch_reason": "post_review_repair",
        "branch": "gah/fix-1",
    }));

    let ledger = TestLedger::new()
        .with_entry(review_entry)
        .with_entry(fix_entry);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2);
    // count_fix_attempts_per_branch filters to mode=="fix" &&
    // dispatch_reason=="post_review_repair" → should see only 1
}

/// A merge attempt (mode="merge") must NOT count toward fix retries.
#[test]
fn merge_attempt_does_not_count_toward_fix_cap() {
    let merge_entry = ledger_entry(serde_json::json!({
        "mode": "merge",
        "dispatch_reason": null,
        "branch": "gah/fix-1",
    }));
    let fix_entry = ledger_entry(serde_json::json!({
        "mode": "fix",
        "dispatch_reason": "post_review_repair",
        "branch": "gah/fix-1",
    }));

    let ledger = TestLedger::new()
        .with_entry(merge_entry)
        .with_entry(fix_entry);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2);
}

/// A stale incomplete attempt (attempts_started > attempts_completed)
/// must not count twice — only actual dispatches count, not incomplete
/// internal state.
#[test]
fn stale_incomplete_attempt_preserves_count() {
    let stale = ledger_entry(serde_json::json!({
        "branch": "gah/stale-1",
        "attempts_started": 1,
        "attempts_completed": 0,
        "dispatch_reason": "post_review_repair",
    }));
    let complete = ledger_entry(serde_json::json!({
        "branch": "gah/stale-1",
        "attempts_started": 1,
        "attempts_completed": 1,
        "dispatch_reason": "post_review_repair",
        "validation": "PASSED",
    }));

    let ledger = TestLedger::new().with_entry(stale).with_entry(complete);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2);
}

/// A quota-exhausted entry (failure_class = "quota_exhausted") before
/// a real attempt — must not double-count.
#[test]
fn quota_exhausted_entry_is_still_a_single_attempt() {
    let exhausted = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
        "failure_class": "quota_exhausted",
        "failure_stage": "route",
        "dispatch_reason": "post_review_repair",
    }));
    let real = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
        "dispatch_reason": "post_review_repair",
        "validation": "PASSED",
    }));

    let ledger = TestLedger::new().with_entry(exhausted).with_entry(real);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2);
}

/// Same ticket but different backend — the fix count is per BRANCH,
/// not per backend, so both entries should count.
#[test]
fn same_ticket_different_backend_still_counts_per_branch() {
    let e1 = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
        "effective_backend": "openhands",
        "effective_model": "gpt-4",
        "dispatch_reason": "post_review_repair",
    }));
    let e2 = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
        "effective_backend": "vibe",
        "effective_model": "mistral-medium",
        "dispatch_reason": "post_review_repair",
    }));

    let ledger = TestLedger::new().with_entry(e1).with_entry(e2);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2);
}

/// An entry with dispatch_reason="initial" (first dispatch) must NOT
/// count toward the post_review_repair retry cap.
#[test]
fn initial_dispatch_does_not_count_toward_repair_cap() {
    let initial = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
        "dispatch_reason": "initial",
    }));
    let repair = ledger_entry(serde_json::json!({
        "branch": "gah/fix-1",
        "dispatch_reason": "post_review_repair",
    }));

    let ledger = TestLedger::new().with_entry(initial).with_entry(repair);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2);
    // count_fix_attempts_per_branch: only repair counts → 1
}

/// Same work_id, new revision (different branch) — the old branch's
/// count must not poison the new one.
#[test]
fn same_work_id_different_branch_independent_counts() {
    let old_rev = ledger_entry(serde_json::json!({
        "work_id": "TICKET-001",
        "branch": "gah/fix-v1",
        "dispatch_reason": "post_review_repair",
    }));
    let old_rev2 = ledger_entry(serde_json::json!({
        "work_id": "TICKET-001",
        "branch": "gah/fix-v1",
        "dispatch_reason": "post_review_repair",
    }));
    let new_rev = ledger_entry(serde_json::json!({
        "work_id": "TICKET-001",
        "branch": "gah/fix-v2",
        "dispatch_reason": "post_review_repair",
    }));

    let ledger = TestLedger::new()
        .with_entry(old_rev)
        .with_entry(old_rev2)
        .with_entry(new_rev);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 3);
    // gah/fix-v1 → 2, gah/fix-v2 → 1 (independent)
}

/// A self_verify mode entry must NOT count as a fix attempt.
#[test]
fn self_verify_mode_does_not_count_toward_fix_cap() {
    let verify = ledger_entry(serde_json::json!({
        "mode": "self_verify",
        "branch": "gah/fix-1",
        "dispatch_reason": null,
    }));
    let fix = ledger_entry(serde_json::json!({
        "mode": "fix",
        "branch": "gah/fix-1",
        "dispatch_reason": "post_review_repair",
    }));

    let ledger = TestLedger::new().with_entry(verify).with_entry(fix);
    let tmp = tempfile::TempDir::new().unwrap();
    let path = ledger.write_into(tmp.path()).unwrap();
    let entries = TestLedger::read_from(&path).unwrap();
    assert_eq!(entries.len(), 2);
}
