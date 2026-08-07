//! Issue #879: one configuration surface for "expose this gah-managed
//! service beyond loopback" instead of hand-run, per-service `ufw`
//! sessions with no shared record of intent. Two occurrences of the exact
//! same manual pattern (the memory gateway's port, then the dashboard's)
//! was the signal this needed to stop being ad hoc.
//!
//! Scope is deliberately narrow: this module computes which CIDRs an
//! exposure level allows and applies the corresponding `ufw` rules
//! idempotently. It does not rewrite other services' own bind-host config
//! (a third-party service like the memory gateway owns its own config
//! file) -- callers get a bind-host recommendation to apply themselves.

use crate::config::Defaults;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkExposureLevel {
    /// No firewall rule; the service should bind loopback-only. Safe
    /// default -- nothing becomes reachable without an explicit opt-in.
    #[default]
    Loopback,
    /// Reachable from `Defaults::lan_cidrs`.
    Lan,
    /// Reachable from `Defaults::lan_cidrs` and `Defaults::tailscale_cidr`.
    LanTailscale,
}

impl NetworkExposureLevel {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "loopback" => Some(Self::Loopback),
            "lan" => Some(Self::Lan),
            "lan_tailscale" | "lan+tailscale" => Some(Self::LanTailscale),
            _ => None,
        }
    }

    /// The bind host a service at this exposure level should use. Advisory
    /// only -- this module can't rewrite a third-party service's own
    /// config, so callers (or the operator) apply it there.
    pub fn recommended_bind_host(self) -> &'static str {
        match self {
            Self::Loopback => "127.0.0.1",
            Self::Lan | Self::LanTailscale => "0.0.0.0",
        }
    }
}

/// Resolve an exposure level into the concrete CIDRs it allows, given the
/// operator's configured LAN/tailscale ranges. `Loopback` always resolves
/// to no CIDRs (nothing to firewall open).
pub fn required_cidrs(defaults: &Defaults, level: NetworkExposureLevel) -> Vec<String> {
    match level {
        NetworkExposureLevel::Loopback => Vec::new(),
        NetworkExposureLevel::Lan => defaults.lan_cidrs.clone(),
        NetworkExposureLevel::LanTailscale => {
            let mut cidrs = defaults.lan_cidrs.clone();
            if let Some(tailscale) = &defaults.tailscale_cidr {
                cidrs.push(tailscale.clone());
            }
            cidrs
        }
    }
}

/// Existing `ufw` rules for a port, as `(source_cidr, port)` pairs parsed
/// from `ufw status`. Used to keep `apply` idempotent -- re-running with
/// the same config must not duplicate rules.
fn existing_allowed_cidrs(port: u16) -> Result<Vec<String>> {
    let output = Command::new("sudo")
        .args(["-n", "ufw", "status"])
        .output()
        .context("running sudo ufw status")?;
    if !output.status.success() {
        anyhow::bail!(
            "ufw status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let port_prefix = format!("{port}/tcp");
    let mut cidrs = Vec::new();
    for line in text.lines() {
        // Plain `ufw status` lines look like "8420/tcp  ALLOW  192.168.5.0/24  # comment";
        // `ufw status verbose` inserts "IN" after ALLOW. Don't assume a fixed
        // column index (a real bug here: `nth(3)` matched the verbose shape
        // and silently found nothing against plain `ufw status`, only masked
        // because `ufw allow` is itself idempotent) -- find the first token
        // that looks like a CIDR instead.
        if !line.trim_start().starts_with(&port_prefix) {
            continue;
        }
        if let Some(cidr) = line
            .split_whitespace()
            .skip(1) // skip the "<port>/tcp" column itself, which also contains '/'
            .find(|token| {
                token.contains('/') && token.chars().next().is_some_and(|c| c.is_ascii_digit())
            })
        {
            cidrs.push(cidr.to_string());
        }
    }
    Ok(cidrs)
}

/// Apply the firewall side of an exposure level for one port: allow every
/// CIDR in `cidrs` that isn't already allowed, skip the rest. Never
/// removes existing rules -- narrowing exposure is a deliberate, separate
/// operator action, not an automatic side effect of re-running this.
///
/// Inherits the parent's stdio (not piped) so an interactive `sudo`
/// password prompt works when passwordless `sudo ufw` isn't configured --
/// this command isn't assumed to run unattended.
pub fn apply(port: u16, label: &str, cidrs: &[String]) -> Result<()> {
    let already_allowed = existing_allowed_cidrs(port)?;
    for cidr in cidrs {
        if already_allowed.iter().any(|existing| existing == cidr) {
            println!("ufw: {cidr} -> {port}/tcp already allowed, skipping");
            continue;
        }
        let comment = format!("{label} ({cidr})");
        let status = Command::new("sudo")
            .args([
                "ufw",
                "allow",
                "from",
                cidr,
                "to",
                "any",
                "port",
                &port.to_string(),
                "proto",
                "tcp",
                "comment",
                &comment,
            ])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("running sudo ufw allow")?;
        if !status.success() {
            anyhow::bail!("sudo ufw allow from {cidr} to any port {port} exited with {status}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
