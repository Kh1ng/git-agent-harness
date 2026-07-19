use super::*;

#[test]
fn review_gitlab_posts_comment_and_keeps_ci_separate_from_mergeability() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "gitlab",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    let glab_log = tmp.path().join("glab.log");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"],\"evidence\":[\"file:src.txt\"]}\nEOF\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\ncase \"$1 $2\" in\n  \"api projects/42/merge_requests\")\n    printf '%s\\n' '[{{\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"iid\":7,\"source_branch\":\"feature/review\",\"target_branch\":\"main\",\"draft\":true,\"sha\":\"review-source-sha\",\"detailed_merge_status\":\"draft\",\"head_pipeline\":{{\"sha\":\"review-source-sha\",\"status\":\"success\"}}}}]'\n    ;;\n  \"api projects/42/merge_requests/7/notes\")\n    printf '%s\\n' '{{\"id\":1}}'\n    ;;\n  \"api projects/42/merge_requests/7\")\n    printf '%s\\n' '{{\"iid\":7,\"source_branch\":\"feature/review\",\"target_branch\":\"main\"}}'\n    ;;\n  *) echo \"unexpected glab invocation: $*\" >&2; exit 1 ;;\n esac\n",
            glab_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "feature/review",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env_remove("GITLAB_PAT")
        .env_remove("GITLAB_PAT2")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Resolved MR: https://gitlab.example.com/owner/real/-/merge_requests/7",
        ));

    let glab_log = fs::read_to_string(glab_log).unwrap();
    assert!(glab_log.contains("source_branch=feature/review"));
    assert!(glab_log.contains("projects/42/merge_requests/7/notes"));
    assert!(glab_log.contains("add_labels=gah-ready-for-human"));
    assert!(!glab_log.contains("PRIVATE-TOKEN"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let mr_description =
        fs::read_to_string(session_dir.join("review-bundle/mr-description.md")).unwrap();
    assert!(mr_description.contains("Draft: true"));
    assert!(mr_description.contains("CI: passed"));
    assert!(mr_description.contains("Mergeability: draft"));
}
