use super::*;
use crate::config::RoutingPolicy;
use crate::dispatch::test_util::{gah_config, profile};
use std::fs;
use std::path::Path;

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
