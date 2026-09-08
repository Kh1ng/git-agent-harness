use super::*;

fn config_with_profile() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = test_tempdir();
    let config = tmp.path().join("config.toml");
    let artifact_root = tmp.path().join("artifacts");
    std::fs::write(&config, "[defaults]\ncurrent_manager = \"codex\"\n").unwrap();

    bin()
        .args([
            "profile",
            "add",
            "test",
            "--display-name",
            "Test",
            "--repo-id",
            "owner/repo",
            "--provider",
            "github",
            "--repo",
            "owner/repo",
            "--local-path",
            tmp.path().to_str().unwrap(),
            "--artifact-root",
            artifact_root.to_str().unwrap(),
            "--config-path",
            config.to_str().unwrap(),
        ])
        .assert()
        .success();

    (tmp, config)
}

#[test]
fn bare_json_shape_remains_byte_for_byte_compatible() {
    let (_tmp, config) = config_with_profile();

    bin()
        .args([
            "config",
            "show",
            "--json",
            "--config",
            config.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout("{\"current_manager\":\"codex\"}\n");
}

#[test]
fn full_json_is_versioned_and_profile_keyed() {
    let (_tmp, config) = config_with_profile();
    let output = bin()
        .args([
            "config",
            "show",
            "--json",
            "--full",
            "--profile",
            "test",
            "--config",
            config.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["schema_version"], 3);
    assert_eq!(payload["config_path"], config.to_string_lossy().as_ref());
    assert_eq!(payload["profiles"]["test"]["profile"], "test");
    assert_eq!(payload["profiles"]["test"]["delivery_mode"], "pr");
    assert_eq!(payload["profiles"].as_object().unwrap().len(), 1);
}

#[test]
fn full_and_profile_require_machine_readable_mode() {
    bin().args(["config", "show", "--full"]).assert().failure();
    bin()
        .args(["config", "show", "--json", "--profile", "test"])
        .assert()
        .failure();
}

#[test]
fn node_role_and_central_url_survive_config_changes_and_need_no_profile() {
    let tmp = test_tempdir();
    let path = tmp.path().join("config.toml");
    let config = path.to_str().unwrap();
    bin()
        .args([
            "config",
            "set",
            "--config",
            config,
            "--node-role",
            "worker",
            "--registry-central-url",
            "http://192.168.1.10:3773",
        ])
        .assert()
        .success();
    bin()
        .args([
            "config",
            "set",
            "--config",
            config,
            "--current-manager",
            "claude",
        ])
        .assert()
        .success();
    let output = bin()
        .args(["status", "--role", "--json", "--config-path", config])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let node: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(node["role"], "worker");
    let overridden = bin()
        .env("GAH_NODE_ROLE", "central")
        .args(["status", "--role", "--json", "--config-path", config])
        .output()
        .unwrap();
    assert!(overridden.status.success());
    let overridden: serde_json::Value = serde_json::from_slice(&overridden.stdout).unwrap();
    assert_eq!(overridden["role"], "central");
    bin()
        .env("GAH_NODE_ROLE", "typo")
        .args(["status", "--role", "--json", "--config-path", config])
        .assert()
        .failure();
    assert_eq!(node["central_url"], "http://192.168.1.10:3773");
    let before = std::fs::read(&path).unwrap();
    bin()
        .args([
            "config",
            "set",
            "--config",
            config,
            "--clear",
            "registry_central_url",
        ])
        .assert()
        .failure();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    bin()
        .args([
            "config",
            "set",
            "--config",
            config,
            "--node-role",
            "central",
        ])
        .assert()
        .success();
    let output = bin()
        .args(["status", "--role", "--json", "--config-path", config])
        .output()
        .unwrap();
    let node: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(node["role"], "central");
}
