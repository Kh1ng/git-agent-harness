// Command execution for `gah network-expose` (issue #879).

use crate::config;
use crate::network_exposure::{apply, required_cidrs, NetworkExposureLevel};
use anyhow::{Context, Result};

pub struct Args {
    pub port: u16,
    pub label: String,
    pub level: Option<String>,
    pub config_path: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    let cfg = config::load(args.config_path.as_deref())?;
    let level = match args.level.as_deref() {
        Some(raw) => NetworkExposureLevel::parse(raw).with_context(|| {
            format!("unrecognized --level '{raw}' (expected loopback|lan|lan_tailscale)")
        })?,
        None => cfg.defaults.network_exposure,
    };
    let cidrs = required_cidrs(&cfg.defaults, level);
    if cidrs.is_empty() {
        println!(
            "network-expose: level={level:?} requires no firewall rule for port {} ({})",
            args.port, args.label
        );
    } else {
        apply(args.port, &args.label, &cidrs)?;
    }
    println!(
        "Recommended bind host for '{}' at this level: {}",
        args.label,
        level.recommended_bind_host()
    );
    Ok(())
}
