use super::*;

#[test]
fn profile_show_displays_effective_validation_timeout() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "profile",
            "show",
            "test-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("validation_timeout_seconds: 300"));
}

#[test]
fn profile_set_validation_timeout_round_trip() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);

    bin()
        .args([
            "profile",
            "set",
            "test-repo",
            "--validation-timeout-seconds",
            "900",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated profile 'test-repo'"));

    bin()
        .args([
            "profile",
            "show",
            "test-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("validation_timeout_seconds: 900"));
}

#[test]
fn profile_set_validation_timeout_clears_to_default_when_unset() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);

    bin()
        .args([
            "profile",
            "set",
            "test-repo",
            "--validation-timeout-seconds",
            "900",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated profile 'test-repo'"));

    bin()
        .args([
            "profile",
            "set",
            "test-repo",
            "--clear",
            "validation_timeout_seconds",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated profile 'test-repo'"));

    bin()
        .args([
            "profile",
            "show",
            "test-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("validation_timeout_seconds: 300"));
}

#[test]
fn profile_list_json_reports_effective_validation_timeout() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);

    bin()
        .args([
            "profile",
            "list",
            "--json",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""validation_timeout_seconds":300"#,
        ));
}

#[test]
fn profile_set_rejects_zero_validation_timeout() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);

    bin()
        .args([
            "profile",
            "set",
            "test-repo",
            "--validation-timeout-seconds",
            "0",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("0 is not in 1.."));
}

#[test]
fn profile_set_keeps_config_path_as_a_compatibility_alias() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);

    bin()
        .args([
            "profile",
            "set",
            "test-repo",
            "--validation-timeout-seconds",
            "900",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
fn profile_add_persists_validation_timeout() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);

    bin()
        .args([
            "profile",
            "add",
            "new-repo",
            "--display-name",
            "New Repo",
            "--repo-id",
            "new-repo",
            "--provider",
            "github",
            "--repo",
            "owner/new-repo",
            "--local-path",
            "/tmp/new-repo",
            "--artifact-root",
            "/tmp/gah-test-artifacts/new-repo",
            "--validation-timeout-seconds",
            "900",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success();

    bin()
        .args([
            "profile",
            "show",
            "new-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("validation_timeout_seconds: 900"));
}

#[test]
fn profile_list_shows_configured_profiles() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args(["profile", "list", "--config", cfg.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("test-repo"))
        .stdout(predicate::str::contains("Test Repo"))
        .stdout(predicate::str::contains("github"));
}

#[test]
fn profile_show_displays_all_fields() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "profile",
            "show",
            "test-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("default_target_branch: main"))
        .stdout(predicate::str::contains("provider:              github"))
        .stdout(predicate::str::contains("claude_args"));
}

#[test]
fn profile_show_unknown_profile_fails_with_hint() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "profile",
            "show",
            "no-such-profile",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("test-repo"));
}

#[test]
fn profile_show_displays_validation_commands() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config_with_validation(&tmp);
    bin()
        .args([
            "profile",
            "show",
            "validated-repo",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("validation_commands"))
        .stdout(predicate::str::contains("cargo test --quiet"))
        .stdout(predicate::str::contains("cargo clippy -- -D warnings"));
}
