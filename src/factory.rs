//! Host-local factory module policy. Missing configuration preserves legacy installs.
use anyhow::{bail, Context, Result};
use std::process::Command;

pub fn enabled(config_path: Option<&str>) -> Result<bool> {
    if !crate::config::resolve_config_path(config_path).exists() {
        return Ok(false);
    }
    Ok(crate::config::load(config_path)?
        .defaults
        .factory_enabled
        .unwrap_or(true))
}

pub fn require_enabled(config_path: Option<&str>) -> Result<()> {
    if !enabled(config_path)? {
        bail!("Factory automation is disabled. Enable it in this computer's Settings or with gah config set --factory-enabled true.");
    }
    Ok(())
}

fn factory_unit(name: &str) -> bool {
    (name.starts_with("gah-loop@") && name.ends_with(".service"))
        || matches!(name, "gah-watchdog.timer" | "gah-watchdog.service")
}

/// Enabling permits explicit loop starts; it never dispatches work by itself.
/// Disabling stops every installed/loaded loop instance and the factory watchdog.
/// Quota refresh and prune/chat maintenance are shared local application services.
pub fn apply_services(enabled: bool) -> Result<()> {
    if enabled || !cfg!(target_os = "linux") {
        return Ok(());
    }
    if !Command::new("systemctl")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return Ok(());
    }
    let mut units = std::collections::BTreeSet::new();
    for listing in ["list-units", "list-unit-files"] {
        let output = Command::new("systemctl")
            .args([
                "--user",
                listing,
                "--all",
                "--plain",
                "--no-legend",
                "--no-pager",
            ])
            .output()
            .context("listing factory services")?;
        if !output.status.success() {
            bail!(
                "Cannot list factory services: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        for name in String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| factory_unit(name))
        {
            units.insert(name.to_owned());
        }
    }
    for unit in units {
        // Templates cannot be stopped; disable them to prevent future boot starts.
        let action = if unit == "gah-loop@.service" {
            vec!["--user", "disable", unit.as_str()]
        } else {
            vec!["--user", "disable", "--now", unit.as_str()]
        };
        let status = Command::new("systemctl").args(action).status()?;
        if !status.success() {
            bail!("Cannot disable factory service {unit}: {status}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_factory_services_are_selected() {
        for name in [
            "gah-loop@.service",
            "gah-loop@project.service",
            "gah-watchdog.timer",
            "gah-watchdog.service",
        ] {
            assert!(factory_unit(name));
        }
        for name in [
            "gah-server.service",
            "gah-worker.service",
            "gah-prune.timer",
            "gah-quota-refresh.timer",
            "tdai-memory-gateway.service",
        ] {
            assert!(!factory_unit(name));
        }
    }

    #[test]
    fn fresh_and_legacy_configuration_defaults_differ() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let path = path.to_str().unwrap();
        assert!(!enabled(Some(path)).unwrap());
        std::fs::write(path, "[defaults]\n").unwrap();
        assert!(enabled(Some(path)).unwrap());
        std::fs::write(path, "[defaults]\nfactory_enabled = false\n").unwrap();
        assert!(require_enabled(Some(path)).is_err());
        std::fs::write(path, "[defaults]\nfactory_enabled = true\n").unwrap();
        assert!(require_enabled(Some(path)).is_ok());
    }
}
