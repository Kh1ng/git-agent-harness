//! The first project comes from a checkout the user already has: its
//! `origin` remote names the provider and repository, and its remote HEAD
//! names the target branch, so setup asks for a path instead of eight flags.

use super::requirements::Provider;
use crate::init::InitArgs;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub local_path: PathBuf,
    /// `None` for a host that is neither github.com nor recognizably GitLab.
    pub provider: Option<Provider>,
    pub host: String,
    /// `owner/name` (GitLab may have nested groups).
    pub repo: String,
    pub default_branch: String,
}

/// Splits a remote URL into host and repository path. The shared
/// normalizer lowercases; the repository keeps the case the URL gives it.
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let normalized = crate::memory_gateway::normalize_remote_url(url);
    let (host, repo) = normalized.split_once('/')?;
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() || !repo.contains('/') {
        return None;
    }
    let url = url.trim();
    let repo = url
        .to_lowercase()
        .rfind(repo)
        .and_then(|start| url.get(start..start + repo.len()))
        .unwrap_or(repo);
    Some((host.to_string(), repo.to_string()))
}

pub fn provider_for_host(host: &str) -> Option<Provider> {
    if host == "github.com" {
        Some(Provider::Github)
    } else if host == "gitlab.com" || host.split('.').any(|label| label == "gitlab") {
        Some(Provider::Gitlab)
    } else {
        None
    }
}

fn git(path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

pub fn inspect(path: &Path) -> Result<Checkout> {
    let local_path = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    if git(&local_path, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        bail!("{} is not a git checkout", local_path.display());
    }
    let remote = git(&local_path, &["remote", "get-url", "origin"])
        .context("This checkout has no `origin` remote. Add one (git remote add origin <url>) and run setup again.")?;
    let (host, repo) = parse_remote(&remote)
        .with_context(|| format!("Cannot read a host and repository from origin {remote}"))?;
    let default_branch = git(
        &local_path,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .and_then(|reference| reference.strip_prefix("origin/").map(str::to_string))
    // A clone without origin/HEAD usually still has the usual trunk names;
    // the branch checked out now may be a feature branch.
    .or_else(|| {
        ["main", "master"].into_iter().find_map(|branch| {
            git(
                &local_path,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/origin/{branch}"),
                ],
            )
            .map(|_| branch.to_string())
        })
    })
    .or_else(|| {
        // Unlike rev-parse, this also names a branch that has no commits yet.
        git(&local_path, &["symbolic-ref", "--short", "HEAD"])
    })
    .unwrap_or_else(|| "main".into());
    Ok(Checkout {
        provider: provider_for_host(&host),
        local_path,
        host,
        repo,
        default_branch,
    })
}

/// A profile name GAH accepts: the repository's last path segment, lowercase.
pub fn profile_name(repo: &str) -> String {
    let name: String = repo
        .rsplit('/')
        .next()
        .unwrap_or(repo)
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        "project".into()
    } else {
        name
    }
}

pub fn init_args(checkout: &Checkout, provider: Provider, profile: &str) -> InitArgs {
    InitArgs {
        profile: profile.to_string(),
        display_name: checkout
            .repo
            .rsplit('/')
            .next()
            .unwrap_or(&checkout.repo)
            .to_string(),
        provider: match provider {
            Provider::Github => "github".into(),
            Provider::Gitlab => "gitlab".into(),
        },
        repo: checkout.repo.clone(),
        local_path: checkout.local_path.display().to_string(),
        default_target_branch: checkout.default_branch.clone(),
        provider_api_base: (provider == Provider::Gitlab)
            .then(|| format!("https://{}/api/v4", checkout.host)),
        provider_project_id: None,
        artifact_root: None,
        worktree_base: None,
        oh_profile: None,
        config_path: None,
        print: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_name_host_and_repository() {
        assert_eq!(
            parse_remote("git@github.com:Kh1ng/git-agent-harness.git"),
            Some(("github.com".into(), "Kh1ng/git-agent-harness".into()))
        );
        assert_eq!(
            parse_remote("https://gitlab.example.com:8443/group/sub/app"),
            Some(("gitlab.example.com".into(), "group/sub/app".into()))
        );
        assert_eq!(
            parse_remote("https://token@github.com/o/r"),
            Some(("github.com".into(), "o/r".into()))
        );
        assert_eq!(parse_remote("not a url"), None);
    }

    #[test]
    fn providers_come_from_the_host() {
        assert_eq!(provider_for_host("github.com"), Some(Provider::Github));
        assert_eq!(provider_for_host("gitlab.com"), Some(Provider::Gitlab));
        assert_eq!(
            provider_for_host("gitlab.corp.example"),
            Some(Provider::Gitlab)
        );
        assert_eq!(provider_for_host("git.example.com"), None);
    }

    #[test]
    fn a_real_checkout_becomes_a_profile() {
        let root = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success())
        };
        run(&["init", "-q", "-b", "trunk"]);
        run(&[
            "remote",
            "add",
            "origin",
            "https://gitlab.example.com/team/Web-App.git",
        ]);
        let checkout = inspect(root.path()).unwrap();
        assert_eq!(checkout.provider, Some(Provider::Gitlab));
        assert_eq!(
            checkout.default_branch, "trunk",
            "no remote HEAD yet: the current branch"
        );
        let args = init_args(&checkout, Provider::Gitlab, &profile_name(&checkout.repo));
        assert_eq!(
            (args.profile.as_str(), args.display_name.as_str()),
            ("web-app", "Web-App")
        );
        assert_eq!(
            args.provider_api_base.as_deref(),
            Some("https://gitlab.example.com/api/v4")
        );
        assert!(inspect(&root.path().join("missing")).is_err());
    }
}
