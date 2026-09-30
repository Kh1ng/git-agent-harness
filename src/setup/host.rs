//! What `gah setup` asks of the machine: which OS and package manager it
//! has, whether a program runs, and what it prints. Tests substitute a fake.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Linux,
    Macos,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageManager {
    Brew,
    Apt,
    Dnf,
    Pacman,
}

/// One finished command: exit status and combined output.
#[derive(Debug, Clone)]
pub struct Probe {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait Host {
    fn os(&self) -> Os;
    fn package_manager(&self) -> Option<PackageManager>;
    /// Runs a program briefly; `None` when it is not installed or hangs.
    fn probe(&self, program: &str, args: &[&str]) -> Option<Probe>;
    fn exists(&self, path: &std::path::Path) -> bool;
    fn env(&self, key: &str) -> Option<String>;
}

pub struct SystemHost;

impl Host for SystemHost {
    fn os(&self) -> Os {
        match std::env::consts::OS {
            "linux" => Os::Linux,
            "macos" => Os::Macos,
            _ => Os::Other,
        }
    }

    fn package_manager(&self) -> Option<PackageManager> {
        let on_path =
            |name: &str| crate::runner::resolve::resolve_executable_on_path(name).is_some();
        if on_path("brew") {
            Some(PackageManager::Brew)
        } else if on_path("apt-get") {
            Some(PackageManager::Apt)
        } else if on_path("dnf") {
            Some(PackageManager::Dnf)
        } else if on_path("pacman") {
            Some(PackageManager::Pacman)
        } else {
            None
        }
    }

    fn probe(&self, program: &str, args: &[&str]) -> Option<Probe> {
        let executable = crate::runner::resolve::resolve_executable_on_path(program)?;
        let mut command = Command::new(executable);
        command.args(args);
        let output = crate::runner::process::run_bounded(command, Duration::from_secs(20))?;
        Some(Probe {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn exists(&self, path: &std::path::Path) -> bool {
        path.exists()
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|value| !value.is_empty())
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// A user-level install (rustup, nvm, a Homebrew cask) lands in a directory
/// this process's PATH does not include yet. Prepend the known ones that now
/// exist, so the next check and the service installer find the new program.
pub fn refresh_path() {
    let home = home();
    let mut candidates = vec![
        home.join(".cargo/bin"),
        home.join(".local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
        let mut versions: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        versions.sort();
        if let Some(latest) = versions.pop() {
            candidates.insert(0, latest.join("bin"));
        }
    }
    let current: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let mut next: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|dir| dir.is_dir() && !current.contains(dir))
        .collect();
    if next.is_empty() {
        return;
    }
    next.extend(current);
    if let Ok(joined) = std::env::join_paths(next) {
        std::env::set_var("PATH", joined);
    }
}

/// Parses the first `major.minor` in a version string ("v22.4.1", "git
/// version 2.43.0").
pub fn version(text: &str) -> Option<(u32, u32)> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let mut parts = text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty());
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_from_common_outputs() {
        assert_eq!(version("v22.4.1"), Some((22, 4)));
        assert_eq!(version("git version 2.43.0"), Some((2, 43)));
        assert_eq!(
            version("cargo 1.94.1 (29ea6fb6a 2026-03-24)"),
            Some((1, 94))
        );
        assert_eq!(version("no digits"), None);
    }
}
