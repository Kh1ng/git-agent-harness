use super::{
    create_draft_mr, draft_mr_title, find_review_target_by_mr, github_review_target_by_number,
    gitlab_target_from_value, mark_ready_for_review, merge_mr, parse_gitlab_mr_reference,
    MrReferenceError, TEST_PATH_OVERRIDE,
};
use crate::config::{Profile, RoutingPolicy};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

/// Sets the PATH override consulted by `provider_command()` for the
/// *current test thread only* (a thread-local, not `std::env::set_var`).
/// Rust runs tests in parallel threads within one process, and PATH is
/// process-global — mutating it directly corrupts unrelated tests in
/// other modules (worktree, dispatch, routing) that need the real PATH
/// for `git`/`sh` mid-run. This was tried and reproduced that exact
/// failure before being replaced with this seam.
struct PathOverride;

impl PathOverride {
    fn set(path: String) -> Self {
        TEST_PATH_OVERRIDE.with(|p| *p.borrow_mut() = Some(path));
        PathOverride
    }
}

impl Drop for PathOverride {
    fn drop(&mut self) {
        TEST_PATH_OVERRIDE.with(|p| *p.borrow_mut() = None);
    }
}

fn make_fake_bin(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

fn github_profile() -> Profile {
    Profile {
        manager_wake_autonomy: crate::config::WakeAutonomy::default(),
        prune_older_than_days: None,
        display_name: "Repo".into(),
        repo_id: "repo".into(),
        provider: "github".into(),
        repo: "owner/repo".into(),
        local_path: "/tmp/repo".into(),
        artifact_root: "/tmp/artifacts".into(),
        default_target_branch: "main".into(),
        provider_api_base: None,
        provider_project_id: None,
        oh_profile: None,
        openhands_args: vec![],
        codex_args: vec![],
        codex_path: None,
        claude_args: vec![],
        claude_path: None,
        agy_path: None,
        vibe_args: vec![],
        vibe_path: None,
        opencode_args: vec![],
        opencode_path: None,
        agy_second_home: None,
        agy_print_timeout_seconds: std::collections::HashMap::new(),
        agy_idle_timeout_seconds: None,
        opencode_idle_timeout_seconds: None,
        opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
        max_concurrent_per_model: std::collections::HashMap::new(),
        openhands_idle_timeout_seconds: None,
        vibe_idle_timeout_seconds: None,
        codex_idle_timeout_seconds: None,
        claude_idle_timeout_seconds: None,
        max_parallel_workers: None,
        max_open_managed_mrs: None,
        policy_path: None,
        env_file: None,
        env_file_prod: None,
        validation_commands: vec![],
        auto_fix_commands: vec![],
        test_file_patterns: vec![],
        known_baseline_failure_markers: vec![],
        model_improve: None,
        model_pm: None,
        model_review: None,
        review_timeout_seconds: None,
        review_hard_timeout_seconds: None,
        validation_timeout_seconds: None,
        notify_command: None,
        routing: RoutingPolicy::default(),
        pacing: Default::default(),
        publishing: Default::default(),
    }
}

fn gitlab_profile() -> Profile {
    Profile {
        prune_older_than_days: None,
        provider: "gitlab".into(),
        provider_api_base: Some("https://gitlab.example.com/api/v4".into()),
        provider_project_id: Some("42".into()),
        ..github_profile()
    }
}

#[test]
fn github_mr_missing_gh_produces_actionable_error() {
    let tmp = TempDir::new().unwrap();
    let empty_bin = tmp.path().join("bin");
    fs::create_dir_all(&empty_bin).unwrap();
    // PATH deliberately has no fallback to the real system PATH: this
    // must fail even on a machine where `gh` happens to be installed.
    let _guard = PathOverride::set(empty_bin.to_str().unwrap().to_string());

    let err = create_draft_mr(&github_profile(), "gah/test", "title", "body").unwrap_err();

    assert!(format!("{:#}", err).contains("gh pr create"));
}

#[test]
fn github_mr_nonzero_exit_surfaces_stderr() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        "#!/bin/sh\necho 'insufficient scope' >&2\nexit 1\n",
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = create_draft_mr(&github_profile(), "gah/test", "title", "body").unwrap_err();

    let msg = format!("{:#}", err);
    assert!(msg.contains("gh pr create failed"));
    assert!(msg.contains("insufficient scope"));
}

#[test]
fn github_mr_body_is_redacted_before_it_reaches_provider_cli() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let args_path = bin_dir.join("gh-args.txt");
    make_fake_bin(
            &bin_dir,
            "gh",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho 'https://github.test/owner/repo/pull/1'\n",
                args_path.display()
            ),
        );
    let _guard = PathOverride::set(bin_dir.to_string_lossy().into_owned());

    create_draft_mr(
        &github_profile(),
        "gah/test",
        "title",
        "summary Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
    )
    .unwrap();

    let args = fs::read_to_string(args_path).unwrap();
    assert!(!args.contains("abcdefghijklmnopqrstuvwxyz"));
    assert!(args.contains("[REDACTED:TOKEN]"));
}

#[test]
fn gitlab_mr_error_json_response_fails_closed() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
            &bin_dir,
            "glab",
            "#!/bin/sh\nprintf '%s\\n' '{\"message\":\"404 Project Not Found\",\"token\":\"glpat-abcdefghijklmnopqrstuvwxyz\"}'\necho 'glab: API request failed: 404' >&2\nexit 1\n",
        );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap_err();

    let msg = format!("{:#}", err);
    assert!(msg.contains("glab api gitlab create mr failed"));
    assert!(msg.contains("404 Project Not Found"));
    assert!(!msg.contains("glpat-abcdefghijklmnopqrstuvwxyz"));
    assert!(msg.contains("[REDACTED:GITLAB_TOKEN]"));
}

#[test]
fn gitlab_mr_missing_required_fields_fails_closed() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
            &bin_dir,
            "glab",
            "#!/bin/sh\nprintf '%s\\n' '{\"web_url\":\"https://gitlab.example.com/group/repo/-/merge_requests/42\"}'\nexit 0\n",
        );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap_err();

    let msg = format!("{:#}", err);
    assert!(msg.contains("invalid merge request payload"));
    assert!(msg.contains("web_url"));
    assert!(msg.contains("merge_requests/42"));
}

#[test]
fn gitlab_mr_empty_web_url_fails_closed() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        "#!/bin/sh\nprintf '%s\\n' '{\"iid\":42,\"web_url\":\"\"}'\nexit 0\n",
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap_err();

    let msg = format!("{:#}", err);
    assert!(msg.contains("invalid merge request payload"));
    assert!(msg.contains("\"iid\":42"));
}

#[test]
fn gitlab_mr_valid_response_returns_id_and_url() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let args_path = bin_dir.join("glab-args.txt");
    make_fake_bin(
            &bin_dir,
            "glab",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{{\"iid\":42,\"web_url\":\"https://gitlab.example.com/group/repo/-/merge_requests/42\"}}'\nexit 0\n",
                args_path.display()
            ),
        );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let mr = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap();

    assert_eq!(mr.id, "42");
    assert_eq!(
        mr.url,
        "https://gitlab.example.com/group/repo/-/merge_requests/42"
    );
    let args = fs::read_to_string(args_path).unwrap();
    assert!(args.contains("projects/42/merge_requests"));
    assert!(args.contains("gitlab.example.com"));
    assert!(args.contains("source_branch=gah/test"));
    assert!(args.contains("target_branch=main"));
    assert!(!args.contains("PRIVATE-TOKEN"));
}

#[test]
fn provider_draft_title_is_capped_after_prefix() {
    let title = draft_mr_title(&"é".repeat(300));

    assert_eq!(title.chars().count(), 255);
    assert!(title.starts_with("Draft: "));
    assert!(title.ends_with("..."));
}

#[test]
fn gitlab_review_target_errors_loudly_on_api_error_response() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    // Even a successful provider process must fail closed when its body is
    // not an MR. This protects the fetch refspec from empty branch fields.
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        "#!/bin/sh\necho '{\"message\":\"404 Project Not Found\"}'\nexit 0\n",
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = find_review_target_by_mr(&gitlab_profile(), "235").unwrap_err();

    assert!(format!("{:#}", err).contains("did not return a merge request"));
}

#[test]
fn gitlab_review_target_uses_authenticated_glab_session_and_surfaces_api_failure() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let args_path = bin_dir.join("glab-args.txt");
    make_fake_bin(
        &bin_dir,
        "glab",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho '401 Unauthorized' >&2\nexit 1\n",
            args_path.display()
        ),
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = find_review_target_by_mr(&gitlab_profile(), "235").unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("401 Unauthorized"));
    let args = fs::read_to_string(args_path).unwrap();
    assert!(args.contains("--hostname\ngitlab.example.com"));
    assert!(!args.contains("PRIVATE-TOKEN"));
}

#[test]
fn gitlab_mr_reference_accepts_bare_iid() {
    let iid = parse_gitlab_mr_reference(&gitlab_profile(), "284").unwrap();
    assert_eq!(iid, "284");
}

#[test]
fn gitlab_mr_reference_normalizes_canonical_url_to_iid() {
    let iid = parse_gitlab_mr_reference(
        &gitlab_profile(),
        "https://gitlab.example.com/owner/repo/-/merge_requests/284",
    )
    .unwrap();
    assert_eq!(iid, "284");

    let iid_with_diffs = parse_gitlab_mr_reference(
        &gitlab_profile(),
        "https://gitlab.example.com/owner/repo/-/merge_requests/284/diffs?nav=1#note_42",
    )
    .unwrap();
    assert_eq!(iid_with_diffs, "284");

    let mut local_http = gitlab_profile();
    local_http.provider_api_base = Some("http://gitlab.example.com/api/v4".into());
    let local_iid = parse_gitlab_mr_reference(
        &local_http,
        "http://gitlab.example.com/owner/repo/-/merge_requests/284",
    )
    .unwrap();
    assert_eq!(local_iid, "284");
}

#[test]
fn gitlab_mr_reference_rejects_malformed_url() {
    let err = parse_gitlab_mr_reference(&gitlab_profile(), "http://gitlab.example.com/owner/repo")
        .unwrap_err();
    assert!(matches!(err, MrReferenceError::MalformedUrl(_)));
    assert!(format!("{err}").contains("malformed --mr value"));

    let err_suffix = parse_gitlab_mr_reference(
        &gitlab_profile(),
        "https://gitlab.example.com/owner/repo/-/merge_requests/284invalid",
    )
    .unwrap_err();
    assert!(matches!(err_suffix, MrReferenceError::MalformedUrl(_)));

    let err_noncanonical = parse_gitlab_mr_reference(
        &gitlab_profile(),
        "https://gitlab.example.com/owner/repo/merge_requests/284",
    )
    .unwrap_err();
    assert!(matches!(
        err_noncanonical,
        MrReferenceError::MalformedUrl(_)
    ));

    let err_credentials = parse_gitlab_mr_reference(
        &gitlab_profile(),
        "https://user@gitlab.example.com/owner/repo/-/merge_requests/284",
    )
    .unwrap_err();
    assert!(matches!(err_credentials, MrReferenceError::MalformedUrl(_)));
}

#[test]
fn gitlab_mr_reference_rejects_wrong_project_url() {
    let err = parse_gitlab_mr_reference(
        &gitlab_profile(),
        "https://gitlab.example.com/other/repo/-/merge_requests/284",
    )
    .unwrap_err();
    assert!(matches!(err, MrReferenceError::CrossProject { .. }));
    assert!(format!("{err}").contains("does not match this profile's project"));
}

#[test]
fn gitlab_mr_reference_rejects_wrong_host_url() {
    let err = parse_gitlab_mr_reference(
        &gitlab_profile(),
        "https://attacker.example.com/owner/repo/-/merge_requests/284",
    )
    .unwrap_err();
    assert!(matches!(err, MrReferenceError::CrossProject { .. }));

    let mut profile_no_base = gitlab_profile();
    profile_no_base.provider_api_base = None;
    let err_no_base = parse_gitlab_mr_reference(
        &profile_no_base,
        "https://attacker.example.com/owner/repo/-/merge_requests/284",
    )
    .unwrap_err();
    assert!(matches!(err_no_base, MrReferenceError::InvalidProfile(_)));
}

#[test]
fn gitlab_mr_reference_rejects_invalid_profile_configuration() {
    let mut missing_base = gitlab_profile();
    missing_base.provider_api_base = None;
    let missing = parse_gitlab_mr_reference(
        &missing_base,
        "https://gitlab.example.com/owner/repo/-/merge_requests/284",
    )
    .unwrap_err();
    assert!(matches!(missing, MrReferenceError::InvalidProfile(_)));

    let mut malformed_base = gitlab_profile();
    malformed_base.provider_api_base = Some("not-a-url".into());
    let malformed = parse_gitlab_mr_reference(
        &malformed_base,
        "https://gitlab.example.com/owner/repo/-/merge_requests/284",
    )
    .unwrap_err();
    assert!(matches!(malformed, MrReferenceError::InvalidProfile(_)));
}

#[test]
fn gitlab_dispatch_accepts_bare_iid_and_canonical_url() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let args_path = bin_dir.join("glab-args.txt");
    make_fake_bin(
        &bin_dir,
        "glab",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nprintf '%s\\n' '{{\"iid\":284,\"web_url\":\"https://gitlab.example.com/owner/repo/-/merge_requests/284\",\"source_branch\":\"gah/284\",\"target_branch\":\"main\"}}'\nexit 0\n",
            args_path.display()
        ),
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let target1 = find_review_target_by_mr(&gitlab_profile(), "284").unwrap();
    assert_eq!(target1.id, "284");
    assert_eq!(target1.source_branch, "gah/284");

    let target2 = find_review_target_by_mr(
        &gitlab_profile(),
        "https://gitlab.example.com/owner/repo/-/merge_requests/284",
    )
    .unwrap();
    assert_eq!(target2.id, "284");
    assert_eq!(target2.source_branch, "gah/284");

    assert_eq!(target1, target2);

    let args = fs::read_to_string(args_path).unwrap();
    assert!(args.contains("projects/42/merge_requests/284"));
}

#[test]
fn gitlab_dispatch_by_malformed_mr_url_fails_before_any_provider_launch() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    // Deliberately no `glab` binary in PATH: a malformed/cross-project `--mr`
    // must fail at preflight validation without ever launching the provider CLI.
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = find_review_target_by_mr(&gitlab_profile(), "not-a-valid-mr-reference").unwrap_err();

    assert!(format!("{:#}", err).contains("malformed --mr value"));

    let cross_project = find_review_target_by_mr(
        &gitlab_profile(),
        "https://gitlab.example.com/other/repo/-/merge_requests/284",
    )
    .unwrap_err();
    assert!(format!("{cross_project:#}").contains("does not match this profile's project"));
}

#[test]
fn github_dispatch_accepts_bare_number_and_canonical_url() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let args_path = bin_dir.join("gh-args.txt");
    make_fake_bin(
        &bin_dir,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nprintf '%s\\n' '{{\"number\":7,\"url\":\"https://github.test/owner/repo/pull/7\",\"headRefName\":\"gah/7\",\"baseRefName\":\"main\"}}'\n",
            args_path.display()
        ),
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    find_review_target_by_mr(&github_profile(), "7").unwrap();
    find_review_target_by_mr(&github_profile(), "https://github.com/owner/repo/pull/7").unwrap();

    let args = fs::read_to_string(args_path).unwrap();
    assert!(args.contains("pr\nview\n7\n"));
    assert!(args.contains("pr\nview\nhttps://github.com/owner/repo/pull/7\n"));
}

#[test]
fn review_targets_capture_provider_source_and_target_shas() {
    let gitlab = serde_json::json!({
        "iid": 42,
        "web_url": "https://gitlab.test/group/repo/-/merge_requests/42",
        "source_branch": "gah/42",
        "target_branch": "main",
        "detailed_merge_status": "draft",
        "sha": "source-gitlab-sha",
        "diff_refs": { "base_sha": "target-gitlab-sha" }
    });
    let gitlab_target = gitlab_target_from_value(&gitlab).unwrap();
    assert_eq!(gitlab_target.ci_status, None);
    assert_eq!(gitlab_target.merge_status.as_deref(), Some("draft"));
    assert_eq!(
        gitlab_target.source_sha.as_deref(),
        Some("source-gitlab-sha")
    );
    assert_eq!(
        gitlab_target.target_sha.as_deref(),
        Some("target-gitlab-sha")
    );

    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
            &bin_dir,
            "gh",
            "#!/bin/sh\nprintf '%s\\n' '{\"number\":7,\"url\":\"https://github.test/owner/repo/pull/7\",\"headRefName\":\"gah/7\",\"baseRefName\":\"main\",\"headRefOid\":\"source-github-sha\",\"statusCheckRollup\":[]}'\n",
        );
    let _guard = PathOverride::set(bin_dir.to_string_lossy().into_owned());
    let github_target = github_review_target_by_number(&github_profile(), "7").unwrap();
    assert_eq!(
        github_target.source_sha.as_deref(),
        Some("source-github-sha")
    );
    assert_eq!(github_target.target_sha, None);
}

#[test]
fn find_review_target_by_mr_prefers_pipeline_status_for_ci_status_on_draft_mr() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        "#!/bin/sh\ncase \"$1 $2\" in\n  \"api projects/42/merge_requests/235\") printf '{\"iid\":235,\"web_url\":\"https://gitlab.test/group/repo/-/merge_requests/235\",\"source_branch\":\"gah/235\",\"target_branch\":\"main\",\"sha\":\"pipeline-source-sha\",\"detailed_merge_status\":\"draft\",\"head_pipeline\":{\"sha\":\"pipeline-source-sha\",\"status\":\"success\"}}\\n' ;;\n  *) echo \"unexpected glab invocation: $@\" >&2; exit 1 ;;\nesac\n",
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let target = find_review_target_by_mr(&gitlab_profile(), "235").unwrap();

    assert_eq!(target.ci_status.as_deref(), Some("success"));
    assert_eq!(target.merge_status.as_deref(), Some("draft"));
}

#[test]
fn find_review_target_by_mr_ignores_stale_pipeline_status_not_matching_source_sha() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        "#!/bin/sh\ncase \"$1 $2\" in\n  \"api projects/42/merge_requests/235\") printf '{\"iid\":235,\"web_url\":\"https://gitlab.test/group/repo/-/merge_requests/235\",\"source_branch\":\"gah/235\",\"target_branch\":\"main\",\"sha\":\"current-sha\",\"detailed_merge_status\":\"can_be_merged\",\"head_pipeline\":{\"sha\":\"old-sha\",\"status\":\"success\"}}\\n' ;;\n  \"api projects/42/merge_requests/235/pipelines\") printf '[{\"sha\":\"old-sha\",\"status\":\"success\"}]\\n' ;;\n  *) echo \"unexpected glab invocation: $@\" >&2; exit 1 ;;\nesac\n",
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let target = find_review_target_by_mr(&gitlab_profile(), "235").unwrap();

    assert_eq!(target.ci_status.as_deref(), Some("missing"));
    assert_eq!(target.merge_status.as_deref(), Some("can_be_merged"));
}

#[test]
fn merge_mr_github_un_drafts_then_merges() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    // Handles the subcommand shapes merge_mr's call chain hits, in
    // order: REST pulls lookup (resolve branch -> number), `pr view` (resolve
    // full ReviewTarget), `pr ready` (un-draft), `pr merge`. Fails
    // loudly (unrecognized args) instead of a lenient default if a
    // future edit changes the call sequence without updating this test.
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/repo/pulls") printf '[{"number":42}]\n' ;;
  "pr view 42 --repo") printf '{"number":42,"url":"https://github.com/owner/repo/pull/42","title":"test","body":"body","isDraft":false,"headRefName":"gah/test","baseRefName":"main","headRefOid":"source-github-sha"}\n' ;;
  "pr ready 42 --repo") exit 0 ;;
  "pr merge 42 --squash") echo "$@" > "${0%/*}/merge_call.txt"; exit 0 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    merge_mr(&github_profile(), "gah/test", None).unwrap();

    let call = fs::read_to_string(bin_dir.join("merge_call.txt")).unwrap();
    assert!(call.contains("42"));
    assert!(call.contains("--squash"));
    assert!(call.contains("--delete-branch"));
    // Regression (live on PR #255): without --admin, `gh pr merge` is
    // rejected by branch protection's required-approving-review count,
    // which gah's own issue-comment review verdict can never satisfy.
    assert!(call.contains("--admin"));
    assert!(call.contains("--match-head-commit source-github-sha"));
}

#[test]
fn merge_mr_rejects_a_generation_changed_after_review() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/repo/pulls") printf '[{"number":42}]\n' ;;
  "pr view 42 --repo") printf '{"number":42,"url":"https://github.com/owner/repo/pull/42","title":"changed","body":"body","isDraft":false,"headRefName":"gah/test","baseRefName":"main","headRefOid":"new-source-sha"}\n' ;;
  "pr ready 42 --repo") echo "unexpected ready" > "${0%/*}/mutated.txt"; exit 0 ;;
  "pr merge 42 --squash") echo "unexpected merge" > "${0%/*}/mutated.txt"; exit 0 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let error = merge_mr(
        &github_profile(),
        "gah/test",
        Some("review-v1:old-source:sha256:old-metadata"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("changed after review"));
    assert!(!bin_dir.join("mutated.txt").exists());
}

#[test]
fn mark_ready_for_review_github_un_drafts_only() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/repo/pulls") printf '[{"number":42}]\n' ;;
  "pr view 42 --repo") printf '{"number":42,"url":"https://github.com/owner/repo/pull/42","headRefName":"gah/test","baseRefName":"main"}\n' ;;
  "pr ready 42 --repo") echo "$@" > "${0%/*}/ready_call.txt"; exit 0 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    mark_ready_for_review(&github_profile(), "gah/test").unwrap();

    let call = fs::read_to_string(bin_dir.join("ready_call.txt")).unwrap();
    assert!(call.contains("42"));
    assert!(!call.contains("merge"));
}

#[test]
fn merge_mr_github_fails_loudly_when_merge_rejected() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/repo/pulls") printf '[{"number":42}]\n' ;;
  "pr view 42 --repo") printf '{"number":42,"url":"https://github.com/owner/repo/pull/42","headRefName":"gah/test","baseRefName":"main"}\n' ;;
  "pr ready 42 --repo") exit 0 ;;
  "pr merge 42 --squash") echo "not mergeable: review required" >&2; exit 1 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = merge_mr(&github_profile(), "gah/test", None).unwrap_err();

    assert!(format!("{:#}", err).contains("not mergeable"));
}

#[test]
fn merge_mr_gitlab_un_drafts_then_merges() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        r#"#!/bin/sh
case "$1 $2" in
  "api projects/42/merge_requests") printf '[{"iid":7,"web_url":"https://gitlab.example.com/x/-/merge_requests/7","source_branch":"gah/test","target_branch":"main","title":"test","description":"body","draft":false,"sha":"source-gitlab-sha"}]\n' ;;
  "mr update") exit 0 ;;
  "mr merge") echo "$@" > "${0%/*}/merge_call.txt"; exit 0 ;;
  *) echo "unexpected glab invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    merge_mr(&gitlab_profile(), "gah/test", None).unwrap();

    let call = fs::read_to_string(bin_dir.join("merge_call.txt")).unwrap();
    assert!(call.contains(" 7 "));
    assert!(call.contains("--squash"));
    assert!(call.contains("--remove-source-branch"));
    assert!(call.contains("--sha source-gitlab-sha"));
}

#[test]
fn mark_ready_for_review_gitlab_un_drafts_only() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        r#"#!/bin/sh
case "$1 $2" in
  "api projects/42/merge_requests") printf '[{"iid":7,"web_url":"https://gitlab.example.com/x/-/merge_requests/7","source_branch":"gah/test","target_branch":"main"}]\n' ;;
  "mr update") echo "$@" > "${0%/*}/ready_call.txt"; exit 0 ;;
  *) echo "unexpected glab invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    mark_ready_for_review(&gitlab_profile(), "gah/test").unwrap();

    let call = fs::read_to_string(bin_dir.join("ready_call.txt")).unwrap();
    assert!(call.contains(" 7 "));
    assert!(call.contains("--ready"));
}

#[test]
fn github_post_review_comment_applies_labels_via_rest_api_not_pr_edit() {
    // Regression: `gh pr edit --add-label` fails every time on real repos
    // with a "Projects (classic)" GraphQL error, and the old code
    // silently swallowed that failure so labels never got applied. This
    // pins that we now hit the REST labels endpoint instead, and that
    // `gh pr edit` is never invoked for this path.
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/repo/pulls") printf '[{"number":42}]\n' ;;
  "api repos/owner/repo/issues/42/labels --jq "*) printf 'gah-needs-fix\nunrelated\n' ;;
  "api --method GET repos/owner/repo/issues/42/comments") printf '[]\n' ;;
  "api --method POST repos/owner/repo/issues/42/comments") echo "$@" >> "${0%/*}/comment_call.txt" ;;
  "api "*) echo "$@" >> "${0%/*}/label_call.txt" ;;
  "pr "*) echo "gh pr commands must not be called: $@" >&2; exit 1 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
exit 0
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    super::post_review_comment(
        &github_profile(),
        "gah/test",
        "review body",
        &["gah-ready-for-human"],
    )
    .unwrap();

    let call = fs::read_to_string(bin_dir.join("label_call.txt")).unwrap();
    assert!(call.contains("repos/owner/repo/issues/42/labels"));
    assert!(call.contains("--method DELETE"));
    assert!(call.contains("gah-needs-fix"));
    assert!(call.contains("labels[]=gah-ready-for-human"));
    let comment_call = fs::read_to_string(bin_dir.join("comment_call.txt")).unwrap();
    assert!(comment_call.contains("--method POST"));
    assert!(comment_call.contains("body=review body"));
}

#[test]
fn gitlab_source_issue_comment_is_idempotent() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        r#"#!/bin/sh
case "$*" in
  *"--method GET"*)
    if [ -f "$0.accepted" ]; then
      printf '[{"body":"already satisfied"}]\n'
    else
      printf '[]\n'
    fi
    ;;
  *"--method POST"*)
    : > "$0.accepted"
    echo post >> "$0.posts"
    printf '{}\n'
    ;;
  *) echo "unexpected glab invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    super::post_issue_comment(&gitlab_profile(), "42", "already satisfied").unwrap();
    super::post_issue_comment(&gitlab_profile(), "42", "already satisfied").unwrap();

    assert_eq!(
        fs::read_to_string(bin_dir.join("glab.posts")).unwrap(),
        "post\n"
    );
}

#[test]
fn github_repaired_state_removes_needs_fix_and_preserves_unrelated_labels() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/repo/pulls") printf '[{"number":42}]\n' ;;
  "api repos/owner/repo/issues/42/labels --jq "*) printf 'gah-needs-fix\nbug\n' ;;
  "api "*) echo "$@" >> "${0%/*}/calls.txt" ;;
  *) exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    super::set_review_state_labels(&github_profile(), "gah/test", &["gah-review-escalating"])
        .unwrap();

    let calls = fs::read_to_string(bin_dir.join("calls.txt")).unwrap();
    assert!(calls.contains("--method DELETE"));
    assert!(calls.contains("gah-needs-fix"));
    assert!(calls.contains("labels[]=gah-review-escalating"));
    assert!(
        !calls.contains("/bug"),
        "unrelated labels must be preserved"
    );
}

#[test]
fn gitlab_review_state_adds_desired_and_removes_stale_labels() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        r#"#!/bin/sh
case "$1 $2" in
  "api projects/42/merge_requests")
    printf '[{"iid":7,"web_url":"https://gitlab.example/x/7","source_branch":"gah/test","target_branch":"main"}]\n'
    ;;
  "api projects/42/merge_requests/7")
    echo "$@" > "${0%/*}/label_call.txt"
    printf '{}\n'
    ;;
  *) echo "unexpected glab invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    super::set_review_state_labels(&gitlab_profile(), "gah/test", &["gah-review-escalating"])
        .unwrap();

    let call = fs::read_to_string(bin_dir.join("label_call.txt")).unwrap();
    assert!(call.contains("add_labels"));
    assert!(call.contains("gah-review-escalating"));
    assert!(call.contains("remove_labels"));
    assert!(call.contains("gah-needs-fix"));
    assert!(!call.contains("PRIVATE-TOKEN"));
}

#[test]
fn github_post_review_comment_reports_label_apply_failure() {
    // A posted comment without its controller label is not a successful
    // review. Surface the failure so the controller can bound retries and
    // escalate instead of repeatedly treating the PR as unreviewed.
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1" in
  api)
    if [ "$2 $3 $4" = "--method GET repos/owner/repo/pulls" ]; then
      printf '[{"number":42}]\n'
      exit 0
    fi
    if [ "$2 $3 $4" = "--method GET repos/owner/repo/issues/42/comments" ]; then
      printf '[]\n'
      exit 0
    fi
    if [ "$2 $3 $4" = "--method POST repos/owner/repo/issues/42/comments" ]; then
      exit 0
    fi
    echo "label write denied" >&2
    exit 1
    ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = super::post_review_comment(
        &github_profile(),
        "gah/test",
        "review body",
        &["gah-ready-for-human"],
    )
    .unwrap_err();
    assert!(err.to_string().contains("applying review labels"));
}

#[test]
fn github_pr_lookup_retries_a_transient_tls_failure_and_uses_rest() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
count_file="${0%/*}/count"
count=0
[ -f "$count_file" ] && read -r count < "$count_file"
count=$((count + 1))
printf '%s' "$count" > "$count_file"
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/repo/pulls")
    if [ "$count" -eq 1 ]; then
      echo 'net/http: TLS handshake timeout' >&2
      exit 1
    fi
    printf '[{"number":42}]\n'
    ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let number = super::github_find_pr_number_by_branch(&github_profile(), "gah/test").unwrap();

    assert_eq!(number, "42");
    assert_eq!(fs::read_to_string(bin_dir.join("count")).unwrap(), "2");
}

#[test]
fn github_comment_retry_detects_a_timed_out_post_that_was_already_accepted() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/repo/pulls") printf '[{"number":42}]\n' ;;
  "api repos/owner/repo/issues/42/labels --jq "*) printf 'gah-review-escalating\n' ;;
  "api --method GET repos/owner/repo/issues/42/comments")
    if [ -f "${0%/*}/accepted" ]; then
      printf '[{"body":"review body"}]\n'
    else
      printf '[]\n'
    fi
    ;;
  "api --method POST repos/owner/repo/issues/42/comments")
    : > "${0%/*}/accepted"
    echo post >> "${0%/*}/post_calls"
    echo 'net/http: TLS handshake timeout' >&2
    exit 1
    ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    super::post_review_comment(
        &github_profile(),
        "gah/test",
        "review body",
        &["gah-review-escalating"],
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(bin_dir.join("post_calls")).unwrap(),
        "post\n",
        "the retry must observe the accepted comment instead of duplicating it"
    );
}

#[test]
fn github_review_label_read_retries_a_transient_tls_failure() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3" in
  "api repos/owner/repo/issues/42/labels --jq")
    count=0
    [ -f "${0%/*}/reads" ] && read -r count < "${0%/*}/reads"
    count=$((count + 1))
    printf '%s' "$count" > "${0%/*}/reads"
    if [ "$count" -eq 1 ]; then
      echo 'net/http: TLS handshake timeout' >&2
      exit 1
    fi
    ;;
  "api repos/owner/repo/issues/42/labels -f") echo "$@" > "${0%/*}/add_call" ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    super::github_set_review_state_labels(&github_profile(), "42", &["gah-review-escalating"])
        .unwrap();

    assert_eq!(fs::read_to_string(bin_dir.join("reads")).unwrap(), "2");
    assert!(fs::read_to_string(bin_dir.join("add_call"))
        .unwrap()
        .contains("labels[]=gah-review-escalating"));
}
