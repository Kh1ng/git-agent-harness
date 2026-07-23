use super::*;

#[test]
fn init_prints_profile_snippet() {
    bin()
        .args([
            "init",
            "--profile",
            "sample",
            "--display-name",
            "Sample Repo",
            "--provider",
            "github",
            "--repo",
            "owner/sample",
            "--local-path",
            "/tmp/sample",
            "--print",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[profiles.sample]"))
        .stdout(predicate::str::contains("provider = \"github\""));
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

#[test]
fn dispatch_pm_writes_ledger_entry() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "pm",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success();

    let ledger = tmp.path().join("artifacts/ledger.jsonl");
    let text = fs::read_to_string(ledger).unwrap();
    assert!(text.contains("\"profile\":\"real\""));
    assert!(text.contains("\"mode\":\"pm\""));
}

#[test]
fn dispatch_records_effective_model_for_routed_runs() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let bare = repo.parent().unwrap().join("origin.git");
    ProcessCommand::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["remote", "add", "origin", bare.to_str().unwrap()])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let ticket = tmp.path().join("ticket.md");
    fs::write(
        &ticket,
        "# Ticket\n\nRecommended backend: claude\nRecommended model: claude-sonnet-4\n",
    )
    .unwrap();
    // This test verifies route attribution, not publication. The fixture
    // deliberately uses an illustrative GitHub remote, so keep publication
    // disabled once the backend creates the minimal diff required by the
    // no-progress guard.
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.publishing]\nallow_pull_request_creation = false\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "improve",
            "--target",
            ticket.to_str().unwrap(),
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();

    let ledger = tmp.path().join("artifacts/ledger.jsonl");
    let text = fs::read_to_string(ledger).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "claude");
    assert_eq!(entry["effective_model"], "claude-sonnet-4");
}
