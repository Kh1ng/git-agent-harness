//! The macOS LaunchAgent a role runs under, installed by the same updater
//! used by first install and the desktop role control.
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
    let role_name = macos_launch_agent_role(role);
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

fn macos_launch_agent_role(role: HostRole) -> &'static str {
    match role {
        // Standalone hosts run the same control-plane service as central hosts.
        HostRole::Central | HostRole::Standalone => "central",
        HostRole::Worker => "worker",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn macos_launch_agent_roles_match_the_installer_contract() {
        for (role, expected) in [
            (HostRole::Central, "central"),
            (HostRole::Standalone, "central"),
            (HostRole::Worker, "worker"),
        ] {
            let installer_role = macos_launch_agent_role(role);
            assert_eq!(installer_role, expected);
            // Exercise the script's role validation without changing host services.
            let output = Command::new("bash")
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/macos-launchd.sh"))
                .args(["status", installer_role])
                .env("GAH_LAUNCHD_DRY_RUN", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "installer rejected {role:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
