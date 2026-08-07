use super::*;
use crate::config::Defaults;
use crate::test_support::PathGuard;
use std::fs;

fn defaults(lan_cidrs: &[&str], tailscale_cidr: Option<&str>) -> Defaults {
    Defaults {
        lan_cidrs: lan_cidrs.iter().map(|s| s.to_string()).collect(),
        tailscale_cidr: tailscale_cidr.map(str::to_string),
        ..Default::default()
    }
}

/// A fake `sudo` answering exactly the two invocations `apply`/
/// `existing_allowed_cidrs` make: `-n ufw status` (returns a canned
/// status with one pre-existing rule for `port`/`preexisting_cidr`) and
/// `ufw allow ...` (records the call and succeeds). Matches real `ufw
/// status`'s plain (non-verbose) column layout exactly -- this is the
/// shape that caught a real parsing bug (the code assumed a `verbose`-only
/// "ALLOW IN" column that plain `ufw status` doesn't have).
fn write_fake_sudo(
    dir: &std::path::Path,
    record_path: &std::path::Path,
    port: u16,
    preexisting_cidr: &str,
) {
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "-n" ] && [ "$2" = "ufw" ] && [ "$3" = "status" ]; then
  cat <<EOF
Status: active

To                         Action      From
--                         ------      ----
{port}/tcp                   ALLOW       {preexisting_cidr}             # existing rule
EOF
  exit 0
fi
if [ "$1" = "ufw" ] && [ "$2" = "allow" ]; then
  echo "$@" >> '{record}'
  echo "Rule added"
  exit 0
fi
echo "unexpected fake sudo invocation: $@" >&2
exit 1
"#,
        record = record_path.display(),
    );
    let path = dir.join("sudo");
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&path, perms).unwrap();
}

#[test]
fn apply_skips_an_already_allowed_cidr_and_only_invokes_ufw_for_the_new_one() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let record_path = tmp.path().join("allow-calls.log");
    write_fake_sudo(&bin_dir, &record_path, 9998, "192.168.5.0/24");
    let _path_guard = PathGuard::set(bin_dir.to_str().unwrap());

    apply(
        9998,
        "test service",
        &[
            "192.168.5.0/24".to_string(), // already allowed per the fake status
            "192.168.1.0/24".to_string(), // new
        ],
    )
    .unwrap();

    let record = fs::read_to_string(&record_path).unwrap_or_default();
    assert!(
        !record.contains("192.168.5.0/24"),
        "must not re-issue an allow for an already-allowed CIDR: {record}"
    );
    assert!(
        record.contains("192.168.1.0/24"),
        "must issue an allow for the new CIDR: {record}"
    );
}

#[test]
fn existing_allowed_cidrs_parses_plain_non_verbose_ufw_status_output() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let record_path = tmp.path().join("allow-calls.log");
    write_fake_sudo(&bin_dir, &record_path, 4242, "100.64.0.0/10");
    let _path_guard = PathGuard::set(bin_dir.to_str().unwrap());

    let cidrs = existing_allowed_cidrs(4242).unwrap();

    assert_eq!(cidrs, vec!["100.64.0.0/10".to_string()]);
}

#[test]
fn parse_recognizes_every_documented_spelling() {
    assert_eq!(
        NetworkExposureLevel::parse("loopback"),
        Some(NetworkExposureLevel::Loopback)
    );
    assert_eq!(
        NetworkExposureLevel::parse("lan"),
        Some(NetworkExposureLevel::Lan)
    );
    assert_eq!(
        NetworkExposureLevel::parse("lan_tailscale"),
        Some(NetworkExposureLevel::LanTailscale)
    );
    assert_eq!(
        NetworkExposureLevel::parse("lan+tailscale"),
        Some(NetworkExposureLevel::LanTailscale)
    );
    assert_eq!(NetworkExposureLevel::parse("bogus"), None);
}

#[test]
fn default_level_is_loopback() {
    assert_eq!(
        NetworkExposureLevel::default(),
        NetworkExposureLevel::Loopback
    );
}

#[test]
fn loopback_never_requires_any_cidr_even_if_lan_and_tailscale_are_configured() {
    let d = defaults(&["192.168.5.0/24"], Some("100.64.0.0/10"));
    assert!(required_cidrs(&d, NetworkExposureLevel::Loopback).is_empty());
}

#[test]
fn lan_requires_only_the_configured_lan_cidrs() {
    let d = defaults(&["192.168.1.0/24", "192.168.5.0/24"], Some("100.64.0.0/10"));
    assert_eq!(
        required_cidrs(&d, NetworkExposureLevel::Lan),
        vec!["192.168.1.0/24".to_string(), "192.168.5.0/24".to_string()]
    );
}

#[test]
fn lan_tailscale_requires_lan_cidrs_plus_tailscale() {
    let d = defaults(&["192.168.5.0/24"], Some("100.64.0.0/10"));
    assert_eq!(
        required_cidrs(&d, NetworkExposureLevel::LanTailscale),
        vec!["192.168.5.0/24".to_string(), "100.64.0.0/10".to_string()]
    );
}

#[test]
fn lan_tailscale_with_no_tailscale_cidr_configured_falls_back_to_lan_only() {
    let d = defaults(&["192.168.5.0/24"], None);
    assert_eq!(
        required_cidrs(&d, NetworkExposureLevel::LanTailscale),
        vec!["192.168.5.0/24".to_string()]
    );
}

#[test]
fn empty_lan_cidrs_with_lan_level_requires_nothing() {
    // Not a footgun: an operator who sets network_exposure = "lan" without
    // naming any lan_cidrs gets no firewall rule at all, not a wildcard.
    let d = defaults(&[], None);
    assert!(required_cidrs(&d, NetworkExposureLevel::Lan).is_empty());
}

#[test]
fn recommended_bind_host_is_loopback_only_for_loopback_level() {
    assert_eq!(
        NetworkExposureLevel::Loopback.recommended_bind_host(),
        "127.0.0.1"
    );
    assert_eq!(NetworkExposureLevel::Lan.recommended_bind_host(), "0.0.0.0");
    assert_eq!(
        NetworkExposureLevel::LanTailscale.recommended_bind_host(),
        "0.0.0.0"
    );
}
