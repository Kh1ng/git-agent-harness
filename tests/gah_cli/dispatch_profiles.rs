use super::*;

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
fn dispatch_dry_run_improve_prints_plan() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("DRY RUN"))
        .stdout(predicate::str::contains("origin/main"))
        .stdout(predicate::str::contains("gah/test-repo-"));
}

#[test]
fn dispatch_dry_run_shows_backend_in_plan() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--backend",
            "claude",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude"));
}

#[test]
fn dispatch_dry_run_shows_oh_profile_when_given() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--oh-profile",
            "some-profile",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("some-profile"));
}

#[test]
fn dispatch_dry_run_pm_mode_prints_pm_steps() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "pm",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pm-report.md"));
}

#[test]
fn dispatch_unknown_mode_fails() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "bogus-mode",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .failure();
}

fn write_dispatch_config_with_validation(tmp: &TempDir) -> std::path::PathBuf {
    let cfg = tmp.path().join("gah-config-validation.toml");
    fs::write(
        &cfg,
        r#"
[defaults]
artifact_root = "/tmp/gah-test-artifacts"
worktree_base = "/tmp/gah-test-worktrees"
llm_base_url  = "http://localhost:4000"
llm_model_local = "local/test"
llm_model_cloud = "cloud/test"

[profiles.validated-repo]
display_name          = "Validated Repo"
repo_id               = "validated-repo"
provider              = "github"
repo                  = "owner/validated-repo"
local_path            = "/tmp/nonexistent-repo"
artifact_root         = "/tmp/gah-test-artifacts/validated-repo"
default_target_branch = "main"
validation_commands   = ["cargo test --quiet", "cargo clippy -- -D warnings"]
"#,
    )
    .unwrap();
    cfg
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

#[test]
fn dispatch_dry_run_shows_validation_commands() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config_with_validation(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "validated-repo",
            "--mode",
            "improve",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Validation"))
        .stdout(predicate::str::contains("cargo test --quiet"));
}

#[test]
fn dispatch_dry_run_shows_retries_in_plan() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config_with_validation(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "validated-repo",
            "--mode",
            "improve",
            "--retries",
            "3",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Retries:      3"));
}

#[test]
fn dispatch_dry_run_candidate_json_target_labeled() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    let fake_candidates = tmp.path().join("candidates.json");
    // Write a minimal valid candidates.json so build_task identifies it
    fs::write(
        &fake_candidates,
        r#"{"counts":{"seen":1,"converted":1,"skipped_warning":0},"candidates":[]}"#,
    )
    .unwrap();
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--target",
            fake_candidates.to_str().unwrap(),
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("candidate JSON"));
}

#[test]
fn dispatch_dry_run_allow_draft_fail_shown() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--allow-draft-fail",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Allow draft fail: true"));
}

#[test]
fn dispatch_dry_run_oh_profile_shows_model() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--oh-profile",
            "my-profile",
            "--model",
            "custom/model-name",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("OH profile:   my-profile"))
        .stdout(predicate::str::contains(
            "Model override: custom/model-name",
        ));
}

#[test]
fn dispatch_dry_run_model_override_shows_custom_model() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--model",
            "custom/test-model",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("custom/test-model"));
}

#[test]
fn dispatch_dry_run_oh_profile_does_not_pass_profile_flag() {
    let tmp = test_tempdir();
    let cfg = write_dispatch_config(&tmp);
    // The dry-run output must not contain "--profile" as an OpenHands argument.
    // It only shows the GAH --oh-profile flag which is a different thing.
    let output = bin()
        .args([
            "dispatch",
            "--profile",
            "test-repo",
            "--mode",
            "improve",
            "--oh-profile",
            "some-profile",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    // GAH --oh-profile IS shown in dry-run output
    assert!(stdout.contains("some-profile"), "oh-profile should appear");
    // But there should be no mention of --profile being passed to openhands
    // The dry-run shows: openhands --headless --json -t ... (no --profile)
    let openhands_line = stdout.lines().find(|l| l.contains("openhands --headless"));
    if let Some(line) = openhands_line {
        assert!(
            !line.contains("--profile"),
            "OpenHands arg line must not contain --profile"
        );
    }
}
