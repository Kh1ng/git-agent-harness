//! Host-local factory module policy.
use anyhow::{bail, Context, Result};
use std::process::Command;

/// Whether this host may run factory loops. A config written before the
/// module existed has no `factory_enabled` key and stays on. No config file
/// at all is a host that has not been set up, which is off.
pub fn enabled(config_path: Option<&str>) -> Result<bool> {
    if !crate::config::resolve_config_path(config_path).exists() {
        return Ok(false);
    }
    Ok(crate::config::load(config_path)?
        .defaults
        .factory_enabled
        .unwrap_or(true))
}

/// True only when a readable config turns the module off. `enabled` is also
/// false for a missing config and fails for an unreadable one; neither says
/// the operator switched the factory off, so a caller that stops services on
/// its own initiative (`gah update`) must ask this instead.
pub fn disabled_by_config(config_path: Option<&str>) -> bool {
    crate::config::resolve_config_path(config_path).exists()
        && matches!(enabled(config_path), Ok(false))
}

/// `gah update` has just reinstalled the loop and watchdog units. When this
/// host's config turns the module off, stop them again. A host with no config
/// at the default path, or one that does not parse, keeps its loops. A
/// service-control failure is a warning and never aborts the update:
/// `gah loop` refuses to start while the module is off.
pub fn keep_services_off_after_update() {
    if !disabled_by_config(None) {
        return;
    }
    match apply_services(false) {
        Ok(()) => {
            println!("Factory automation disabled: loop and watchdog services remain inactive.")
        }
        Err(error) => eprintln!(
            "warning: factory automation is disabled but its services could not be stopped: {error:#}. \
             Run `gah config set --factory-enabled false` again once systemd is reachable."
        ),
    }
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
        // Off, but not switched off: `gah update` must leave services alone.
        assert!(!disabled_by_config(Some(path)));
        std::fs::write(path, "[defaults\n").unwrap();
        assert!(enabled(Some(path)).is_err());
        assert!(!disabled_by_config(Some(path)));
        std::fs::write(path, "[defaults]\n").unwrap();
        assert!(enabled(Some(path)).unwrap());
        assert!(!disabled_by_config(Some(path)));
        std::fs::write(path, "[defaults]\nfactory_enabled = false\n").unwrap();
        assert!(disabled_by_config(Some(path)));
        assert!(require_enabled(Some(path)).is_err());
        std::fs::write(path, "[defaults]\nfactory_enabled = true\n").unwrap();
        assert!(require_enabled(Some(path)).is_ok());
    }
}
