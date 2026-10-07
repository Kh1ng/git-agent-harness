use super::*;

#[test]
fn text_output_still_passes_for_a_valid_profile() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"))
        .stdout(predicate::str::contains("manager memory"));
}

#[test]
fn emits_structured_readiness_checks_without_text_noise() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");

    let output = bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let snapshot: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(snapshot["schema_version"], 1);
    assert!(matches!(
        snapshot["overall_status"].as_str(),
        Some("ok" | "warn")
    ));
    assert!(snapshot["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|check| {
            check["profile"] == "real"
                && check["name"] == "manager memory"
                && check["status"] == "ok"
        })
    }));
}

#[test]
fn doctor_warns_but_passes_when_manager_memory_is_missing() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    fs::remove_file(repo.join("docs/MANAGER_MEMORY.md")).unwrap();
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .stdout(predicate::str::contains("[WARN] manager memory"))
        .stdout(predicate::str::contains("[FAIL]").not());
}

#[test]
fn doctor_validate_warns_when_nothing_extra_configured() {
    // TICKET-076: no validation_commands, no env_file, no routing backend
    // configured -- --validate must WARN, not FAIL, and doctor must still
    // pass overall (matches plain `doctor`'s existing passing behavior).
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--validate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("[WARN]").and(predicate::str::contains("validation commands")),
        )
        .stdout(predicate::str::contains("backend executables"));
}

#[test]
fn doctor_validate_fails_on_unresolvable_validation_command() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "validation_commands = [\"definitely-not-a-real-tool-xyz test\"]\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--validate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .failure()
        .stdout(
            predicate::str::contains("[FAIL]").and(predicate::str::contains("validation command")),
        );
}

#[test]
fn doctor_validate_fails_on_missing_env_file() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "env_file = \"/definitely/does/not/exist.env\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--validate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .failure()
        .stdout(predicate::str::contains("[FAIL]").and(predicate::str::contains("env_file")));
}

#[test]
fn doctor_validate_fails_on_missing_backend_executable_but_plain_doctor_still_passes() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "codex_path = \"/definitely/does/not/exist/codex\"\n[profiles.real.routing]\ndefault_backend = \"codex\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");
    // An explicit (nonexistent) codex_path override makes this deterministic
    // regardless of whether the real dev machine happens to have a codex
    // binary on PATH.

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success();

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--validate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .failure()
        .stdout(
            predicate::str::contains("[FAIL]").and(predicate::str::contains("backend executable")),
        );
}

/// Issue #1366: the worktree-base check is a defaults-level check that must
/// run even when the config has no profiles, so an unwritable base fails
/// doctor instead of reporting an overall "ok".
#[test]
fn doctor_json_reports_unwritable_worktree_base_with_no_profiles() {
    let tmp = test_tempdir();
    let unwritable = tmp.path().join("worktree-base-is-a-file");
    fs::write(&unwritable, "regular file, not a directory").unwrap();
    let cfg = tmp.path().join("gah-config-empty.toml");
    fs::write(
        &cfg,
        format!("[defaults]\nworktree_base = \"{}\"\n", unwritable.display()),
    )
    .unwrap();

    let output = bin()
        .args(["doctor", "--config-path", cfg.to_str().unwrap(), "--json"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let snapshot: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(snapshot["overall_status"], "fail");
    assert!(snapshot["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|check| {
            check["name"] == "worktree_base"
                && check["status"] == "fail"
                && check.get("profile").is_none()
        })
    }));
}

/// Issue #1366: an empty `worktree_base` passes doctor, which reports the
/// default that dispatch will use, and the check creates nothing under HOME.
#[test]
fn doctor_json_reports_the_resolved_default_for_an_empty_worktree_base() {
    let tmp = test_tempdir();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let cfg = tmp.path().join("gah-config-empty.toml");
    fs::write(&cfg, "[defaults]\nworktree_base = \"\"\n").unwrap();

    let output = bin()
        .args(["doctor", "--config-path", cfg.to_str().unwrap(), "--json"])
        .env("HOME", &home)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let snapshot: Value = serde_json::from_slice(&output).unwrap();

    let resolved = home.join(".local/share/gah/worktrees");
    assert!(snapshot["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|check| {
            check["name"] == "worktree_base"
                && check["status"] == "ok"
                && check["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains(resolved.to_str().unwrap()))
        })
    }));
    assert!(!home.join(".local").exists());
}

/// The write probe can land in a directory GAH does not own, so it must not
/// touch a file that already has the old fixed probe name, nor write through
/// a symlink with that name.
#[cfg(unix)]
#[test]
fn doctor_worktree_base_probe_leaves_existing_files_alone() {
    let tmp = test_tempdir();
    let parent = tmp.path().join("parent");
    fs::create_dir_all(&parent).unwrap();
    let target = tmp.path().join("symlink-target");
    fs::write(&target, "target contents").unwrap();
    let cfg = tmp.path().join("gah-config-empty.toml");

    for base in [parent.clone(), parent.join("not-created-yet")] {
        let probe = parent.join(".gah-write-test");
        let _ = fs::remove_file(&probe);
        fs::write(&probe, "operator file").unwrap();
        fs::write(
            &cfg,
            format!("[defaults]\nworktree_base = \"{}\"\n", base.display()),
        )
        .unwrap();
        let doctor = || {
            bin()
                .args(["doctor", "--config-path", cfg.to_str().unwrap()])
                .assert()
                .success();
        };

        doctor();
        assert_eq!(fs::read_to_string(&probe).unwrap(), "operator file");

        fs::remove_file(&probe).unwrap();
        std::os::unix::fs::symlink(&target, &probe).unwrap();
        doctor();
        assert!(fs::symlink_metadata(&probe).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "target contents");
    }
    // Only the operator's entry is left: the probe cleaned up after itself.
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
}

/// A regular file above the worktree base blocks creating it, so doctor must
/// fail instead of probing the directory that holds the file.
#[test]
fn doctor_fails_for_a_worktree_base_below_a_regular_file() {
    let tmp = test_tempdir();
    let file = tmp.path().join("not-a-directory");
    fs::write(&file, "regular file").unwrap();
    let cfg = tmp.path().join("gah-config-empty.toml");
    fs::write(
        &cfg,
        format!(
            "[defaults]\nworktree_base = \"{}\"\n",
            file.join("worktrees").display()
        ),
    )
    .unwrap();

    bin()
        .args(["doctor", "--config-path", cfg.to_str().unwrap()])
        .assert()
        .failure()
        .stdout(predicate::str::contains("[FAIL]").and(predicate::str::contains("worktree_base")));
}

/// TICKET-105: `gah doctor --validate` reuses the exact same
/// `review_preflight` check as the real review invocation.
#[test]
fn doctor_validate_reports_missing_review_capability() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\nreview_required_capabilities = { claude = [\"ponytail\"] }\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");
    make_fake_bin(&fake_bin, "claude");

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--validate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .assert()
        .failure()
        .stdout(
            predicate::str::contains("[FAIL]")
                .and(predicate::str::contains("review capabilities"))
                .and(predicate::str::contains("required capability missing")),
        );

    // Installing the plugin makes the same check pass.
    fs::create_dir_all(home.join(".claude/plugins/cache/ponytail")).unwrap();
    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--validate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("[PASS]").and(predicate::str::contains("review capabilities")),
        );
}
