mod support;

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use support::{isolate_command, IsolatedCommand};
use tempfile::TempDir;

fn bin() -> IsolatedCommand<Command> {
    let cmd = Command::cargo_bin("gah").unwrap();
    isolate_command(cmd, |cmd, root| {
        let tmp = root.join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        cmd.env("XDG_STATE_HOME", root.join("xdg-state"));
        cmd.env("GAH_AVAILABILITY_PATH", root.join("availability.json"));
        cmd.env(
            "GAH_VALIDATION_CHECK_PATH",
            root.join("validation-check.json"),
        );
        cmd.env("TMPDIR", tmp);
    })
}

#[test]
fn profile_cli_supports_delivery_mode_add_set_show() {
    let home = TempDir::new().unwrap();
    let cfg_path = home.path().join("gah.toml");
    fs::write(&cfg_path, "[profiles]\n").unwrap();

    let local_path = home.path().join("local");
    let artifact_root = home.path().join("artifacts");
    fs::create_dir_all(&local_path).unwrap();
    fs::create_dir_all(&artifact_root).unwrap();

    bin()
        .args([
            "profile",
            "add",
            "handoff-prof",
            "--display-name",
            "Handoff Profile",
            "--repo-id",
            "handoff-repo",
            "--provider",
            "github",
            "--repo",
            "owner/repo",
            "--local-path",
            local_path.to_str().unwrap(),
            "--artifact-root",
            artifact_root.to_str().unwrap(),
            "--delivery-mode",
            "handoff",
            "--config",
            cfg_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    bin()
        .args([
            "profile",
            "list",
            "--json",
            "--config",
            cfg_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"delivery_mode\":\"handoff\""));

    bin()
        .args([
            "config",
            "show",
            "--json",
            "--full",
            "--profile",
            "handoff-prof",
            "--config",
            cfg_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"delivery_mode\":\"handoff\""));

    bin()
        .args([
            "profile",
            "set",
            "handoff-prof",
            "--delivery-mode",
            "pr",
            "--config",
            cfg_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    bin()
        .args([
            "profile",
            "list",
            "--json",
            "--config",
            cfg_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"delivery_mode\":\"pr\""));
}
