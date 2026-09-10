// Command execution for `gah network-expose` (issue #879).

use crate::config;
use crate::network_exposure::{
    apply, default_tailscale_cidr, required_cidrs, NetworkExposureLevel,
};
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

/// Issue #943: resolved tailnet address for this device.
pub struct TailscaleIpArgs {
    pub config_path: Option<String>,
    pub json: bool,
}

/// Parse one dotted-quad IPv4 address.
fn parse_ipv4(raw: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut parts = raw.trim().split('.');
    for octet in octets.iter_mut() {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *octet = part.parse().ok()?;
    }
    parts.next().is_none().then_some(octets)
}

/// Whether `ip` falls inside `cidr` (e.g. "100.64.0.0/10"). The tailscale
/// range is the fixed CGNAT block 100.64.0.0/10 by default; operators with
/// unusual setups override it via `[defaults].tailscale_cidr`.
fn is_ipv4_in_cidr(ip: &str, cidr: &str) -> bool {
    let Some((base, prefix_raw)) = cidr.split_once('/') else {
        return false;
    };
    let Ok(prefix) = prefix_raw.parse::<u32>() else {
        return false;
    };
    if prefix > 32 {
        return false;
    }
    let (Some(ip_octets), Some(base_octets)) = (parse_ipv4(ip), parse_ipv4(base)) else {
        return false;
    };
    let ip_bits = u32::from_be_bytes(ip_octets);
    let base_bits = u32::from_be_bytes(base_octets);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (ip_bits & mask) == (base_bits & mask)
}

/// Collect candidate IPv4s from `ip -4 -o addr show` (Linux) and
/// `ifconfig` (macOS/BSD): one address per line, no scopes, no casts.
fn scan_interface_addresses() -> Vec<String> {
    let mut addresses = Vec::new();
    if let Ok(output) = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show"])
        .output()
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                // "2: eth0    inet 100.118.97.79/32 scope global ..."
                if let Some(field) = line.split_whitespace().nth(3) {
                    let address = field.split('/').next().unwrap_or(field);
                    if parse_ipv4(address).is_some() {
                        addresses.push(address.to_string());
                    }
                }
            }
        }
    }
    if let Ok(output) = std::process::Command::new("ifconfig").output() {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                // "        inet 100.118.97.79 netmask 0xfffff000 ..."
                let fields = line.split_whitespace().collect::<Vec<_>>();
                if fields.first() == Some(&"inet")
                    && fields
                        .get(1)
                        .is_some_and(|address| parse_ipv4(address).is_some())
                {
                    addresses.push(fields[1].to_string());
                }
            }
        }
    }
    addresses
}

pub fn tailscale_ip(args: TailscaleIpArgs) -> Result<()> {
    let cfg = config::load(args.config_path.as_deref())?;
    let cidr = cfg
        .defaults
        .tailscale_cidr
        .clone()
        .or_else(default_tailscale_cidr)
        .unwrap_or_else(|| "100.64.0.0/10".to_string());

    // 1. Authoritative: the tailscale CLI knows its own address even when
    //    interface naming or routing is exotic.
    let mut candidates = Vec::new();
    if let Ok(output) = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let address = line.trim();
                if parse_ipv4(address).is_some() {
                    candidates.push(address.to_string());
                }
            }
        }
    }
    // 2. Fallback: scan local interfaces for an address inside the tailnet
    //    range (covers installs where `tailscale` is not on PATH but the
    //    interface is up, e.g. containers sharing the host tailnet).
    for address in scan_interface_addresses() {
        if !candidates.contains(&address) {
            candidates.push(address);
        }
    }

    let Some(ip) = candidates
        .iter()
        .find(|address| is_ipv4_in_cidr(address, &cidr))
    else {
        anyhow::bail!(
            "no tailnet IPv4 address found for CIDR {cidr}; is this device joined to a tailnet (`tailscale up`)?"
        );
    };

    if args.json {
        println!(
            "{}",
            serde_json::json!({ "tailscale_ip": ip, "cidr": cidr })
        );
    } else {
        println!("{ip}");
    }
    Ok(())
}

#[cfg(test)]
mod tailscale_ip_tests {
    use super::*;

    #[test]
    fn parses_ipv4_and_rejects_garbage() {
        assert_eq!(parse_ipv4("100.118.97.79"), Some([100, 118, 97, 79]));
        assert_eq!(parse_ipv4("0.0.0.0"), Some([0, 0, 0, 0]));
        assert_eq!(parse_ipv4("255.255.255.255"), Some([255, 255, 255, 255]));
        assert_eq!(parse_ipv4(""), None);
        assert_eq!(parse_ipv4("100.118.97"), None);
        assert_eq!(parse_ipv4("100.118.97.79.5"), None);
        assert_eq!(parse_ipv4("a.b.c.d"), None);
        assert_eq!(parse_ipv4("1001.1.1.1"), None);
        assert_eq!(parse_ipv4("100.118.-1.79"), None);
        assert_eq!(
            parse_ipv4(" 100.118.97.79"),
            Some([100, 118, 97, 79]),
            "surrounding whitespace is tolerated ( callers pass trimmed CLI output)"
        );
    }

    #[test]
    fn cidr_matching_covers_the_whole_cgnat_block() {
        let cidr = "100.64.0.0/10";
        assert!(is_ipv4_in_cidr("100.64.0.1", cidr));
        assert!(is_ipv4_in_cidr("100.118.97.79", cidr));
        assert!(is_ipv4_in_cidr("100.127.255.254", cidr));
        assert!(
            !is_ipv4_in_cidr("100.128.0.0", cidr),
            "100.64/10 ends at 100.127"
        );
        assert!(!is_ipv4_in_cidr("100.63.255.255", cidr));
        assert!(!is_ipv4_in_cidr("192.168.1.10", cidr));
        assert!(!is_ipv4_in_cidr("10.0.0.1", cidr));
        assert!(!is_ipv4_in_cidr("not-an-ip", cidr));
        assert!(!is_ipv4_in_cidr("100.118.97.79", "garbage"));
        assert!(is_ipv4_in_cidr("100.118.97.79", "100.118.97.79/32"));
        assert!(!is_ipv4_in_cidr("100.118.97.78", "100.118.97.79/32"));
        assert!(is_ipv4_in_cidr("1.2.3.4", "0.0.0.0/0"));
    }

    #[test]
    fn parses_linux_and_bsd_interface_output() {
        // `ip -4 -o addr show` line shape (Linux).
        assert!(parse_ipv4(
            "2: eth0    inet 100.118.97.79/32 scope global tailscale0\n"
                .split_whitespace()
                .nth(3)
                .unwrap()
                .split('/')
                .next()
                .unwrap()
        )
        .is_some());
        // `ifconfig` line shape (macOS/BSD).
        let fields: Vec<&str> =
            "        inet 100.118.97.79 netmask 0xfffff000 broadcast 100.119.255.255"
                .split_whitespace()
                .collect();
        assert_eq!(fields.first(), Some(&"inet"));
        assert!(parse_ipv4(fields[1]).is_some());
    }
}
