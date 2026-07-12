//! Deterministic CLI and control-plane server update workflow.
//!
//! `cargo build --release` only updates `target/release/gah`. The command a
//! host actually invokes normally lives at `$CARGO_HOME/bin/gah`, so a normal
//! build can silently leave the control plane on old behavior.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::env;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct UpdateArgs {
    pub repo: Option<PathBuf>,
    pub restart_server: bool,
    pub server_service: String,
}

pub fn run(args: UpdateArgs) -> Result<()> {
    let repo = resolve_repo(args.repo.as_deref())?;
    let _update_lock = acquire_update_lock(&repo)?;
    ensure_default_branch_checkout(&repo)?;
    ensure_clean(&repo)?;
    if args.restart_server {
        ensure_no_running_loop_before_server_restart()?;
    }

    println!("Updating GAH CLI/control plane from {}", repo.display());
    run_command(&repo, "git", &["fetch", "origin", "--prune"])?;
    run_command(&repo, "git", &["pull", "--ff-only"])?;

    // This is the authoritative CLI deployment step. It replaces the Cargo
    // executable selected by PATH, unlike a target/release-only build.
    run_command(&repo, "cargo", &["install", "--path", ".", "--force"])?;
    // The control-plane server is part of the MVP; web/desktop/mobile clients
    // intentionally have independent release workflows.
    run_command(
        &repo,
        "npm",
        &[
            "ci",
            "--include=dev",
            "--legacy-peer-deps",
            "--prefer-offline",
            "--no-audit",
            "--no-fund",
        ],
    )?;
    run_command(&repo, "npm", &["run", "build:server"])?;

    let binary = installed_binary_path()?;
    if !binary.is_file() {
        bail!(
            "cargo install completed but expected executable is missing: {}",
            binary.display()
        );
    }
    if !repo.join("apps/server/dist/bin.js").is_file() {
        bail!("server build did not produce apps/server/dist/bin.js");
    }
    run_command(&repo, binary.to_string_lossy().as_ref(), &["--help"])?;

    println!("Installed CLI: {}", binary.display());
    println!(
        "Built server:  {}",
        repo.join("apps/server/dist/bin.js").display()
    );

    if args.restart_server {
        run_command(&repo, "sudo", &["systemctl", "daemon-reload"])?;
        run_command(
            &repo,
            "sudo",
            &["systemctl", "restart", &args.server_service],
        )?;
        run_command(
            &repo,
            "systemctl",
            &["is-active", "--quiet", &args.server_service],
        )?;
        println!("Restarted service: {}", args.server_service);
    } else {
        println!(
            "Server not restarted; pass --restart-server when this host serves the control plane."
        );
    }
    Ok(())
}

fn resolve_repo(repo: Option<&Path>) -> Result<PathBuf> {
    let path = repo
        .map(Path::to_path_buf)
        .unwrap_or(env::current_dir().context("reading current directory")?);
    let output = Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("resolving repository root")?;
    if !output.status.success() {
        bail!(
            "{} is not a Git checkout: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(PathBuf::from(
        String::from_utf8(output.stdout)
            .context("repository root was not UTF-8")?
            .trim(),
    ))
}

fn ensure_default_branch_checkout(repo: &Path) -> Result<()> {
    let branch = captured(repo, "git", &["branch", "--show-current"])?;
    let default_ref = captured(
        repo,
        "git",
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "cannot determine origin's default branch; run `git remote set-head origin -a` in {} before updating",
            repo.display()
        )
    })?;
    let default_branch = default_ref.strip_prefix("origin/").unwrap_or(&default_ref);
    if branch != default_branch {
        bail!(
            "refusing to update non-default branch '{branch}' (origin default is '{default_branch}'); switch to the default branch first"
        );
    }
    Ok(())
}

/// Serialize all mutable update steps (`git pull`, `cargo install`, and `npm
/// ci`) for one checkout. Unlike a profile lock this is deliberately repo-wide:
/// two operators updating the same source tree would otherwise race on Git and
/// dependency state before any GAH profile exists.
fn acquire_update_lock(repo: &Path) -> Result<File> {
    let git_dir = captured(repo, "git", &["rev-parse", "--absolute-git-dir"])?;
    let lock_path = PathBuf::from(git_dir).join("gah-update.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening update lock {}", lock_path.display()))?;
    lock.try_lock_exclusive().map_err(|_| {
        anyhow::anyhow!(
            "another `gah update` is already running for {}; wait for it to finish",
            repo.display()
        )
    })?;
    Ok(lock)
}

/// A dashboard-started loop remains in `gah-server.service`'s cgroup even
/// though Node detaches it. Restarting that service would kill the loop, so
/// fail closed before mutating the installation. The server itself uses the
/// same `gah loop --profile …` process shape for its status fallback.
fn ensure_no_running_loop_before_server_restart() -> Result<()> {
    let output = Command::new("pgrep")
        .args(["-af", "gah loop --profile"])
        .output()
        .context("checking for active gah loops before server restart")?;
    if output.status.code() == Some(1) {
        return Ok(());
    }
    if !output.status.success() {
        bail!(
            "could not determine whether a gah loop is active before restarting the server: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let processes = String::from_utf8_lossy(&output.stdout).trim().to_string();
    bail!(
        "refusing to restart gah-server.service while a gah loop is active:\n{processes}\nStop the loop cleanly from the dashboard or `gah loop` owner, then rerun `gah update --restart-server`."
    );
}

fn ensure_clean(repo: &Path) -> Result<()> {
    let status = captured(repo, "git", &["status", "--porcelain"])?;
    if !status.is_empty() {
        bail!(
            "refusing to update a dirty checkout; commit, stash, or move these changes first:\n{status}"
        );
    }
    Ok(())
}

fn installed_binary_path() -> Result<PathBuf> {
    let cargo_home = match env::var_os("CARGO_HOME") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(
            env::var_os("HOME").context("HOME is required to locate Cargo-installed gah")?,
        )
        .join(".cargo"),
    };
    Ok(cargo_home.join("bin").join("gah"))
}

fn captured(repo: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("starting {program} {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .context("command output was not UTF-8")
}

fn run_command(repo: &Path, program: &str, args: &[&str]) -> Result<()> {
    println!("> {} {}", program, args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(repo)
        .status()
        .with_context(|| format!("starting {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("{program} {} exited with {status}", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ensure_clean, ensure_default_branch_checkout, installed_binary_path};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository_with_origin_head() -> (TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "initial"]);
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        (tmp, repo)
    }

    #[test]
    fn installed_binary_lives_in_cargo_bin() {
        let path = installed_binary_path().unwrap();
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("gah"));
        assert_eq!(
            path.parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            Some("bin")
        );
    }

    #[test]
    fn default_branch_check_accepts_origin_default_and_rejects_feature_branch() {
        let (_tmp, repo) = repository_with_origin_head();
        ensure_default_branch_checkout(&repo).unwrap();
        git(&repo, &["checkout", "-b", "feature"]);
        let err = ensure_default_branch_checkout(&repo).unwrap_err();
        assert!(err.to_string().contains("non-default branch 'feature'"));
    }

    #[test]
    fn default_branch_check_explains_missing_origin_head() {
        let (_tmp, repo) = repository_with_origin_head();
        git(&repo, &["symbolic-ref", "-d", "refs/remotes/origin/HEAD"]);
        let err = ensure_default_branch_checkout(&repo).unwrap_err();
        assert!(err.to_string().contains("git remote set-head origin -a"));
    }

    #[test]
    fn clean_check_rejects_uncommitted_changes() {
        let (_tmp, repo) = repository_with_origin_head();
        ensure_clean(&repo).unwrap();
        std::fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();
        let err = ensure_clean(&repo).unwrap_err();
        assert!(err.to_string().contains("dirty.txt"));
    }
}
