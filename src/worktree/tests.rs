use super::*;
use std::process::Command as StdCommand;
use tempfile::TempDir;

fn init_bare_repo_with_main(dir: &Path) {
    StdCommand::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(dir)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir)
        .output()
        .unwrap();
    fs::write(dir.join("f.txt"), "content\n").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(dir)
        .output()
        .unwrap();
}

fn add_bare_origin(repo: &Path) -> PathBuf {
    let bare = repo.parent().unwrap().join("origin.git");
    StdCommand::new("git")
        .args(["init", "--bare", "-q", bare.to_str().unwrap()])
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["remote", "add", "origin", bare.to_str().unwrap()])
        .current_dir(repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-q", "-u", "origin", "main"])
        .current_dir(repo)
        .output()
        .unwrap();
    bare
}

// ── git() / git_raw() ───────────────────────────────────────────────

#[test]
fn git_bails_on_nonzero_exit_with_stderr_context() {
    let tmp = TempDir::new().unwrap();
    init_bare_repo_with_main(tmp.path());

    let err = git(&["not-a-real-git-subcommand"], tmp.path()).unwrap_err();

    let msg = format!("{:#}", err);
    assert!(msg.contains("git not-a-real-git-subcommand"), "{msg}");
}

#[test]
fn git_missing_working_directory_surfaces_actionable_error() {
    let missing = std::env::temp_dir().join("gah-test-definitely-missing-dir-xyz");
    let _ = fs::remove_dir_all(&missing);

    let err = git(&["status"], &missing).unwrap_err();

    // std::process::Command surfaces this as a launch error via the
    // anyhow context wired in git_raw(), not a git stderr message.
    assert!(format!("{:#}", err).contains("git status"));
}

#[test]
fn wait_with_timeout_drains_output_larger_than_os_pipe_capacity() {
    let child = StdCommand::new("sh")
        .args([
            "-c",
            "head -c 262144 /dev/zero; head -c 262144 /dev/zero >&2",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let output = wait_with_timeout_for(child, "large-output fixture", Duration::from_secs(2))
        .expect("concurrent pipe drains must prevent a child-output deadlock");

    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 262_144);
    assert_eq!(output.stderr.len(), 262_144);
}

// ── create() ─────────────────────────────────────────────────────────

#[test]
fn create_fails_loudly_when_target_branch_does_not_exist_on_origin() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    let worktree_base = tmp.path().join("worktrees");

    let err = create(&repo, "does-not-exist", "gah/test-1", &worktree_base).unwrap_err();

    let msg = format!("{:#}", err);
    assert!(
        msg.contains("creating worktree from origin/does-not-exist"),
        "{msg}"
    );
}

#[test]
fn create_succeeds_for_real_branch() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    let worktree_base = tmp.path().join("worktrees");

    let wt = create(&repo, "main", "gah/test-2", &worktree_base).unwrap();

    assert!(wt.join("f.txt").exists());
}

#[test]
fn create_from_linked_worktree_uses_shared_git_directory_for_fetch_lock() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);

    let linked = tmp.path().join("linked");
    git(
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            linked.to_str().unwrap(),
            "main",
        ],
        &repo,
    )
    .unwrap();
    assert!(linked.join(".git").is_file());

    let worktree_base = tmp.path().join("worktrees");
    let created = create(&linked, "main", "gah/from-linked", &worktree_base).unwrap();

    assert!(created.join("f.txt").exists());
    assert!(repo.join(".git/gah-fetch.lock").is_file());
}

// ── ensure_staged() ──────────────────────────────────────────────────

#[test]
fn ensure_staged_fails_when_nothing_is_staged() {
    let tmp = TempDir::new().unwrap();
    init_bare_repo_with_main(tmp.path());

    let err = ensure_staged(tmp.path()).unwrap_err();

    assert!(format!("{:#}", err).contains("nothing to commit"));
}

// ── has_uncommitted_changes() ────────────────────────────────────────

#[test]
fn has_uncommitted_changes_false_when_backend_already_committed_its_own_work() {
    let tmp = TempDir::new().unwrap();
    // Nest under a `repo` subdir, not tmp.path() directly -- add_bare_origin
    // creates the bare origin as a *sibling* of its argument, and every
    // other test in this file uses a repo subdir for exactly this reason
    // (tmp.path() directly would make the bare origin land in the shared
    // system temp root, colliding with other parallel tests' origins).
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    // Simulate a backend (e.g. vibe) that commits its own changes during
    // the run, leaving HEAD ahead of origin/main but a clean working tree.
    fs::write(repo.join("g.txt"), "backend wrote this\n").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-q", "-m", "backend self-commit"])
        .current_dir(&repo)
        .output()
        .unwrap();

    assert!(!has_uncommitted_changes(&repo).unwrap());
    // has_changes must still report true via the origin diff -- this
    // commit is real work that needs pushing, just not re-staged.
    assert!(has_changes(&repo, "main").unwrap());
}

#[test]
fn state_snapshot_distinguishes_attempt_progress_from_existing_pr_changes() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);

    // Model an existing PR branch: it already differs from main before a
    // repair starts, so branch-vs-main cannot prove repair progress.
    fs::write(repo.join("existing.txt"), "existing PR change\n").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-q", "-m", "existing PR"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(has_changes(&repo, "main").unwrap());

    let before = state_snapshot(&repo).unwrap();
    assert_eq!(state_snapshot(&repo).unwrap(), before);

    // Uncommitted edits and backend-created commits both count as net
    // progress relative to this attempt's own starting point.
    fs::write(repo.join("existing.txt"), "repaired\n").unwrap();
    assert_ne!(state_snapshot(&repo).unwrap(), before);
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-q", "-m", "repair"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_ne!(state_snapshot(&repo).unwrap(), before);
}

#[test]
fn has_uncommitted_changes_true_for_a_dirty_working_tree() {
    let tmp = TempDir::new().unwrap();
    init_bare_repo_with_main(tmp.path());
    fs::write(tmp.path().join("f.txt"), "modified\n").unwrap();

    assert!(has_uncommitted_changes(tmp.path()).unwrap());
}

// ── push_branch() ────────────────────────────────────────────────────

#[test]
fn transient_network_classifier_matches_only_transport_weather() {
    for text in [
        "ssh: connect to host github.com port 22: Connection timed out",
        "fatal: the remote end hung up unexpectedly: Connection reset by peer",
        "fatal: could not resolve host: github.com",
        "fatal: early EOF",
        "ssh_exchange_identification: Connection closed by remote host",
    ] {
        assert!(
            is_transient_network_error(text),
            "expected transient: {text}"
        );
    }
    for text in [
        "remote: Permission to owner/repo denied to user",
        "fatal: Authentication failed for 'https://github.com/owner/repo.git/'",
        "! [rejected] main -> main (non-fast-forward)",
    ] {
        assert!(
            !is_transient_network_error(text),
            "unexpected transient: {text}"
        );
    }
}

#[test]
fn transient_network_operation_retries_once_then_succeeds() {
    let mut attempts = 0;
    let result = retry_transient_git_network("test push", || {
        attempts += 1;
        if attempts == 1 {
            anyhow::bail!("ssh: connect to host github.com port 22: Connection timed out");
        }
        Ok("pushed")
    });
    assert_eq!(result.unwrap(), "pushed");
    assert_eq!(attempts, 2);
}

#[test]
fn push_retries_fake_git_timeout_once_then_completes() {
    // This fixture is a checked-in, immutable script (see the file for
    // why) rather than one written+chmod'd here at test run time, so
    // that this test can never race a concurrent fork() elsewhere in the
    // parallel suite into an ETXTBSY ("Text file busy") failure.
    let fake_git = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/fake_git_push_retry.sh"
    ));
    let tmp = TempDir::new().unwrap();
    let count_path = tmp.path().join("push-count");

    push_branch_with_executable(
        &fake_git,
        tmp.path(),
        "main",
        count_path.to_str().unwrap(),
        "",
    )
    .unwrap();

    assert_eq!(fs::read_to_string(count_path).unwrap(), "2");
}

#[test]
fn non_transient_network_operation_does_not_retry() {
    let mut attempts = 0;
    let result: Result<()> = retry_transient_git_network("test push", || {
        attempts += 1;
        anyhow::bail!("fatal: Authentication failed")
    });
    assert!(result.is_err());
    assert_eq!(attempts, 1);
}

#[test]
fn push_branch_fails_loudly_for_unreachable_remote() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    let bogus_remote = tmp.path().join("does-not-exist-as-a-remote");

    let err = push_branch(&repo, "main", bogus_remote.to_str().unwrap(), "").unwrap_err();

    assert!(format!("{:#}", err).contains("push failed"));
}

// ── create_existing() ─────────────────────────────────────────────────────

#[test]
fn create_existing_succeeds_for_real_branch() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    let worktree_base = tmp.path().join("worktrees");

    // First create a branch on the origin
    StdCommand::new("git")
        .args(["checkout", "-b", "test-branch"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "origin", "test-branch"])
        .current_dir(&repo)
        .output()
        .unwrap();
    // Go back to main
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();

    // Now try to create worktree from existing branch
    let wt = create_existing(&repo, "test-branch", &worktree_base).unwrap();

    assert!(wt.join("f.txt").exists());
}

#[test]
fn create_existing_checks_out_a_real_local_branch_not_detached_head() {
    // Regression: `git worktree add <path> origin/<branch>` (no -B)
    // leaves the worktree in detached HEAD -- there's no local ref
    // named <branch> to serve as a push source, so a later
    // `git push origin <branch>` from that worktree silently exits 0
    // while pushing nothing at all. `-B <branch>` must be present so
    // the worktree is actually checked out onto a real local branch.
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    let bare_origin = add_bare_origin(&repo);
    let worktree_base = tmp.path().join("worktrees");

    StdCommand::new("git")
        .args(["checkout", "-q", "-b", "test-branch"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-q", "origin", "test-branch"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "-q", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();

    let wt = create_existing(&repo, "test-branch", &worktree_base).unwrap();

    let symbolic_ref = StdCommand::new("git")
        .args(["symbolic-ref", "HEAD"])
        .current_dir(&wt)
        .output()
        .unwrap();
    assert!(
        symbolic_ref.status.success(),
        "worktree must be on a real branch, not detached HEAD"
    );
    assert_eq!(
        String::from_utf8_lossy(&symbolic_ref.stdout).trim(),
        "refs/heads/test-branch"
    );

    // Commit a change and push it back -- this is the actual
    // regression: confirm the commit reaches the remote branch, not
    // just that `git push` reports success.
    fs::write(wt.join("f.txt"), "modified by fix\n").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&wt)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-q", "-m", "a fix"])
        .current_dir(&wt)
        .output()
        .unwrap();
    push_branch(&wt, "test-branch", bare_origin.to_str().unwrap(), "").unwrap();

    let log = StdCommand::new("git")
        .args(["log", "--oneline", "refs/heads/test-branch"])
        .current_dir(&bare_origin)
        .output()
        .unwrap();
    let log_text = String::from_utf8_lossy(&log.stdout);
    assert!(
        log_text.contains("a fix"),
        "the commit must actually reach the remote branch, got: {log_text}"
    );
}

#[test]
fn refresh_existing_branch_merges_new_target_before_repair() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    let worktree_base = tmp.path().join("worktrees");

    git(&["checkout", "-b", "repair"], &repo).unwrap();
    fs::write(repo.join("repair.txt"), "existing repair\n").unwrap();
    git(&["add", "repair.txt"], &repo).unwrap();
    git(&["commit", "-m", "repair work"], &repo).unwrap();
    git(&["push", "origin", "repair"], &repo).unwrap();
    git(&["checkout", "main"], &repo).unwrap();
    fs::write(repo.join("target-fix.txt"), "fixed on main\n").unwrap();
    git(&["add", "target-fix.txt"], &repo).unwrap();
    git(&["commit", "-m", "target fix"], &repo).unwrap();
    git(&["push", "origin", "main"], &repo).unwrap();

    let wt = create_existing(&repo, "repair", &worktree_base).unwrap();
    assert_eq!(
        refresh_existing_branch_from_target(&wt, "main").unwrap(),
        TargetRefreshOutcome::Merged
    );
    assert_eq!(
        fs::read_to_string(wt.join("target-fix.txt")).unwrap(),
        "fixed on main\n"
    );
    assert!(
        git_raw(&["merge-base", "--is-ancestor", "origin/main", "HEAD"], &wt)
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn refresh_existing_branch_is_noop_when_target_is_already_present() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    let worktree_base = tmp.path().join("worktrees");

    git(&["checkout", "-b", "repair"], &repo).unwrap();
    git(&["push", "origin", "repair"], &repo).unwrap();
    git(&["checkout", "main"], &repo).unwrap();
    let wt = create_existing(&repo, "repair", &worktree_base).unwrap();
    let head_before = git(&["rev-parse", "HEAD"], &wt).unwrap();

    assert_eq!(
        refresh_existing_branch_from_target(&wt, "main").unwrap(),
        TargetRefreshOutcome::AlreadyCurrent
    );
    assert_eq!(git(&["rev-parse", "HEAD"], &wt).unwrap(), head_before);
}

#[test]
fn refresh_existing_branch_returns_live_conflict_for_bounded_repair() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    let worktree_base = tmp.path().join("worktrees");

    git(&["checkout", "-b", "repair"], &repo).unwrap();
    fs::write(repo.join("f.txt"), "repair version\n").unwrap();
    git(&["add", "f.txt"], &repo).unwrap();
    git(&["commit", "-m", "repair edit"], &repo).unwrap();
    git(&["push", "origin", "repair"], &repo).unwrap();
    git(&["checkout", "main"], &repo).unwrap();
    fs::write(repo.join("f.txt"), "target version\n").unwrap();
    git(&["add", "f.txt"], &repo).unwrap();
    git(&["commit", "-m", "target edit"], &repo).unwrap();
    git(&["push", "origin", "main"], &repo).unwrap();

    let wt = create_existing(&repo, "repair", &worktree_base).unwrap();
    let outcome = refresh_existing_branch_from_target(&wt, "main").unwrap();

    let TargetRefreshOutcome::Conflicted {
        target_ref,
        files,
        details,
    } = outcome
    else {
        panic!("expected a repairable merge conflict")
    };
    assert_eq!(target_ref, "origin/main");
    assert_eq!(files, vec!["f.txt"]);
    assert!(details.contains("CONFLICT (content)"));
    assert!(merge_in_progress(&wt).unwrap());
    assert_eq!(unmerged_files(&wt).unwrap(), vec!["f.txt"]);
    assert_eq!(files_with_conflict_markers(&wt, &files).unwrap(), files);

    let recovery = tmp.path().join("recovery");
    preserve_conflict_recovery(&wt, &recovery, "main").unwrap();
    let manifest = fs::read_to_string(recovery.join("manifest.txt")).unwrap();
    assert!(manifest.contains("target=origin/main"));
    assert!(manifest.contains("unmerged_files=f.txt"));
    assert!(recovery.join("working.patch").exists());

    fs::write(wt.join("f.txt"), "resolved target + repair\n").unwrap();
    git(&["add", "f.txt"], &wt).unwrap();
    assert!(unmerged_files(&wt).unwrap().is_empty());
    assert!(files_with_conflict_markers(&wt, &files).unwrap().is_empty());
    assert!(merge_in_progress(&wt).unwrap());
    assert!(!target_is_ancestor(&wt, "main").unwrap());
    git(&["commit", "-m", "resolve target refresh"], &wt).unwrap();
    assert!(target_is_ancestor(&wt, "main").unwrap());
}

#[test]
fn branch_attachment_detects_foreign_worktree_without_inferring_ownership() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    StdCommand::new("git")
        .args(["checkout", "-q", "-b", "shared-branch"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-q", "origin", "shared-branch"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "-q", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();
    // An externally-owned worktree living OUTSIDE GAH's managed base.
    let foreign = tmp.path().join("external-worktree");
    let add = StdCommand::new("git")
        .args([
            "worktree",
            "add",
            "-q",
            "-B",
            "shared-branch",
            foreign.to_str().unwrap(),
            "origin/shared-branch",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(add.status.success(), "worktree add must succeed");

    let attachment = branch_attachment(&repo, "shared-branch").unwrap();
    let attachment = attachment.expect("must detect the attached worktree");
    assert_eq!(
        attachment.path, foreign,
        "path must be the foreign worktree"
    );
    assert!(attachment.clean);

    // The same branch with no foreign attachment reports nothing.
    let none = branch_attachment(&repo, "no-such-branch").unwrap();
    assert!(none.is_none());
}

#[test]
fn create_existing_never_replaces_an_attached_worktree_at_the_expected_path() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    let worktree_base = tmp.path().join("worktrees");

    StdCommand::new("git")
        .args(["checkout", "-q", "-b", "shared-branch"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-q", "origin", "shared-branch"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "-q", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();

    let wt = create_existing(&repo, "shared-branch", &worktree_base).unwrap();
    fs::write(wt.join("uncommitted.txt"), "must survive").unwrap();

    let attachment = branch_attachment(&repo, "shared-branch").unwrap();
    let attachment = attachment.expect("must detect the attached worktree");
    assert!(!attachment.clean);

    let error = create_existing(&repo, "shared-branch", &worktree_base).unwrap_err();
    assert!(error
        .to_string()
        .contains("refusing to replace existing worktree path"));
    assert_eq!(
        fs::read_to_string(wt.join("uncommitted.txt")).unwrap(),
        "must survive"
    );
}

// ── fetch_remote_branch_sha() ───────────────────────────────────────
// Issue #537: this is the primitive the repair retry-reset loop relies
// on to detect a concurrent push to the repair branch instead of
// silently resetting over it (or, worse, resetting to `main`).

#[test]
fn fetch_remote_branch_sha_returns_the_branch_tip_matching_local_head() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    add_bare_origin(&repo);
    StdCommand::new("git")
        .args(["checkout", "-q", "-b", "gah/repair"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-q", "-u", "origin", "gah/repair"])
        .current_dir(&repo)
        .output()
        .unwrap();
    // `create_existing` uses a real (non-detached) local branch, so it
    // can't share a checkout with `repo` itself still on that branch.
    StdCommand::new("git")
        .args(["checkout", "-q", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let head = git(&["rev-parse", "gah/repair"], &repo).unwrap();

    let worktree_base = tmp.path().join("worktrees");
    let wt = create_existing(&repo, "gah/repair", &worktree_base).unwrap();

    let observed = fetch_remote_branch_sha(&repo, &wt, "gah/repair").unwrap();

    assert_eq!(observed, head);
}

#[test]
fn fetch_remote_branch_sha_detects_a_concurrent_push_to_the_repair_branch() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_bare_repo_with_main(&repo);
    let bare = add_bare_origin(&repo);
    StdCommand::new("git")
        .args(["checkout", "-q", "-b", "gah/repair"])
        .current_dir(&repo)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-q", "-u", "origin", "gah/repair"])
        .current_dir(&repo)
        .output()
        .unwrap();
    // `create_existing` uses a real (non-detached) local branch, so it
    // can't share a checkout with `repo` itself still on that branch.
    StdCommand::new("git")
        .args(["checkout", "-q", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();

    let worktree_base = tmp.path().join("worktrees");
    let wt = create_existing(&repo, "gah/repair", &worktree_base).unwrap();
    let repair_base_sha = fetch_remote_branch_sha(&repo, &wt, "gah/repair").unwrap();

    // Simulate a human (or another gah process) pushing a new commit to
    // the repair branch from a second clone while a retry loop is
    // mid-attempt against the first worktree.
    let second_clone = tmp.path().join("second-clone");
    StdCommand::new("git")
        .args([
            "clone",
            "-q",
            "-b",
            "gah/repair",
            bare.to_str().unwrap(),
            second_clone.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    fs::write(second_clone.join("concurrent.txt"), "human push").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&second_clone)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args([
            "-c",
            "user.email=human@example.com",
            "-c",
            "user.name=Human",
            "commit",
            "-q",
            "-m",
            "concurrent push",
        ])
        .current_dir(&second_clone)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-q"])
        .current_dir(&second_clone)
        .output()
        .unwrap();

    let observed = fetch_remote_branch_sha(&repo, &wt, "gah/repair").unwrap();

    assert_ne!(
        observed, repair_base_sha,
        "must observe the concurrent push, not the stale local view captured at dispatch start"
    );
}
