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

#[test]
fn set_backend_instance_enabled_writes_merged_entry_and_flips_state() {
    let (tmp, config) = config_with_profile();
    let config_path = config.to_str().unwrap();
    // Declare the instance at the repo-defaults level; the profile-level
    // toggle must preserve every declared field (entries replace wholesale).
    // Append the shared-instance declaration as its own table (distinct
    // table names keep TOML order-independent).
    std::fs::write(
        &config,
        format!(
            "{}\n[defaults.routing.backend_instances.codex-paid]\n\
             runner_kind = \"codex\"\n\
             logical_backend = \"codex\"\n\
             executable = \"{}\"\n\
             account_label = \"team-api\"\n\
             supported_models = [\"openai/gpt-5\"]\n",
            std::fs::read_to_string(&config).unwrap(),
            std::env::current_exe().unwrap().display()
        ),
    )
    .unwrap();

    // Unknown instance is rejected.
    bin()
        .args([
            "config",
            "set-backend-instance-enabled",
            "--config",
            config_path,
            "--profile",
            "test",
            "--instance",
            "does-not-exist",
            "--enabled",
            "false",
        ])
        .assert()
        .failure();

    bin()
        .args([
            "config",
            "set-backend-instance-enabled",
            "--config",
            config_path,
            "--profile",
            "test",
            "--instance",
            "codex-paid",
            "--enabled",
            "false",
        ])
        .assert()
        .success();

    let saved = std::fs::read_to_string(&config).unwrap();
    // The profile-level override entry (not the defaults one) carries the
    // flipped flag, and the merged entry preserves every declared field.
    assert!(
        saved.contains("[profiles.test.routing.backend_instances.codex-paid]"),
        "profile-level override entry must be written, got: {saved}"
    );
    let profile_section = saved
        .split("[profiles.test.routing.backend_instances.codex-paid]")
        .nth(1)
        .expect("profile-level entry written");
    assert!(
        profile_section.contains("enabled = false"),
        "flag must be flipped, got: {saved}"
    );
    assert!(
        profile_section.contains(&format!(
            "executable = \"{}\"",
            std::env::current_exe().unwrap().display()
        )),
        "merged entry must preserve declared fields, got: {saved}"
    );

    // Already in the target state: idempotent success.
    bin()
        .args([
            "config",
            "set-backend-instance-enabled",
            "--config",
            config_path,
            "--profile",
            "test",
            "--instance",
            "codex-paid",
            "--enabled",
            "false",
        ])
        .assert()
        .success();

    // Flip back on.
    bin()
        .args([
            "config",
            "set-backend-instance-enabled",
            "--config",
            config_path,
            "--profile",
            "test",
            "--instance",
            "codex-paid",
            "--enabled",
            "true",
        ])
        .assert()
        .success();
    let saved = std::fs::read_to_string(&config).unwrap();
    let profile_section = saved
        .split("[profiles.test.routing.backend_instances.codex-paid]")
        .nth(1)
        .expect("profile-level entry still present");
    assert!(
        profile_section.contains("enabled = true"),
        "flag must flip back, got: {saved}"
    );

    // Unknown profile is rejected.
    bin()
        .args([
            "config",
            "set-backend-instance-enabled",
            "--config",
            config_path,
            "--profile",
            "missing",
            "--instance",
            "codex-paid",
            "--enabled",
            "false",
        ])
        .assert()
        .failure();

    let _ = tmp;
}

#[test]
fn notification_channel_settings_round_trip_and_validate() {
    let (tmp, config) = config_with_profile();
    let config_path = config.to_str().unwrap();

    // Invalid channel is rejected with the expected vocabulary.
    bin()
        .args([
            "config",
            "set",
            "--config",
            config_path,
            "--notification-channel",
            "slack",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("expected none|telegram|discord"));

    bin()
        .args([
            "config",
            "set",
            "--config",
            config_path,
            "--notification-channel",
            "telegram",
            "--telegram-chat-id",
            "123456789",
        ])
        .assert()
        .success();

    let saved = std::fs::read_to_string(&config).unwrap();
    assert!(
        saved.contains("notification_channel = \"telegram\""),
        "got: {saved}"
    );
    assert!(
        saved.contains("telegram_chat_id = \"123456789\""),
        "got: {saved}"
    );

    // Clearing the chat id works; the channel persists.
    bin()
        .args([
            "config",
            "set",
            "--config",
            config_path,
            "--telegram-chat-id",
            "",
        ])
        .assert()
        .success();
    let saved = std::fs::read_to_string(&config).unwrap();
    assert!(
        saved.contains("notification_channel = \"telegram\""),
        "got: {saved}"
    );
    assert!(!saved.contains("telegram_chat_id ="), "got: {saved}");

    let _ = tmp;
}

#[test]
fn routing_candidate_add_remove_move_round_trip_with_preview_and_validation() {
    let (tmp, config) = config_with_profile();
    let config_path = config.to_str().unwrap();

    // Add two candidates (one paid, approval-gated).
    bin()
        .args([
            "config",
            "routing-candidate",
            "add",
            "--config",
            config_path,
            "--profile",
            "test",
            "--list",
            "improve",
            "--backend",
            "codex",
            "--model",
            "gpt-5.4-mini",
        ])
        .assert()
        .success();
    bin()
        .args([
            "config",
            "routing-candidate",
            "add",
            "--config",
            config_path,
            "--profile",
            "test",
            "--list",
            "improve",
            "--backend",
            "vibe",
            "--requires-approval",
        ])
        .assert()
        .success();

    let output = bin()
        .args([
            "config",
            "routing-candidate",
            "add",
            "--config",
            config_path,
            "--profile",
            "test",
            "--list",
            "improve",
            "--backend",
            "claude",
            "--dry-run",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let preview: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        preview.as_array().unwrap().len(),
        3,
        "dry-run previews three entries"
    );
    // Dry-run must not have saved.
    let saved = std::fs::read_to_string(&config).unwrap();
    assert!(
        !saved.contains("\"claude\""),
        "dry-run must not persist, got: {saved}"
    );

    // Reorder: move claude's slot (would-be index 2 doesn't exist; move 1 -> 0).
    bin()
        .args([
            "config",
            "routing-candidate",
            "move",
            "--config",
            config_path,
            "--profile",
            "test",
            "--list",
            "improve",
            "--from",
            "1",
            "--to",
            "0",
        ])
        .assert()
        .success();
    let saved = std::fs::read_to_string(&config).unwrap();
    let profile_section = saved
        .split("[profiles.test.routing.improve_candidates]")
        .nth(1)
        .map(|section| section.split("\n\n").next().unwrap_or(section));
    assert!(profile_section.is_some(), "profile-level list written");

    // Remove the first entry.
    bin()
        .args([
            "config",
            "routing-candidate",
            "remove",
            "--config",
            config_path,
            "--profile",
            "test",
            "--list",
            "improve",
            "--index",
            "0",
        ])
        .assert()
        .success();

    // Out-of-range index fails with a descriptive error.
    bin()
        .args([
            "config",
            "routing-candidate",
            "remove",
            "--config",
            config_path,
            "--profile",
            "test",
            "--list",
            "improve",
            "--index",
            "9",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("out of range"));

    // Invalid list name fails with the expected vocabulary.
    bin()
        .args([
            "config",
            "routing-candidate",
            "add",
            "--config",
            config_path,
            "--profile",
            "test",
            "--list",
            "squads",
            "--backend",
            "codex",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "expected pm|improve|review|escalatory",
        ));

    // Unknown profile fails.
    bin()
        .args([
            "config",
            "routing-candidate",
            "add",
            "--config",
            config_path,
            "--profile",
            "missing",
            "--list",
            "pm",
            "--backend",
            "codex",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("not configured"));

    let _ = tmp;
}

#[test]
fn prompt_policy_cli_versions_audits_and_hides_content_from_show() {
    let (tmp, config) = config_with_profile();
    let path = config.to_str().unwrap();
    let events = tmp.path().join("events.jsonl");
    let content = "## Safety\nKeep the canary POLICY-CANARY-182 visible only in prompts.";

    bin()
        .env("GAH_EVENTS_PATH", &events)
        .args([
            "config",
            "prompt-policy",
            "set",
            "--profile",
            "test",
            "--slot",
            "worker_guidance",
            "--content",
            content,
            "--expected-revision",
            "0",
            "--config",
            path,
        ])
        .assert()
        .success();
    let audit = fs::read_to_string(&events).unwrap();
    assert!(audit.contains("prompt_policy_changed"));
    assert!(!audit.contains("POLICY-CANARY-182"));

    let shown = bin()
        .args([
            "config",
            "prompt-policy",
            "show",
            "--profile",
            "test",
            "--json",
            "--config",
            path,
        ])
        .output()
        .unwrap();
    assert!(shown.status.success());
    let summary: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(summary["revision"], 1);
    assert!(summary["policies"].as_array().unwrap().iter().any(|entry| {
        entry["source"] == "profile_override"
            && entry["byte_size"] == content.len()
            && entry["sha256"].as_str().unwrap().starts_with("sha256:")
    }));
    assert!(!String::from_utf8_lossy(&shown.stdout).contains("POLICY-CANARY-182"));
}
