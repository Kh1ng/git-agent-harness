use super::*;
use crate::provider::{delete_comment, update_comment, CommentThread};

#[test]
fn comment_mutations_reject_non_numeric_ids_and_handoff_writes() {
    let error = update_comment(
        &github_profile(),
        CommentThread::Issue("42"),
        "../99",
        "body",
    )
    .unwrap_err();
    assert!(error.to_string().contains("expected digits only"));

    let mut profile = github_profile();
    profile.delivery_mode = crate::config::DeliveryMode::Handoff;
    update_comment(&profile, CommentThread::Issue("123"), "456", "body").unwrap_err();
    delete_comment(&profile, CommentThread::Issue("123"), "456").unwrap_err();
}

#[test]
fn github_comment_update_and_delete_use_the_shared_comment_resource() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "gh",
        r#"#!/bin/sh
echo "$@" >> "${0%/*}/calls.txt"
printf '{}\n'
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    update_comment(
        &github_profile(),
        CommentThread::Issue("42"),
        "99",
        "updated body",
    )
    .unwrap();
    delete_comment(&github_profile(), CommentThread::Review("gah/test"), "99").unwrap();

    let calls = fs::read_to_string(bin_dir.join("calls.txt")).unwrap();
    assert!(calls.contains(
        "api --method PATCH repos/owner/repo/issues/comments/99 --raw-field body=updated body"
    ));
    assert!(calls.contains("api --method DELETE repos/owner/repo/issues/comments/99"));
}

#[test]
fn gitlab_issue_comment_update_and_delete_use_the_issue_note_resource() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        r#"#!/bin/sh
echo "$@" >> "${0%/*}/calls.txt"
case "$*" in *"--method DELETE"*) ;; *) printf '{}\n' ;; esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    update_comment(
        &gitlab_profile(),
        CommentThread::Issue("17"),
        "99",
        "updated body",
    )
    .unwrap();
    delete_comment(&gitlab_profile(), CommentThread::Issue("17"), "99").unwrap();

    let calls = fs::read_to_string(bin_dir.join("calls.txt")).unwrap();
    assert!(calls.contains("api projects/42/issues/17/notes/99"));
    assert!(calls.contains("--method PUT --raw-field body=updated body"));
    assert!(calls.contains("--method DELETE"));
}

#[test]
fn gitlab_review_comment_resolves_the_branch_before_mutating_its_note() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin(
        &bin_dir,
        "glab",
        r#"#!/bin/sh
case "$*" in
  *projects/42/merge_requests*"--method GET"*source_branch=gah/test*)
    printf '[{"iid":7,"web_url":"https://gitlab.example.com/x/-/merge_requests/7","source_branch":"gah/test","target_branch":"main","title":"Draft: test","description":"body","draft":true}]\n'
    ;;
  *)
    echo "$@" >> "${0%/*}/calls.txt"
    case "$*" in *"--method DELETE"*) ;; *) printf '{}\n' ;; esac
    ;;
esac
"#,
    );
    let _guard = PathOverride::set(bin_dir.to_str().unwrap().to_string());

    update_comment(
        &gitlab_profile(),
        CommentThread::Review("gah/test"),
        "99",
        "updated body",
    )
    .unwrap();
    delete_comment(&gitlab_profile(), CommentThread::Review("gah/test"), "99").unwrap();

    let calls = fs::read_to_string(bin_dir.join("calls.txt")).unwrap();
    assert!(calls.contains("api projects/42/merge_requests/7/notes/99"));
    assert!(calls.contains("--method PUT --raw-field body=updated body"));
    assert!(calls.contains("--method DELETE"));
}
