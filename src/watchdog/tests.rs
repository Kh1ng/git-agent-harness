use super::*;
use crate::test_support::PathGuard;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn classify_active_is_healthy() {
    assert_eq!(classify("loaded", "active", "running"), LoopHealth::Healthy);
}

#[test]
fn classify_uninstantiated_unit_is_healthy() {
    // A template unit that was never started for this profile: not this
    // watchdog's concern, and must never be reported as "stopped".
    assert_eq!(
        classify("not-found", "inactive", "dead"),
        LoopHealth::Healthy
    );
}

#[test]
fn classify_clean_exit_is_stopped_cleanly() {
    assert_eq!(
        classify("loaded", "inactive", "dead"),
        LoopHealth::StoppedCleanly
    );
}

#[test]
fn classify_failed_active_state_is_crashed() {
    assert_eq!(classify("loaded", "failed", "failed"), LoopHealth::Crashed);
}

#[test]
fn classify_transitional_states_are_not_alerted() {
    for active_state in ["activating", "deactivating", "reloading"] {
        assert_eq!(
            classify("loaded", active_state, "start"),
            LoopHealth::Transitional,
            "active_state={active_state}"
        );
    }
}

fn write_fake_systemctl(dir: &std::path::Path, record_path: &std::path::Path) {
    let script = format!(
        r#"#!/bin/sh
echo "$@" >> '{record}'
unit="$3"
case "$unit" in
  gah-loop@healthy.service) printf 'loaded\nactive\nrunning\n' ;;
  gah-loop@stopped.service) printf 'loaded\ninactive\ndead\n' ;;
  gah-loop@crashed.service) printf 'loaded\nfailed\nfailed\n' ;;
  gah-loop@ghost.service) printf 'not-found\ninactive\ndead\n' ;;
  *) echo "unexpected unit: $unit" >&2; exit 1 ;;
esac
"#,
        record = record_path.display(),
    );
    let path = dir.join("systemctl");
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

fn write_config(path: &std::path::Path, profile_names: &[&str]) {
    let mut toml = String::new();
    for name in profile_names {
        toml.push_str(&format!(
            r#"
[profiles.{name}]
display_name = "Test {name}"
repo_id = "test/{name}"
provider = "github"
repo = "test/{name}"
local_path = "/tmp"
artifact_root = "/tmp"
default_target_branch = "main"
"#
        ));
    }
    fs::write(path, toml).unwrap();
}

/// Issue #726 AC5: proves the watchdog never invokes a start/restart/enable
/// mechanism, across every unit state it can observe, by recording every
/// `systemctl` argv actually issued and asserting none of them contain a
/// mutating verb.
#[test]
fn collect_alerts_never_issues_a_start_restart_or_enable_call() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let record_path = tmp.path().join("argv.log");
    write_fake_systemctl(&bin_dir, &record_path);
    let _path_guard = PathGuard::set(bin_dir.to_str().unwrap());

    let cfg_path = tmp.path().join("cfg.toml");
    write_config(&cfg_path, &["healthy", "stopped", "crashed", "ghost"]);
    let cfg = crate::config::load(Some(cfg_path.to_str().unwrap())).unwrap();

    let alerts = collect_alerts(&cfg, None).unwrap();

    assert_eq!(alerts.len(), 2, "{alerts:?}");
    assert!(
        alerts
            .iter()
            .any(|line| line.contains("gah-loop@stopped.service")
                && line.contains("stopped (clean exit)")),
        "{alerts:?}"
    );
    assert!(
        alerts
            .iter()
            .any(|line| line.contains("gah-loop@crashed.service") && line.contains("FAILED")),
        "{alerts:?}"
    );

    let record = fs::read_to_string(&record_path).unwrap();
    assert!(!record.is_empty(), "expected systemctl to be invoked");
    for forbidden in ["start", "restart", "enable", "kill", "stop"] {
        assert!(
            !record.split_whitespace().any(|arg| arg == forbidden),
            "systemctl was invoked with forbidden verb '{forbidden}': {record}"
        );
    }
    assert!(
        record.lines().all(|line| line.contains(" show ")),
        "every invocation must be a read-only 'show' query: {record}"
    );
}

#[test]
fn collect_alerts_scopes_to_one_named_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let record_path = tmp.path().join("argv.log");
    write_fake_systemctl(&bin_dir, &record_path);
    let _path_guard = PathGuard::set(bin_dir.to_str().unwrap());

    let cfg_path = tmp.path().join("cfg.toml");
    write_config(&cfg_path, &["healthy", "crashed"]);
    let cfg = crate::config::load(Some(cfg_path.to_str().unwrap())).unwrap();

    let alerts = collect_alerts(&cfg, Some("healthy")).unwrap();

    assert!(alerts.is_empty(), "{alerts:?}");
    let record = fs::read_to_string(&record_path).unwrap();
    assert!(!record.contains("crashed"), "{record}");
}
