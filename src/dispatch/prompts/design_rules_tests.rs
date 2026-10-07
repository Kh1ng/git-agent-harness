use super::build_task;
use crate::config::tests::test_profile_for_notifications;
use std::fs;

#[test]
fn design_rules_reach_implementation_and_fix_prompts_only() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("docs")).unwrap();
    fs::write(
        tmp.path().join("docs/PROJECT_BRIEF.md"),
        "## Working rules\n- Preserve unknown telemetry as unknown.\n\n## Design rules\n- Prefer deep modules.\n\n## Verification\n- cargo test\n",
    )
    .unwrap();
    let mut prof = test_profile_for_notifications();
    prof.local_path = tmp.path().display().to_string();
    let wt = tmp.path().join("worktree");
    fs::create_dir_all(&wt).unwrap();

    for mode in ["improve", "fix"] {
        let task = build_task(&prof, &wt, mode, "#42", None);
        let working = task.find("### Working rules").unwrap();
        let design = task.find("### Design rules").unwrap();
        let verification = task.find("### Verification").unwrap();
        assert!(working < design && design < verification);
        assert!(task.contains("Prefer deep modules."));
    }

    for mode in ["research", "audit", "estimate", "experiment"] {
        let task = build_task(&prof, &wt, mode, "#42", None);
        assert!(
            !task.contains("### Design rules"),
            "{mode} prompt must not carry the design rules:\n{task}"
        );
        assert!(task.contains("### Working rules"));
        assert!(!task.contains("Prefer deep modules."));
    }
}
