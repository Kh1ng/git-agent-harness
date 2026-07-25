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
fn doctor_fails_when_manager_memory_is_missing() {
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
        .failure()
        .stdout(predicate::str::contains("[FAIL]"))
        .stdout(predicate::str::contains("manager memory"));
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
