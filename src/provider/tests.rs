use super::{
    create_draft_mr, find_review_target_by_mr, github_review_target_by_number,
    gitlab_target_from_value, mark_ready_for_review, merge_mr, TEST_PATH_OVERRIDE,
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
            "curl",
            "#!/bin/sh\nprintf '%s\\n' '{\"message\":\"404 Project Not Found\",\"token\":\"glpat-abcdefghijklmnopqrstuvwxyz\"}'\necho 'curl: (22) The requested URL returned error: 404' >&2\nexit 22\n",
        );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap_err();

    let msg = format!("{:#}", err);
    assert!(msg.contains("curl gitlab create mr failed"));
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
            "curl",
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
        "curl",
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
    make_fake_bin(
            &bin_dir,
            "curl",
            "#!/bin/sh\nprintf '%s\\n' '{\"iid\":42,\"web_url\":\"https://gitlab.example.com/group/repo/-/merge_requests/42\"}'\nexit 0\n",
        );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let mr = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap();

    assert_eq!(mr.id, "42");
    assert_eq!(
        mr.url,
        "https://gitlab.example.com/group/repo/-/merge_requests/42"
    );
}

#[test]
fn gitlab_review_target_errors_loudly_on_api_error_response() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    // Regression: a live review dispatch crashed several layers downstream
    // with "invalid refspec ':refs/remotes/origin/'" because an empty/
    // invalid PRIVATE-TOKEN (missing GITLAB_PAT env var -- profile.pat()
    // reads the real process environment, not a loaded-but-unexported
    // env file) got a 200-shaped GitLab error body back, and the old
    // code silently defaulted source_branch/target_branch to "".
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "curl",
        "#!/bin/sh\necho '{\"message\":\"404 Project Not Found\"}'\nexit 0\n",
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = find_review_target_by_mr(&gitlab_profile(), "235").unwrap_err();

    assert!(format!("{:#}", err).contains("did not return a merge request"));
}

#[test]
fn review_targets_capture_provider_source_and_target_shas() {
    let gitlab = serde_json::json!({
        "iid": 42,
        "web_url": "https://gitlab.test/group/repo/-/merge_requests/42",
        "source_branch": "gah/42",
        "target_branch": "main",
        "sha": "source-gitlab-sha",
        "diff_refs": { "base_sha": "target-gitlab-sha" }
    });
    let gitlab_target = gitlab_target_from_value(&gitlab).unwrap();
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
fn merge_mr_github_un_drafts_then_merges() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    // Handles the 3 subcommand shapes merge_mr's call chain hits, in
    // order: `pr list` (resolve branch -> number), `pr view` (resolve
    // full ReviewTarget), `pr ready` (un-draft), `pr merge`. Fails
    // loudly (unrecognized args) instead of a lenient default if a
    // future edit changes the call sequence without updating this test.
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2" in
  "pr list") printf '[{"number":42}]\n' ;;
  "pr view") printf '{"number":42,"url":"https://github.com/owner/repo/pull/42","headRefName":"gah/test","baseRefName":"main"}\n' ;;
  "pr ready") exit 0 ;;
  "pr merge") echo "$@" > "${0%/*}/merge_call.txt"; exit 0 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    merge_mr(&github_profile(), "gah/test").unwrap();

    let call = fs::read_to_string(bin_dir.join("merge_call.txt")).unwrap();
    assert!(call.contains("42"));
    assert!(call.contains("--squash"));
    assert!(call.contains("--delete-branch"));
    // Regression (live on PR #255): without --admin, `gh pr merge` is
    // rejected by branch protection's required-approving-review count,
    // which gah's own issue-comment review verdict can never satisfy.
    assert!(call.contains("--admin"));
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
case "$1 $2" in
  "pr list") printf '[{"number":42}]\n' ;;
  "pr view") printf '{"number":42,"url":"https://github.com/owner/repo/pull/42","headRefName":"gah/test","baseRefName":"main"}\n' ;;
  "pr ready") echo "$@" > "${0%/*}/ready_call.txt"; exit 0 ;;
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
case "$1 $2" in
  "pr list") printf '[{"number":42}]\n' ;;
  "pr view") printf '{"number":42,"url":"https://github.com/owner/repo/pull/42","headRefName":"gah/test","baseRefName":"main"}\n' ;;
  "pr ready") exit 0 ;;
  "pr merge") echo "not mergeable: review required" >&2; exit 1 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = merge_mr(&github_profile(), "gah/test").unwrap_err();

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
        "curl",
        r#"#!/bin/sh
printf '[{"iid":7,"web_url":"https://gitlab.example.com/x/-/merge_requests/7","source_branch":"gah/test","target_branch":"main"}]\n'
"#,
    );
    make_fake_bin(
        &bin_dir,
        "glab",
        r#"#!/bin/sh
case "$1 $2" in
  "mr update") exit 0 ;;
  "mr merge") echo "$@" > "${0%/*}/merge_call.txt"; exit 0 ;;
  *) echo "unexpected glab invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    merge_mr(&gitlab_profile(), "gah/test").unwrap();

    let call = fs::read_to_string(bin_dir.join("merge_call.txt")).unwrap();
    assert!(call.contains(" 7 "));
    assert!(call.contains("--squash"));
    assert!(call.contains("--remove-source-branch"));
}

#[test]
fn mark_ready_for_review_gitlab_un_drafts_only() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "curl",
        r#"#!/bin/sh
printf '[{"iid":7,"web_url":"https://gitlab.example.com/x/-/merge_requests/7","source_branch":"gah/test","target_branch":"main"}]\n'
"#,
    );
    make_fake_bin(
        &bin_dir,
        "glab",
        r#"#!/bin/sh
case "$1 $2" in
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
case "$1" in
  pr)
    case "$2" in
      list) printf '[{"number":42}]\n' ;;
      comment) exit 0 ;;
      edit) echo "gh pr edit must not be called" >&2; exit 1 ;;
      *) echo "unexpected pr subcommand: $@" >&2; exit 1 ;;
    esac
    ;;
  api)
    if [ "$3" = "--jq" ]; then
      printf 'gah-needs-fix\nunrelated\n'
    else
      echo "$@" >> "${0%/*}/label_call.txt"
    fi
    exit 0
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
        &["gah-ready-for-human"],
    )
    .unwrap();

    let call = fs::read_to_string(bin_dir.join("label_call.txt")).unwrap();
    assert!(call.contains("repos/owner/repo/issues/42/labels"));
    assert!(call.contains("--method DELETE"));
    assert!(call.contains("gah-needs-fix"));
    assert!(call.contains("labels[]=gah-ready-for-human"));
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
case "$1" in
  pr) printf '[{"number":42}]\n' ;;
  api)
    if [ "$3" = "--jq" ]; then
      printf 'gah-needs-fix\nbug\n'
    else
      echo "$@" >> "${0%/*}/calls.txt"
    fi
    ;;
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
        "curl",
        r#"#!/bin/sh
case "$*" in
  *"merge_requests?state=opened"*)
    printf '[{"iid":7,"web_url":"https://gitlab.example/x/7","source_branch":"gah/test","target_branch":"main"}]\n'
    ;;
  *)
    echo "$@" > "${0%/*}/label_call.txt"
    printf '{}\n'
    ;;
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
  pr)
    case "$2" in
      list) printf '[{"number":42}]\n' ;;
      comment) exit 0 ;;
      *) echo "unexpected pr subcommand: $@" >&2; exit 1 ;;
    esac
    ;;
  api) echo "GraphQL: Projects (classic) is being deprecated" >&2; exit 1 ;;
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
