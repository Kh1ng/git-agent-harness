use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::{Child, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GIT_TIMEOUT: Duration = Duration::from_secs(300);
const GIT_NETWORK_ATTEMPTS: u8 = 2;
#[cfg(not(test))]
const GIT_NETWORK_RETRY_BACKOFF: Duration = Duration::from_secs(10);

/// Return true only for transport failures that are normally transient.
///
/// Authentication, authorization, non-fast-forward, and ordinary git errors
/// deliberately do not match: retrying them would hide a real configuration
/// or repository problem behind a pointless delay.
pub fn is_transient_network_error(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "connection timed out",
        "connection reset",
        "could not resolve host",
        "early eof",
        "ssh_exchange_identification",
    ]
    .iter()
    .any(|signature| text.contains(signature))
}

fn git_network_retry_backoff() -> Duration {
    #[cfg(test)]
    {
        Duration::ZERO
    }
    #[cfg(not(test))]
    {
        GIT_NETWORK_RETRY_BACKOFF
    }
}

fn retry_transient_git_network<T>(
    operation: &str,
    mut attempt: impl FnMut() -> Result<T>,
) -> Result<T> {
    for number in 1..=GIT_NETWORK_ATTEMPTS {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(err)
                if number < GIT_NETWORK_ATTEMPTS
                    && is_transient_network_error(&format!("{err:#}")) =>
            {
                eprintln!(
                    "transient git network failure during {operation}; retrying {}/{} after {}s: {:#}",
                    number + 1,
                    GIT_NETWORK_ATTEMPTS,
                    git_network_retry_backoff().as_secs(),
                    err
                );
                thread::sleep(git_network_retry_backoff());
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("bounded git retry loop always returns")
}

fn wait_with_timeout(mut child: Child, context: &str) -> Result<Output> {
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return child
                .wait_with_output()
                .with_context(|| format!("collecting {context} output"));
        }
        if started.elapsed() >= GIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{context} timed out after {}s", GIT_TIMEOUT.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn fetch_origin(repo: &Path) -> Result<()> {
    let git_dir = repo.join(".git");
    let lock_path = git_dir.join("gah-fetch.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening fetch lock {}", lock_path.display()))?;
    lock.lock_exclusive().context("locking shared git fetch")?;
    let result =
        retry_transient_git_network("fetch", || git(&["fetch", "-q", "origin", "--prune"], repo));
    FileExt::unlock(&lock).ok();
    result.map(|_| ())
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DiffStats {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

pub fn git(args: &[&str], cwd: &Path) -> Result<String> {
    let out = git_raw(args, cwd)?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            crate::redact::redact(&String::from_utf8_lossy(&out.stderr)).trim()
        );
    }
    Ok(crate::redact::redact(&String::from_utf8_lossy(&out.stdout))
        .trim()
        .to_string())
}

/// Run git and return raw Output. Does NOT error on non-zero exit.
pub fn git_raw(args: &[&str], cwd: &Path) -> Result<std::process::Output> {
    let child = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("git {}", args.join(" ")))?;
    wait_with_timeout(child, &format!("git {}", args.join(" ")))
}

pub fn create(
    repo: &Path,
    target_branch: &str,
    new_branch: &str,
    worktree_base: &Path,
) -> Result<PathBuf> {
    fetch_origin(repo)?;

    let origin_ref = format!("origin/{}", target_branch);
    let worktree_path = worktree_base.join(new_branch.replace('/', "-"));
    fs::create_dir_all(worktree_path.parent().unwrap_or(worktree_base))?;

    git(
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            new_branch,
            worktree_path.to_str().unwrap(),
            &origin_ref,
        ],
        repo,
    )
    .with_context(|| format!("creating worktree from {}", origin_ref))?;

    Ok(worktree_path)
}

pub fn create_existing(
    repo: &Path,
    existing_branch: &str,
    worktree_base: &Path,
) -> Result<PathBuf> {
    fetch_origin(repo)?;

    let origin_ref = format!("origin/{}", existing_branch);
    let worktree_path = worktree_base.join(existing_branch.replace('/', "-"));
    fs::create_dir_all(worktree_path.parent().unwrap_or(worktree_base))?;

    // Never force-remove an existing path here. Its pathname is not proof
    // that GAH owns it, and even a GAH-created worktree can still belong to a
    // live worker or contain uncommitted recovery data. Lifecycle pruning is
    // the only subsystem allowed to remove retained worktrees.
    if worktree_path.exists() {
        anyhow::bail!(
            "refusing to replace existing worktree path {}; prune or remove it explicitly after verifying it is inactive",
            worktree_path.display()
        );
    }

    // `-B existing_branch` creates/resets a real local branch tracking
    // origin_ref instead of leaving the worktree in detached HEAD.
    // Without this, `git push origin <existing_branch>` from the worktree
    // silently exits 0 while pushing nothing -- there's no local ref by
    // that name to serve as the push source, since detached HEAD isn't
    // one. Confirmed by reproduction: a commit made on a detached-HEAD
    // worktree checkout of `origin/<branch>` never reached the remote
    // branch even though `git push` reported success.
    git(
        &[
            "worktree",
            "add",
            "-q",
            "-B",
            existing_branch,
            worktree_path.to_str().unwrap(),
            &origin_ref,
        ],
        repo,
    )
    .with_context(|| format!("creating worktree from existing branch {}", origin_ref))?;

    Ok(worktree_path)
}

/// Describes a worktree currently attached to a branch, as reported by
/// `git worktree list`, if one exists.
pub struct BranchWorktreeAttachment {
    /// Absolute path of the attached worktree.
    pub path: PathBuf,
    /// True when the attached worktree has no uncommitted changes.
    pub clean: bool,
}

/// Returns the worktree (if any) currently attached to `branch`. A branch
/// checked out in a worktree cannot be reused by `git worktree add`; doing so
/// fails with "branch is already used by worktree at '<path>'", which would
/// otherwise terminate a recurring `gah loop`. Detect this before dispatch so
/// the controller can defer the work item instead of stalling on a hard git
/// failure.
///
/// Detection deliberately does not infer ownership from path. Any attachment
/// can be live or dirty and must be deferred until lifecycle cleanup proves it
/// safe to remove.
pub fn branch_attachment(repo: &Path, branch: &str) -> Result<Option<BranchWorktreeAttachment>> {
    let out = git_raw(&["worktree", "list", "--porcelain"], repo)?;
    if !out.status.success() {
        // If we can't enumerate worktrees, err on the side of attempting the
        // dispatch -- better a surfaced transient failure than a silent skip.
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut current_path: Option<PathBuf> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(p.trim()));
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some(path) = &current_path {
                let branch_ref = format!("refs/heads/{}", branch);
                if b.trim() == branch_ref {
                    let clean = path.exists() && matches!(has_uncommitted_changes(path), Ok(false));
                    return Ok(Some(BranchWorktreeAttachment {
                        path: path.clone(),
                        clean,
                    }));
                }
            }
        }
    }
    Ok(None)
}

pub fn has_changes(worktree: &Path, target_branch: &str) -> Result<bool> {
    if has_uncommitted_changes(worktree)? {
        return Ok(true);
    }
    // ponytail: compare against origin/<target> — @{upstream} fails silently on new untracked branches
    let origin_ref = format!("origin/{}", target_branch);
    let diff = git_raw(&["diff", "HEAD", &origin_ref], worktree)?;
    Ok(!diff.stdout.is_empty())
}

/// Some backends (e.g. vibe) commit their own work during the run instead of
/// leaving a dirty working tree for GAH to stage. `has_changes` can be true
/// purely from those already-committed commits sitting ahead of the target
/// branch -- callers must check this separately before staging, or
/// `ensure_staged` fails loudly on a clean tree ("nothing to commit").
pub fn has_uncommitted_changes(worktree: &Path) -> Result<bool> {
    let status = git_raw(&["status", "--porcelain"], worktree)?;
    Ok(!status.stdout.is_empty())
}

#[allow(dead_code)]
pub fn diff_patch(worktree: &Path, target_branch: &str) -> Result<String> {
    let origin_ref = format!("origin/{}", target_branch);
    Ok(
        String::from_utf8_lossy(&git_raw(&["diff", &origin_ref, "HEAD"], worktree)?.stdout)
            .to_string(),
    )
}

#[allow(dead_code)]
pub fn changed_files(worktree: &Path, target_branch: &str) -> Result<Vec<String>> {
    let origin_ref = format!("origin/{}", target_branch);
    let out = git_raw(&["diff", "--name-only", &origin_ref, "HEAD"], worktree)?;
    let tracked = String::from_utf8_lossy(&out.stdout).to_string();
    let mut files: Vec<String> = tracked
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    let status = git_raw(&["status", "--porcelain"], worktree)?;
    for line in String::from_utf8_lossy(&status.stdout).lines() {
        if line.is_empty() {
            continue;
        }
        let first = line.as_bytes().first().copied().unwrap_or(b' ');
        let second = line.as_bytes().get(1).copied().unwrap_or(b' ');
        if first != b' ' || second != b' ' {
            files.push(line[3..].trim().to_string());
        }
    }
    Ok(files)
}

pub fn diff_stats(worktree: &Path, target_branch: &str) -> Result<DiffStats> {
    let origin_ref = format!("origin/{}", target_branch);
    let out = git_raw(&["diff", "--numstat", &origin_ref, "HEAD"], worktree)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut stats = DiffStats::default();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(adds) = parts.next() else { continue };
        let Some(dels) = parts.next() else { continue };
        stats.files_changed += 1;
        stats.insertions += adds.parse::<u32>().unwrap_or(0);
        stats.deletions += dels.parse::<u32>().unwrap_or(0);
    }
    Ok(stats)
}

#[allow(dead_code)]
pub fn commit_and_push(
    worktree: &Path,
    branch: &str,
    push_url: &str,
    repo_id: &str,
    pat: &str,
) -> Result<()> {
    stage_all(worktree)?;
    ensure_staged(worktree)?;
    commit_msg(
        worktree,
        &format!("gah: improve mode changes for {}", repo_id),
    )?;
    push_branch(worktree, branch, push_url, pat)
}

/// Write a temporary GIT_ASKPASS script that outputs the given password.
/// Returns the path to the script. The caller MUST clean up the file.
fn write_askpass(pat: &str) -> Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!("gah-askpass-{}", std::process::id()));
    let mut f = std::fs::File::create(&path)?;
    f.write_all(b"#!/bin/sh\n")?;
    f.write_all(b"echo \"")?;
    f.write_all(pat.as_bytes())?;
    f.write_all(b"\"\n")?;
    // Make executable
    use std::os::unix::fs::PermissionsExt;
    f.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

pub fn stage_all(worktree: &Path) -> Result<()> {
    git(&["add", "-A"], worktree)?;
    Ok(())
}

pub fn ensure_staged(worktree: &Path) -> Result<()> {
    let staged = git_raw(&["diff", "--cached", "--name-only"], worktree)?;
    if staged.stdout.is_empty() {
        anyhow::bail!("nothing to commit after git add -A");
    }
    Ok(())
}

pub fn commit_msg(worktree: &Path, msg: &str) -> Result<()> {
    git(&["commit", "-q", "-m", msg], worktree)?;
    Ok(())
}

/// Make the current changed tree durable before a dispatch discards or
/// replaces it. Backends sometimes commit themselves; otherwise this creates
/// a local WIP commit. The dispatch branch remains the recovery point for a
/// terminal failure, while retry callers may additionally retain `HEAD` on a
/// checkpoint branch before resetting the working branch to its target.
pub fn preserve_wip(worktree: &Path, target_branch: &str, message: &str) -> Result<bool> {
    if !has_changes(worktree, target_branch)? {
        return Ok(false);
    }
    if has_uncommitted_changes(worktree)? {
        stage_all(worktree)?;
        ensure_staged(worktree)?;
        commit_msg(worktree, message)?;
    }
    Ok(true)
}

/// Preserve changed work under a dedicated checkpoint branch, then let a
/// retry start from the clean target. The checkpoint is local and is removed
/// only after the overall dispatch publishes successfully.
pub fn checkpoint_wip(
    worktree: &Path,
    target_branch: &str,
    checkpoint_branch: &str,
    message: &str,
) -> Result<bool> {
    if !preserve_wip(worktree, target_branch, message)? {
        return Ok(false);
    }
    git(&["branch", "-f", checkpoint_branch, "HEAD"], worktree)?;
    Ok(true)
}

/// Return a retry worktree to the configured target without moving any WIP
/// checkpoint ref created by `checkpoint_wip`.
pub fn reset_to_target(worktree: &Path, target_branch: &str) -> Result<()> {
    let target = format!("origin/{target_branch}");
    git(&["reset", "--hard", &target], worktree)?;
    git(&["clean", "-fd"], worktree)?;
    Ok(())
}

/// Best-effort removal of local, successful-dispatch-only WIP checkpoint
/// refs. Never use this for terminal failures: those refs are recovery data.
pub fn delete_local_branch(repo: &Path, branch: &str) -> Result<()> {
    git(&["branch", "-D", branch], repo)?;
    Ok(())
}

pub fn push_branch(worktree: &Path, branch: &str, push_url: &str, pat: &str) -> Result<()> {
    push_branch_with_executable(Path::new("git"), worktree, branch, push_url, pat)
}

fn push_branch_with_executable(
    executable: &Path,
    worktree: &Path,
    branch: &str,
    push_url: &str,
    pat: &str,
) -> Result<()> {
    let askpass = write_askpass(pat)?;
    let result = retry_transient_git_network("push", || {
        let child = Command::new(executable)
            .args(["push", "-q", push_url, branch])
            .env("GIT_ASKPASS", &askpass)
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(worktree)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("git push")?;
        let out = wait_with_timeout(child, "git push")?;
        if !out.status.success() {
            anyhow::bail!(
                "push failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    });
    let _ = std::fs::remove_file(&askpass);
    result
}

#[allow(dead_code)]
pub fn commit_and_push_msg(
    worktree: &Path,
    branch: &str,
    push_url: &str,
    msg: &str,
    pat: &str,
) -> Result<()> {
    stage_all(worktree)?;
    ensure_staged(worktree)?;
    commit_msg(worktree, msg)?;
    push_branch(worktree, branch, push_url, pat)
}

pub fn cleanup(worktree: &Path, repo: &Path) {
    let _ = git_raw(
        &["worktree", "remove", "-f", worktree.to_str().unwrap_or("")],
        repo,
    );
    let _ = git_raw(&["worktree", "prune"], repo);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
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
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = TempDir::new().unwrap();
        let count_path = tmp.path().join("push-count");
        let fake_git = tmp.path().join("git");
        fs::write(
            &fake_git,
            format!(
                "#!/bin/sh\ncount=0\n[ -f '{count}' ] && count=$(cat '{count}')\ncount=$((count + 1))\nprintf '%s' \"$count\" > '{count}'\nif [ \"$count\" -eq 1 ]; then echo 'ssh: connect to host github.com port 22: Connection timed out' >&2; exit 1; fi\nexit 0\n",
                count = count_path.display()
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&fake_git).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_git, perms).unwrap();

        push_branch_with_executable(&fake_git, tmp.path(), "main", "origin", "").unwrap();

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
}
