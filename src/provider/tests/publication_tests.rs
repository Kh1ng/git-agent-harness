use super::*;
use crate::provider::{post_issue_comment, post_review_comment};

#[test]
fn provider_draft_title_is_capped_after_prefix() {
    let title = draft_mr_title(&"é".repeat(300));

    assert_eq!(title.chars().count(), 255);
    assert!(title.starts_with("Draft: "));
    assert!(title.ends_with("..."));
}

#[test]
fn provider_publication_bodies_remove_home_paths_from_prs_and_comments() {
    let body = "Session: `session-123`; artifacts /home/operator/.local/share/gah/artifacts/session-123; mac /Users/operator/gah/session-123";
    let published = crate::provider::publication_body(body);
    assert!(published.contains("Session: `session-123`"));
    assert!(!published.contains("/home/operator/"));
    assert!(!published.contains("/Users/operator/"));
}

#[test]
fn provider_publication_bodies_remove_a_configured_home_outside_the_standard_roots() {
    let _guard = HomeOverride::set("/var/home/op".to_string());
    let body = "artifacts /var/home/op/.local/share/gah/artifacts/profile/sessions/session-123";

    let published = crate::provider::publication_body(body);

    assert!(published.contains("[local path removed]"));
    assert!(!published.contains("/var/home/op"));
    assert!(!published.contains("/var"));
}

#[test]
fn provider_publication_bodies_remove_configured_windows_home_paths() {
    for home in [r"C:\Users\operator", r"C:\Users\operator\"] {
        let _guard = HomeOverride::set(home.to_string());
        let body = r"artifacts `C:\Users\operator\AppData\Local\gah\artifacts\sessions\session-123`; mixed `C:\Users\operator/AppData/Local/gah`; home C:\Users\operator, sibling C:\Users\operator-tools\config.toml";

        assert_eq!(
            crate::provider::publication_body(body),
            r"artifacts `[local path removed]`; mixed `[local path removed]`; home [local path removed], sibling C:\Users\operator-tools\config.toml"
        );
    }
}

#[test]
fn provider_publication_bodies_remove_a_home_without_a_standard_root_prefix() {
    let _guard = HomeOverride::set("/srv/gah-operator".to_string());
    let body = "state /srv/gah-operator/state/gah/run.log; other /srv/unrelated/tool.log";

    let published = crate::provider::publication_body(body);

    assert!(published.contains("[local path removed]"));
    assert!(!published.contains("/srv/gah-operator"));
    assert!(published.contains("/srv/unrelated/tool.log"));
}

#[test]
fn provider_publication_bodies_do_not_redact_sibling_prefixes_of_home() {
    let _guard = HomeOverride::set("/srv/gah-operator".to_string());
    let body =
        "tools /srv/gah-operator-tools/config.toml; home /srv/gah-operator, end /srv/gah-operator";

    let published = crate::provider::publication_body(body);

    assert!(published.contains("/srv/gah-operator-tools/config.toml"));
    assert_eq!(
        published.matches("[local path removed]").count(),
        2,
        "{published}"
    );
}

#[test]
fn provider_publication_bodies_preserve_web_urls_with_home_like_paths() {
    let _guard = HomeOverride::set("/srv/gah-operator".to_string());
    for url in [
        "https://docs.example.com/root/reference",
        "http://docs.example.com/home/operator/reference",
        "https://docs.example.com/Users/operator/reference",
        "https://docs.example.com/srv/gah-operator/reference",
        "HTTPS://docs.example.com/root/reference",
    ] {
        let body = format!(
            "[reference]({url}) <{url}> `{url}` local `/root/private` /srv/gah-operator/private file:///home/operator/private"
        );
        let published = crate::provider::publication_body(&body);
        assert_eq!(
            published,
            format!(
                "[reference]({url}) <{url}> `{url}` local `[local path removed]` [local path removed] file://[local path removed]"
            )
        );
    }
}

#[test]
fn provider_publication_bodies_preserve_repo_relative_paths() {
    let body = "Updated `app/home/page.tsx`, edited src/root.rs and crates/root/lib.rs, see Controllers/Users/Index.cshtml and ios/Users/List.swift; analysis /root-cause.md and /root.x stay; artifacts /home/operator/.local/share/gah/artifacts/session-123";

    let published = crate::provider::publication_body(body);

    assert!(published.contains("`app/home/page.tsx`"), "{published}");
    assert!(published.contains("src/root.rs"), "{published}");
    assert!(published.contains("crates/root/lib.rs"), "{published}");
    assert!(
        published.contains("Controllers/Users/Index.cshtml"),
        "{published}"
    );
    assert!(published.contains("ios/Users/List.swift"), "{published}");
    assert!(published.contains("/root-cause.md"), "{published}");
    assert!(published.contains("/root.x"), "{published}");
    assert!(published.contains("[local path removed]"), "{published}");
    assert!(!published.contains("/home/operator"), "{published}");
}

#[test]
fn provider_publication_bodies_preserve_repo_relative_paths_for_a_configured_home() {
    let _guard = HomeOverride::set("/app".to_string());
    let body = "changed src/app/page.tsx and templates/app/home/page.tsx; state /app/gah/run.log";

    let published = crate::provider::publication_body(body);

    assert_eq!(
        published,
        "changed src/app/page.tsx and templates/app/home/page.tsx; state [local path removed]"
    );
}

#[test]
fn provider_publication_bodies_redact_home_paths_behind_markdown_punctuation() {
    let body =
        "[/home/ramrod/.local/share/gah](x) **/home/op/x** |/home/op/x| {/home/op/x} a,/home/op/x";

    let published = crate::provider::publication_body(body);

    assert_eq!(
        published,
        "[[local path removed]](x) **[local path removed]** |[local path removed]| {[local path removed]} a,[local path removed]"
    );
}

#[test]
fn provider_publication_bodies_redact_adjacent_configured_home_paths() {
    let _guard = HomeOverride::set("/srv/op".to_string());
    let body = "home /srv/op /srv/op/secret.log";

    let published = crate::provider::publication_body(body);

    assert_eq!(published, "home [local path removed] [local path removed]");
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
        "summary Authorization: Bearer abcdefghijklmnopqrstuvwxyz; artifacts /home/operator/.local/share/gah/artifacts/session-123",
    )
    .unwrap();

    let args = fs::read_to_string(args_path).unwrap();
    assert!(!args.contains("abcdefghijklmnopqrstuvwxyz"));
    assert!(args.contains("[REDACTED:TOKEN]"));
    assert!(!args.contains("/home/operator/"));
    assert!(args.contains("[local path removed]"));
}

#[test]
fn gitlab_mr_error_json_response_fails_closed() {
    const RAW_SECRET: &str = concat!("glpat-", "synthetic_publication_test_1371");
    const REDACTED_MARKER: &str = "[REDACTED:GITLAB_TOKEN]";

    assert_ne!(RAW_SECRET, REDACTED_MARKER);

    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"message\":\"404 Project Not Found\",\"token\":\"{RAW_SECRET}\"}}'\necho 'glab: API request failed: 404' >&2\nexit 1\n"
        ),
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    let err = create_draft_mr(&gitlab_profile(), "gah/test", "title", "body").unwrap_err();

    let msg = format!("{:#}", err);
    assert!(msg.contains("glab api gitlab create mr failed"));
    assert!(msg.contains("404 Project Not Found"));
    assert!(!msg.contains(RAW_SECRET));
    assert!(msg.contains(REDACTED_MARKER));
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

    post_issue_comment(&gitlab_profile(), "42", "already satisfied").unwrap();
    post_issue_comment(&gitlab_profile(), "42", "already satisfied").unwrap();

    assert_eq!(
        fs::read_to_string(bin_dir.join("glab.posts")).unwrap(),
        "post\n"
    );
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

    post_review_comment(
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
