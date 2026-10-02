use super::*;

#[test]
fn memory_hook_setup_installs_from_the_binary_and_merges_repeatably() {
    let home = test_tempdir();
    let claude = home.path().join(".claude/settings.json");
    fs::create_dir_all(claude.parent().unwrap()).unwrap();
    fs::write(&claude, r#"{"permissions":{"allow":["Read"]}}"#).unwrap();
    for _ in 0..2 {
        bin()
            .args([
                "setup",
                "memory-hooks",
                "--tool",
                "claude,codex",
                "--home-dir",
            ])
            .arg(home.path())
            .assert()
            .success();
    }
    let settings: Value = serde_json::from_slice(&fs::read(&claude).unwrap()).unwrap();
    assert_eq!(
        settings["permissions"]["allow"],
        serde_json::json!(["Read"])
    );
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().unwrap().len(),
        1
    );
    assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert!(home.path().join(".codex/hooks.json").is_file());
    assert_eq!(
        fs::read_to_string(home.path().join(".local/bin/gah-memory-hook")).unwrap(),
        include_str!("../../scripts/gah-memory-hook.py")
    );
    assert!(!home.path().join(".config/gah/memory-hooks.json").exists());
}

#[cfg(unix)]
#[test]
fn memory_setup_checks_authenticated_recall_and_names_the_key_file_on_401() {
    use std::os::unix::fs::PermissionsExt;
    let home = test_tempdir();
    let curl = home.path().join("curl");
    let request = home.path().join("request");
    fs::write(&curl, "#!/bin/sh\n/bin/cat > \"$GAH_TEST_MEMORY_REQUEST\"\nprintf '\\n__GAH_MEMORY_GATEWAY_STATUS__:%s\\n' \"$GAH_TEST_MEMORY_STATUS\"\n").unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
    let key_file = home.path().join(".config/gah/tdai-gateway.env");
    let path = format!(
        "{}:{}",
        home.path().display(),
        std::env::var("PATH").unwrap()
    );
    for file_only in [true, false] {
        if file_only {
            git_agent_harness::installer::files::env_set(
                &key_file,
                "TDAI_GATEWAY_API_KEY",
                "fixture-key",
            )
            .unwrap();
        } else {
            fs::remove_file(&key_file).unwrap();
        }
        let mut command = bin();
        command
            .args([
                "setup",
                "memory-hooks",
                "--tool",
                "claude",
                "--gateway-url",
                "https://memory.test",
                "--home-dir",
            ])
            .arg(home.path())
            .env("PATH", &path)
            .env("GAH_TEST_MEMORY_REQUEST", &request)
            .env("GAH_TEST_MEMORY_STATUS", "200");
        if file_only {
            command.env_remove("TDAI_GATEWAY_API_KEY");
        } else {
            command.env("TDAI_GATEWAY_API_KEY", "fixture-key");
        }
        command.assert().success().stdout(predicates::str::contains(
            "authenticated recall check passed",
        ));
        let request = fs::read_to_string(&request).unwrap();
        assert!(request.contains("https://memory.test/recall"));
        assert!(request.contains("Authorization: Bearer fixture-key"));
        assert_eq!(request.matches("url =").count(), 1);
    }
    bin()
        .args([
            "setup",
            "memory-hooks",
            "--tool",
            "claude",
            "--gateway-url",
            "https://memory.test",
            "--home-dir",
        ])
        .arg(home.path())
        .env("PATH", &path)
        .env("GAH_TEST_MEMORY_REQUEST", &request)
        .env("GAH_TEST_MEMORY_STATUS", "401")
        .env_remove("TDAI_GATEWAY_API_KEY")
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("HTTP 401")
                .and(predicates::str::contains("tdai-gateway.env")),
        );
}
