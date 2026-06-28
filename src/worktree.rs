use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

pub fn has_changes(worktree: &Path) -> Result<bool> {
    let status = git_raw(&["status", "--porcelain"], worktree)?;
    if !status.stdout.is_empty() {
        return Ok(true);
    }
    let diff = git_raw(&["diff", "HEAD", "@{upstream}"], worktree)?;
    Ok(!diff.stdout.is_empty())
}

pub fn diff_patch(worktree: &Path, target_branch: &str) -> Result<String> {
    let origin_ref = format!("origin/{}", target_branch);
    Ok(
        String::from_utf8_lossy(&git_raw(&["diff", &origin_ref, "HEAD"], worktree)?.stdout)
            .to_string(),
    )
}

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
        let first = line.as_bytes().get(0).copied().unwrap_or(b' ');
        let second = line.as_bytes().get(1).copied().unwrap_or(b' ');
        if first != b' ' || second != b' ' {
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

    let staged = git_raw(&["diff", "--cached", "--name-only"], worktree)?;
    if staged.stdout.is_empty() {
        anyhow::bail!("nothing to commit after git add -A");
    }

    git(&["commit", "-q", "-m", msg], worktree)?;

    let out = git_raw(&["push", "-q", push_url, branch], worktree).context("git push")?;
    if !out.status.success() {
        anyhow::bail!(
            "push failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn cleanup(worktree: &Path, repo: &Path) {
    let _ = git_raw(
        &["worktree", "remove", "-f", worktree.to_str().unwrap_or("")],
        repo,
    );
    let _ = git_raw(&["worktree", "prune"], repo);
}
