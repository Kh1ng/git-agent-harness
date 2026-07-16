mod support;

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use support::{isolate_command, test_tempdir, IsolatedCommand};
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

fn write_dispatch_config(tmp: &TempDir) -> std::path::PathBuf {
    let cfg = tmp.path().join("gah-config.toml");
    fs::write(
        &cfg,
        r#"
[defaults]
artifact_root = "/tmp/gah-test-artifacts"
worktree_base = "/tmp/gah-test-worktrees"
llm_base_url  = "http://localhost:4000"
llm_model_local = "local/test"
llm_model_cloud = "cloud/test"

[profiles.test-repo]
display_name          = "Test Repo"
repo_id               = "test-repo"
provider              = "github"
repo                  = "owner/test-repo"
local_path            = "/tmp/nonexistent-repo"
artifact_root         = "/tmp/gah-test-artifacts/test-repo"
default_target_branch = "main"
claude_args           = ["--allowedTools", "Edit,Write,Bash"]
"#,
    )
    .unwrap();
    cfg
}

#[test]
fn profile_show_displays_effective_validation_timeout() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "profile",
            "show",
            "test-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("validation_timeout_seconds: 300"));
}

#[test]
fn profile_set_validation_timeout_round_trip() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);

    bin()
        .args([
            "profile",
            "set",
            "test-repo",
            "--validation-timeout-seconds",
            "900",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated profile 'test-repo'"));

    bin()
        .args([
            "profile",
            "show",
            "test-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("validation_timeout_seconds: 900"));
}

#[test]
fn profile_set_validation_timeout_clears_to_default_when_unset() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);

    bin()
        .args([
            "profile",
            "set",
            "test-repo",
            "--validation-timeout-seconds",
            "900",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated profile 'test-repo'"));

    bin()
        .args([
            "profile",
            "set",
            "test-repo",
            "--clear",
            "validation_timeout_seconds",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated profile 'test-repo'"));

    bin()
        .args([
            "profile",
            "show",
            "test-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("validation_timeout_seconds: 300"));
}
