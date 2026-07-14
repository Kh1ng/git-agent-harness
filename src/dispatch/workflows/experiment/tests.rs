use super::*;
use crate::dispatch::test_util::profile;

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
