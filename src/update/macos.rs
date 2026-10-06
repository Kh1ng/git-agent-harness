use super::{run_command, HostRole};
use anyhow::{bail, Context, Result};
use std::env;
use std::path::{Path, PathBuf};

/// Install the one role-appropriate macOS service definition from the same
/// updater used by first install and the desktop role control.
pub(super) fn install_macos_launch_agent(repo: &Path, role: HostRole) -> Result<Option<PathBuf>> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let script = repo.join("scripts/macos-launchd.sh");
    if !script.is_file() {
        bail!("macOS launchd installer is missing: {}", script.display());
    }
    let profile = if role == HostRole::Worker {
        crate::config::load(None)
            .ok()
            .and_then(|config| {
                let mut names: Vec<String> = config.profiles.into_keys().collect();
                names.sort_unstable();
                names.into_iter().next()
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let role_name = match role {
        HostRole::Central => "central",
        HostRole::Standalone => "standalone",
        HostRole::Worker => "worker",
    };
    run_command(
        repo,
        "bash",
        &[
            script.to_string_lossy().as_ref(),
            "install",
            role_name,
            repo.to_string_lossy().as_ref(),
            &profile,
        ],
    )?;
    let label = match role {
        HostRole::Central | HostRole::Standalone => "dev.git-agent-harness.server.plist",
        HostRole::Worker => "dev.git-agent-harness.worker.plist",
    };
    let target = env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is required to install a macOS LaunchAgent")?
        .join("Library/LaunchAgents")
        .join(label);
    Ok(target.is_file().then_some(target))
}
