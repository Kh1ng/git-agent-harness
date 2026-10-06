mod support;

#[test]
fn task_reasoning_round_trips_through_profile_cli_without_changing_other_args() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = dir.path().join("gah.toml");
    std::fs::write(
        &config,
        r#"
[profiles.repo]
display_name = "Repo"
repo_id = "repo"
provider = "github"
repo = "owner/repo"
local_path = "/tmp/repo"
artifact_root = "/tmp/artifacts"
default_target_branch = "main"
codex_args = ["--sandbox", "workspace-write", "-c", "model_reasoning_effort=\"low\""]
claude_args = ["--verbose"]
"#,
    )
    .unwrap();
    let set = |settings: &[&str]| {
        support::gah_command()
            .args(["profile", "set", "repo"])
            .args(settings)
            .arg("--config")
            .arg(&config)
            .assert()
    };
    set(&[
        "--agent-effort",
        "codex=high",
        "--agent-effort",
        "claude=max",
    ])
    .success();
    let output = support::gah_command()
        .args(["profile", "list", "--json", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(output.status.success());
    let profiles: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        profiles[0]["agent_reasoning_effort"],
        serde_json::json!({"codex":"high", "claude":"max"})
    );
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(
        saved["profiles"]["repo"]["codex_args"].as_array().unwrap()[0].as_str(),
        Some("--sandbox")
    );
    assert_eq!(
        saved["profiles"]["repo"]["claude_args"].as_array().unwrap()[0].as_str(),
        Some("--verbose")
    );
    let before = std::fs::read_to_string(&config).unwrap();
    set(&["--agent-effort", "claude=ultra"]).failure();
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
    set(&[
        "--agent-effort",
        "codex=default",
        "--agent-effort",
        "claude=default",
    ])
    .success();
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(
        saved["profiles"]["repo"]["claude_args"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        saved["profiles"]["repo"]["codex_args"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
