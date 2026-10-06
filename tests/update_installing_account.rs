use assert_cmd::Command;
use predicates::boolean::PredicateBooleanExt;
use predicates::str::contains;

#[test]
fn sudo_environment_does_not_refuse_a_non_root_update() {
    // `sudo -iu <account> gah update` runs as that account while keeping
    // SUDO_USER set, so the guard must key on the effective uid (#1322):
    // a non-root run proceeds to checkout access, where this run fails.
    Command::cargo_bin("gah")
        .unwrap()
        .env("SUDO_USER", "provisioning-admin")
        .args(["update", "--repo", "/missing-checkout-must-not-be-accessed"])
        .assert()
        .failure()
        .stderr(contains("is not a Git checkout"))
        .stderr(contains("run gah update without sudo").not());
}

#[test]
fn update_without_pull_accepts_a_local_dirty_checkout_and_cancels_before_changes() {
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&["init", "-b", "local-work"]);
    let file = repo.path().join("untracked.txt");
    std::fs::write(&file, "local changes").unwrap();
    Command::cargo_bin("gah")
        .unwrap()
        .args(["update", "--role", "worker", "--agent", "claude", "--repo"])
        .arg(repo.path())
        .write_stdin("n\n")
        .assert()
        .failure()
        .stderr(contains("Update cancelled before installation"))
        .stdout(contains("Fetch origin").not())
        .stdout(contains("OpenCode").not())
        .stdout(contains("quota-refresh").not());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "local changes");
}

#[cfg(target_os = "linux")]
#[test]
fn update_installs_selected_assets_and_refreshes_them_without_agent_flags() {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn executable(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Exercise the real update command, stubbing only external installation
    // commands. The templates and asset copies are the production paths.
    for agent in ["opencode", "codex", "vibe"] {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let config = temp.path().join("config");
        let bin = temp.path().join("bin");
        let cargo = temp.path().join("cargo");
        for dir in [&repo, &config, &bin, &cargo.join("bin")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        assert!(std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .arg(&repo)
            .output()
            .unwrap()
            .status
            .success());
        for dir in ["systemd", "opencode/agents"] {
            let source = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("packaging")
                .join(dir);
            let destination = repo.join("packaging").join(dir);
            std::fs::create_dir_all(&destination).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                if entry.path().is_file() {
                    std::fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
                }
            }
        }
        executable(&bin.join("cargo"), "exit 0");
        executable(&cargo.join("bin/gah"), "exit 0");
        executable(
            &bin.join("systemctl"),
            "printf '%s\\n' \"$*\" >> \"$GAH_TEST_SYSTEMCTL_LOG\"",
        );
        executable(&bin.join("loginctl"), "exit 0");
        executable(&bin.join("sudo"), "exit 0");
        let log = temp.path().join("systemctl.log");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let run = |selected: Option<&str>| {
            let mut command = Command::cargo_bin("gah").unwrap();
            command
                .env("HOME", temp.path())
                .env("XDG_CONFIG_HOME", &config)
                .env("CARGO_HOME", &cargo)
                .env("PATH", &path)
                .env("GAH_TEST_SYSTEMCTL_LOG", &log)
                .args(["update", "--role", "worker", "--yes", "--repo"])
                .arg(&repo);
            if let Some(selected) = selected {
                command.args(["--agent", selected]);
            }
            command.assert().success();
        };
        run(Some(agent));
        let assets = if agent == "opencode" {
            vec![
                config.join("opencode/agents/gah-reviewer.md"),
                config.join("opencode/agents/gah-implementer.md"),
            ]
        } else {
            vec![
                config.join("systemd/user/gah-quota-refresh.service"),
                config.join("systemd/user/gah-quota-refresh.timer"),
            ]
        };
        let originals: Vec<_> = assets
            .iter()
            .map(|asset| std::fs::read_to_string(asset).unwrap())
            .collect();
        // Either surviving asset is enough to recognize an existing install.
        std::fs::remove_file(&assets[0]).unwrap();
        std::fs::write(&assets[1], "stale asset").unwrap();
        run(None);
        for (asset, original) in assets.iter().zip(originals) {
            assert_eq!(std::fs::read_to_string(asset).unwrap(), original);
        }
        if agent != "opencode" {
            let log = std::fs::read_to_string(&log).unwrap();
            assert_eq!(
                log.lines()
                    .filter(|line| *line == "--user enable --now gah-quota-refresh.timer")
                    .count(),
                2
            );
        } else {
            assert!(!config
                .join("systemd/user/gah-quota-refresh.service")
                .exists());
        }
    }
}
