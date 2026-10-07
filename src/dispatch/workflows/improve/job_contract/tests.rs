use super::*;

#[test]
fn bullet_rules() {
    assert_eq!(
        section(
            "## Allowed files\n- `src/a.rs` (note)\n* src/b.rs (note)\n+ c",
            "Allowed files",
            "job.md"
        )
        .unwrap()
        .unwrap(),
        vec!["src/a.rs", "src/b.rs", "c"]
    );
    for body in [
        "## Verification commands",
        "## Verification commands\nparagraph",
        "## Verification commands\n- ``",
        "## Verification commands\n- ",
    ] {
        let error = section(body, "Verification commands", "job.md")
            .unwrap_err()
            .to_string();
        assert!(error.contains("job.md") && error.contains("Verification commands"));
    }
    assert!(section("## Other\n- a", "Allowed files", "job.md")
        .unwrap()
        .is_none());
}

#[test]
fn glob_rules() {
    for (pattern, path) in [
        ("a", "a"),
        ("dir/", "dir/a/b"),
        ("dir/**", "dir/a"),
        ("src/*.rs", "src/a.rs"),
        ("a*b*c", "axybzc"),
    ] {
        assert!(matches_path(pattern, path));
    }
    for (pattern, path) in [
        ("a", "b"),
        ("dir/", "directory/a"),
        ("src/*.rs", "src/nested/a.rs"),
        ("*", "a/b"),
    ] {
        assert!(!matches_path(pattern, path));
    }
}

#[test]
fn constructor_rejects_paths_and_ignores_provider_issues() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("job.md");
    for path in ["/absolute", "../escape", "a/../b", "C:\\absolute"] {
        std::fs::write(&file, format!("## Allowed files\n- `{path}`")).unwrap();
        assert!(JobContract::load(file.to_str().unwrap(), false).is_err());
        assert!(JobContract::load(file.to_str().unwrap(), true)
            .unwrap()
            .is_none());
    }
    std::fs::write(&file, "## Verification commands\n- false").unwrap();
    assert!(JobContract::load(file.to_str().unwrap(), true)
        .unwrap()
        .is_none());
    std::fs::write(&file, "# Ordinary ticket").unwrap();
    assert!(JobContract::load(file.to_str().unwrap(), false)
        .unwrap()
        .is_none());
}

fn dispatch_args(target: &str, dispatch_reason: Option<&str>) -> crate::dispatch::DispatchArgs {
    crate::dispatch::DispatchArgs {
        profile: "test".into(),
        mode: "fix".into(),
        backend: "codex".into(),
        target: target.into(),
        branch: None,
        mr: None,
        current_branch: false,
        dry_run: false,
        oh_profile: None,
        model: None,
        retries: 0,
        allow_draft_fail: false,
        prod: false,
        issue_intake_override: false,
        allow_unknown_red_baseline: false,
        escalate: false,
        existing_branch: None,
        expected_review_generation: None,
        skip_validation_gate: false,
        dispatch_reason: dispatch_reason.map(Into::into),
        prior_attempt_context: None,
        work_id: None,
        run_id: None,
        route_admission: None,
    }
}

/// A ticket file the loop selected is not a job the operator handed over.
#[test]
fn only_a_direct_dispatch_gets_a_contract() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("TICKET-1-test.md");
    std::fs::write(
        &file,
        "# TICKET-1: Test\n\n## Verification Commands\n- `false`\n",
    )
    .unwrap();
    let target = file.to_str().unwrap();
    assert!(for_dispatch(&dispatch_args(target, None), false)
        .unwrap()
        .is_some());
    for reason in ["initial", "retry", "escalate", "post_review_repair"] {
        assert!(
            for_dispatch(&dispatch_args(target, Some(reason)), false)
                .unwrap()
                .is_none(),
            "{reason}"
        );
    }
    assert!(for_dispatch(&dispatch_args(target, None), true)
        .unwrap()
        .is_none());
}

#[test]
fn changed_paths_include_committed_and_pending_rename_sides() {
    let temp = tempfile::tempdir().unwrap();
    let wt = temp.path();
    let git = |args: &[&str]| {
        let result = worktree::git_raw(args, wt).unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    for file in ["old name", "pending", "unstaged"] {
        std::fs::write(wt.join(file), file).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-m", "base"]);
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&["mv", "old name", "new name"]);
    git(&["commit", "-m", "rename"]);
    git(&["mv", "pending", "staged"]);
    std::fs::write(wt.join("unstaged"), "modified").unwrap();
    std::fs::create_dir(wt.join("untracked")).unwrap();
    std::fs::write(wt.join("untracked/file"), "new").unwrap();
    assert_eq!(
        worktree::contract_changed_files(wt, "main").unwrap(),
        vec![
            "new name",
            "old name",
            "pending",
            "staged",
            "unstaged",
            "untracked/file"
        ]
    );
}
