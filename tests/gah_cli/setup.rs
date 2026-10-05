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

#[cfg(target_os = "linux")]
#[test]
fn factory_toggle_stops_only_factory_services_and_preserves_profiles() {
    let home = test_tempdir();
    let config = home.path().join("config.toml");
    fs::write(&config, "[defaults]\nnode_role = 'standalone'\n").unwrap();
    let log = home.path().join("services.log");
    write_executable(
        &home.path().join("systemctl"),
        r#"#!/bin/sh
case "$*" in
  --version) echo systemd ;;
  *list-units*) printf 'gah-loop@local.service loaded active running\ngah-watchdog.service loaded inactive dead\ngah-server.service loaded active running\n' ;;
  *list-unit-files*) printf 'gah-loop@.service disabled\ngah-loop@local.service enabled\ngah-watchdog.timer enabled\ngah-prune.timer enabled\ngah-quota-refresh.timer enabled\n' ;;
  *) printf '%s\n' "$*" >> "$GAH_FACTORY_TEST_LOG" ;;
esac
"#,
    );
    for enabled in ["false", "true", "false"] {
        bin()
            .args(["config", "set", "--config"])
            .arg(&config)
            .args(["--factory-enabled", enabled])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    home.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("GAH_FACTORY_TEST_LOG", &log)
            .assert()
            .success();
        let saved: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            saved["defaults"]["factory_enabled"].as_bool(),
            Some(enabled == "true")
        );
        assert_eq!(saved["defaults"]["node_role"].as_str(), Some("standalone"));
        if enabled == "false" {
            bin()
                .args(["loop", "--profile", "local", "--config-path"])
                .arg(&config)
                .assert()
                .failure()
                .stderr(predicates::str::contains("Factory automation is disabled"));
        }
    }
    let actions = fs::read_to_string(log).unwrap();
    assert_eq!(
        actions
            .matches("disable --now gah-loop@local.service")
            .count(),
        2
    );
    assert!(actions.contains("disable --now gah-watchdog.timer"));
    assert!(!actions.contains("gah-server"));
    assert!(!actions.contains("gah-prune"));
    assert!(!actions.contains("gah-quota-refresh"));
    assert!(!actions.contains("enable"));
}

#[cfg(unix)]
#[test]
fn standalone_installer_defaults_off_but_preserves_existing_selection() {
    let home = test_tempdir();
    let config = home.path().join("config.toml");
    let log = home.path().join("installer.log");
    let cli = home.path().join("fake-gah");
    write_executable(
        &cli,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$GAH_FACTORY_TEST_LOG\"\n",
    );
    for (contents, selection, expected) in [
        (None, None, "--factory-enabled false"),
        (
            Some("[defaults]\n"),
            None,
            "config set --node-role standalone",
        ),
        (
            Some("[defaults]\nfactory_enabled = false\n"),
            None,
            "config set --node-role standalone",
        ),
        (Some("[defaults]\n"), Some("true"), "--factory-enabled true"),
    ] {
        if let Some(contents) = contents {
            fs::write(&config, contents).unwrap();
        } else if config.exists() {
            fs::remove_file(&config).unwrap();
        }
        let mut command = ProcessCommand::new("bash");
        command
            .arg("scripts/configure-node-role.sh")
            .arg("standalone")
            .arg(&cli)
            .env("GAH_CONFIG", &config)
            .env("GAH_FACTORY_TEST_LOG", &log)
            .env_remove("GAH_CENTRAL_URL");
        if let Some(selection) = selection {
            command.env("GAH_FACTORY_ENABLED", selection);
        } else {
            command.env_remove("GAH_FACTORY_ENABLED");
        }
        assert!(command.status().unwrap().success());
        let invocation = fs::read_to_string(&log).unwrap();
        assert!(invocation.trim().ends_with(expected), "{invocation}");
        if contents.is_some() && selection.is_none() {
            assert!(!invocation.contains("factory-enabled"));
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn factory_disable_failure_still_persists_the_dispatch_guard() {
    let home = test_tempdir();
    let config = home.path().join("config.toml");
    fs::write(&config, "[defaults]\nfactory_enabled = true\n").unwrap();
    write_executable(&home.path().join("systemctl"), "#!/bin/sh\nif [ \"$1\" = --version ]; then echo systemd; exit 0; fi\necho 'Service bus unavailable' >&2\nexit 1\n");
    bin()
        .args(["config", "set", "--config"])
        .arg(&config)
        .args(["--factory-enabled", "false"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                home.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .assert()
        .failure()
        .stderr(predicates::str::contains("Service bus unavailable"));
    let saved: toml::Value = toml::from_str(&fs::read_to_string(config).unwrap()).unwrap();
    assert_eq!(saved["defaults"]["factory_enabled"].as_bool(), Some(false));
}
