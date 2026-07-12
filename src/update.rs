//! Deterministic CLI and control-plane server update workflow.
//!
//! `cargo build --release` only updates `target/release/gah`. The command a
//! host actually invokes normally lives at `$CARGO_HOME/bin/gah`, so a normal
//! build can silently leave the control plane on old behavior.

use anyhow::{bail, Context, Result};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct UpdateArgs {
    pub repo: Option<PathBuf>,
    pub restart_server: bool,
    pub server_service: String,
}

pub fn run(args: UpdateArgs) -> Result<()> {
    let repo = resolve_repo(args.repo.as_deref())?;
    ensure_default_branch_checkout(&repo)?;
    ensure_clean(&repo)?;

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
    )?;
    let default_branch = default_ref.strip_prefix("origin/").unwrap_or(&default_ref);
    if branch != default_branch {
        bail!(
            "refusing to update non-default branch '{branch}' (origin default is '{default_branch}'); switch to the default branch first"
        );
    }
    Ok(())
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
    use super::installed_binary_path;
    use std::path::Path;

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
}
