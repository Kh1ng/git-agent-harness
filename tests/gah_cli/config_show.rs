use serde_json::Value;

fn config_with_profile() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = super::test_tempdir();
    let config = tmp.path().join("config.toml");
    let artifact_root = tmp.path().join("artifacts");
    std::fs::write(&config, "[defaults]\ncurrent_manager = \"codex\"\n").unwrap();

    super::bin()
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

    super::bin()
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
    let output = super::bin()
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
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["config_path"], config.to_string_lossy().as_ref());
    assert_eq!(payload["profiles"]["test"]["profile"], "test");
    assert_eq!(payload["profiles"].as_object().unwrap().len(), 1);
}

#[test]
fn full_and_profile_require_machine_readable_mode() {
    super::bin()
        .args(["config", "show", "--full"])
        .assert()
        .failure();
    super::bin()
        .args(["config", "show", "--json", "--profile", "test"])
        .assert()
        .failure();
}
