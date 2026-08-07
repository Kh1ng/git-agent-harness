//! Alert-only health check for `gah-loop@<profile>.service` systemd units.
//!
//! Issue #726: a previous host-local, untracked script silently restarted a
//! stopped loop by calling the dashboard's start endpoint, which resumed
//! concurrent work during a blocker repair and violated
//! `gah-loop@.service`'s own contract (`Restart=no` -- the operator/
//! dashboard starts work explicitly, nothing else may).
//!
//! This module is read-only by construction: the only external command it
//! ever runs is `systemctl --user show <unit> --property=... --value`, a
//! query that cannot mutate unit state. It never calls `systemctl
//! start`/`restart`/`enable`, `gah loop`, or any HTTP endpoint -- there is
//! no code path here capable of starting a loop, not just a policy choice
//! not to.

use crate::config;
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopHealth {
    /// Running, or never instantiated (no loop unit installed for this
    /// profile) -- nothing worth telling an operator.
    Healthy,
    /// Loaded, not active, exited cleanly (`SubState=dead`). Covers both an
    /// explicit manual stop and a clean failed-validation exit: systemd
    /// cannot distinguish the two on its own, and neither should ever
    /// trigger a restart, so both classify (and alert) the same way.
    StoppedCleanly,
    /// Loaded, `ActiveState=failed` -- a crash or non-zero exit.
    Crashed,
    /// Activating/deactivating/reloading -- transient, resolves itself
    /// within seconds; alerting on it would just be noise.
    Transitional,
}

pub(crate) fn unit_name(profile: &str) -> String {
    format!("gah-loop@{profile}.service")
}

pub(crate) fn classify(load_state: &str, active_state: &str, sub_state: &str) -> LoopHealth {
    if load_state == "not-found" {
        return LoopHealth::Healthy;
    }
    match active_state {
        "active" => LoopHealth::Healthy,
        "failed" => LoopHealth::Crashed,
        "inactive" if sub_state == "dead" => LoopHealth::StoppedCleanly,
        _ => LoopHealth::Transitional,
    }
}

fn query_unit_state(unit: &str) -> Result<LoopHealth> {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            unit,
            "--property=LoadState,ActiveState,SubState",
            "--value",
        ])
        .output()
        .with_context(|| format!("querying systemd state for {unit}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "systemctl --user show {unit} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let load_state = lines.next().unwrap_or_default();
    let active_state = lines.next().unwrap_or_default();
    let sub_state = lines.next().unwrap_or_default();
    Ok(classify(load_state, active_state, sub_state))
}

/// Compute the alert lines for the selected profiles (one named profile, or
/// every configured profile -- AC4). Separated from `run` so tests can
/// assert on the exact alert text without capturing process stdout. A
/// per-profile query error is logged and skipped, never propagated: one
/// unreachable systemd instance must not hide alerts for the rest.
pub(crate) fn collect_alerts(
    cfg: &config::GahConfig,
    profile_name: Option<&str>,
) -> Result<Vec<String>> {
    let profiles = crate::prune::selected_profiles(cfg, profile_name)?;
    let mut alerts = Vec::new();
    for (name, _profile) in profiles {
        let unit = unit_name(&name);
        match query_unit_state(&unit) {
            Ok(LoopHealth::StoppedCleanly) => alerts.push(format!(
                "[gah watchdog] {unit} is stopped (clean exit) -- not restarting automatically; verify this is intentional"
            )),
            Ok(LoopHealth::Crashed) => alerts.push(format!(
                "[gah watchdog] {unit} has FAILED (crashed) -- not restarting automatically; investigate and start it manually"
            )),
            Ok(LoopHealth::Healthy | LoopHealth::Transitional) => {}
            Err(err) => eprintln!("[gah watchdog] could not check {unit}: {err:#}"),
        }
    }
    Ok(alerts)
}

/// Print at most one alert line per profile whose loop is stopped or
/// crashed. Never fails on a single profile's query error -- this is a
/// health check, not a gate; it must not itself become a silent-failure
/// risk.
pub fn run(profile_name: Option<&str>, config_path: Option<&str>) -> Result<()> {
    let cfg = config::load(config_path)?;
    for alert in collect_alerts(&cfg, profile_name)? {
        println!("{alert}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
