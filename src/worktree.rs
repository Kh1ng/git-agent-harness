use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

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
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run git and return raw Output. Does NOT error on non-zero exit.
pub fn git_raw(args: &[&str], cwd: &Path) -> Result<std::process::Output> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    Ok(out)
}

pub fn create(
    repo: &Path,
    target_branch: &str,
    new_branch: &str,
    worktree_base: &Path,
) -> Result<PathBuf> {
    git(&["fetch", "-q", "origin", "--prune"], repo)?;

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

pub fn push_branch(worktree: &Path, branch: &str, push_url: &str, pat: &str) -> Result<()> {
    let askpass = write_askpass(pat)?;
    let out = Command::new("git")
        .args(["push", "-q", push_url, branch])
        .env("GIT_ASKPASS", &askpass)
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(worktree)
        .output()
        .context("git push")?;
    let _ = std::fs::remove_file(&askpass);
    if !out.status.success() {
        anyhow::bail!(
            "push failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
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
    fn push_branch_fails_loudly_for_unreachable_remote() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_bare_repo_with_main(&repo);
        let bogus_remote = tmp.path().join("does-not-exist-as-a-remote");

        let err = push_branch(&repo, "main", bogus_remote.to_str().unwrap(), "").unwrap_err();

        assert!(format!("{:#}", err).contains("push failed"));
    }
}
