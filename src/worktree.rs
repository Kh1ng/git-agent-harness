use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn git(args: &[&str], cwd: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        // Prevent user git config (color, pager, aliases) from corrupting output
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

pub fn has_changes(worktree: &Path) -> Result<bool> {
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree)
        .output()?;
    if !status.stdout.is_empty() {
        return Ok(true);
    }
    // Check committed but not yet on origin
    let diff = Command::new("git")
        .args(["diff", "HEAD", "@{upstream}"])
        .current_dir(worktree)
        .output()?;
    Ok(!diff.stdout.is_empty())
}

pub fn diff_patch(worktree: &Path, target_branch: &str) -> Result<String> {
    let origin_ref = format!("origin/{}", target_branch);
    let out = Command::new("git")
        .args(["diff", &origin_ref, "HEAD"])
        .current_dir(worktree)
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn changed_files(worktree: &Path, target_branch: &str) -> Result<Vec<String>> {
    let origin_ref = format!("origin/{}", target_branch);
    // Tracked diff
    let tracked = Command::new("git")
        .args(["diff", "--name-only", &origin_ref, "HEAD"])
        .current_dir(worktree)
        .output()?;
    let mut files: Vec<String> = String::from_utf8_lossy(&tracked.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    // Untracked
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree)
        .output()?;
    for line in String::from_utf8_lossy(&status.stdout).lines() {
        if line.starts_with("??") {
            files.push(line[3..].trim().to_string());
        }
    }
    Ok(files)
}

pub fn commit_and_push(worktree: &Path, branch: &str, push_url: &str, repo_id: &str) -> Result<()> {
    commit_and_push_msg(
        worktree,
        branch,
        push_url,
        &format!("gah: improve mode changes for {}", repo_id),
    )
}

pub fn commit_and_push_msg(worktree: &Path, branch: &str, push_url: &str, msg: &str) -> Result<()> {
    git(&["add", "-A"], worktree)?;

    let staged = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(worktree)
        .output()?;
    if staged.stdout.is_empty() {
        anyhow::bail!("nothing to commit after git add -A");
    }

    git(&["commit", "-q", "-m", msg], worktree)?;

    let out = Command::new("git")
        .args(["push", "-q", push_url, branch])
        .current_dir(worktree)
        .output()
        .context("git push")?;
    if !out.status.success() {
        anyhow::bail!(
            "push failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn cleanup(worktree: &Path, repo: &Path) {
    let _ = Command::new("git")
        .args(["worktree", "remove", "-f", worktree.to_str().unwrap_or("")])
        .current_dir(repo)
        .output();
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo)
        .output();
}
