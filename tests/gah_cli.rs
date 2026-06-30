use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command as ProcessCommand;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("gah").unwrap()
}

fn write_fixture_dir() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();

    let scout_dir = tmp.path().join("scout");
    fs::create_dir_all(&scout_dir).unwrap();

    let scout = include_str!("fixtures/scout_readme_missing.json");
    fs::write(scout_dir.join("scout.json"), scout).unwrap();

    let gate_dir = tmp.path().join("gate");
    fs::create_dir_all(&gate_dir).unwrap();

    let gate = include_str!("fixtures/gate_readme_warn_sparse.json")
        .replace("__SCOUT_ARTIFACT__", scout_dir.to_str().unwrap());
    fs::write(gate_dir.join("gate.json"), gate).unwrap();

    let watchlist = include_str!("fixtures/model_watchlist.json");
    fs::write(tmp.path().join("model_watchlist.json"), watchlist).unwrap();

    tmp
}

fn latest_child_dir(root: &std::path::Path) -> std::path::PathBuf {
    let mut dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.pop().unwrap()
}

fn make_fake_bin(dir: &std::path::Path, name: &str) {
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

fn make_fake_bin_with_body(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

fn prepend_path(dir: &std::path::Path) -> String {
    let old = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), old)
}

fn init_git_repo(path: &std::path::Path) {
    fs::create_dir_all(path.join("docs")).unwrap();
    ProcessCommand::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .output()
        .unwrap();
    fs::write(path.join("README.md"), "hello\n").unwrap();
    fs::write(path.join("docs/MANAGER_MEMORY.md"), "# Memory\n").unwrap();
    ProcessCommand::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(path)
        .output()
        .unwrap();
}

fn write_real_repo_config(
    tmp: &TempDir,
    repo: &std::path::Path,
    provider: &str,
) -> std::path::PathBuf {
    write_real_repo_config_with_extra(tmp, repo, provider, "", "")
}

fn write_real_repo_config_with_extra(
    tmp: &TempDir,
    repo: &std::path::Path,
    provider: &str,
    extra_profile: &str,
    extra_defaults: &str,
) -> std::path::PathBuf {
    let cfg = tmp.path().join("gah-config-real.toml");
    let extra = match provider {
        "gitlab" => "provider_api_base = \"https://gitlab.example.com/api/v4\"\nprovider_project_id = \"42\"\n",
        _ => "",
    };
    fs::write(
        &cfg,
        format!(
            r#"
[defaults]
artifact_root = "{root}/artifacts"
worktree_base = "{root}/worktrees"
llm_base_url  = "http://localhost:4000"
llm_model_local = "local/test"
llm_model_cloud = "cloud/test"
{extra_defaults}

[profiles.real]
display_name          = "Real Repo"
repo_id               = "real"
provider              = "{provider}"
repo                  = "owner/real"
local_path            = "{repo}"
artifact_root         = "{root}/artifacts/real"
default_target_branch = "main"
{extra}
{extra_profile}
"#,
            root = tmp.path().display(),
            provider = provider,
            repo = repo.display(),
            extra = extra,
            extra_profile = extra_profile,
            extra_defaults = extra_defaults,
        ),
    )
    .unwrap();
    cfg
}

#[test]
fn help_works() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("git agent harness"));
}

#[test]
fn warn_candidates_are_skipped_by_default() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    bin()
        .args([
            "candidates",
            "--gate-artifact",
            gate.to_str().unwrap(),
            "--out-root",
            out_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let artifact = latest_child_dir(&out_root.join("scout-to-backlog-candidates"));
    let data: Value =
        serde_json::from_str(&fs::read_to_string(artifact.join("candidates.json")).unwrap())
            .unwrap();

    assert_eq!(data["counts"]["seen"], 1);
    assert_eq!(data["counts"]["converted"], 0);
    assert_eq!(data["counts"]["skipped_warning"], 1);
    assert_eq!(data["candidates"].as_array().unwrap().len(), 0);
}

#[test]
fn warn_candidates_are_included_with_flag_and_hydrated_from_scout() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    bin()
        .args([
            "candidates",
            "--gate-artifact",
            gate.to_str().unwrap(),
            "--include-warnings",
            "--out-root",
            out_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let artifact = latest_child_dir(&out_root.join("scout-to-backlog-candidates"));
    let data: Value =
        serde_json::from_str(&fs::read_to_string(artifact.join("candidates.json")).unwrap())
            .unwrap();

    assert_eq!(data["counts"]["converted"], 1);
    let c = &data["candidates"][0];

    assert_eq!(c["candidate_id"], "001");
    assert_eq!(c["source_gate_status"], "warn");
    assert_eq!(c["suggested_blueprint_phase"], "needs:human");
    assert_eq!(c["provider_mutation_allowed"], false);

    let labels = c["suggested_labels"].as_array().unwrap();
    assert!(labels.iter().any(|v| v == "type:docs"));
    assert!(labels.iter().any(|v| v == "risk:low"));
    assert!(labels.iter().any(|v| v == "needs:human-review"));
    assert!(!labels.iter().any(|v| v == "agent:ready"));

    assert!(c["affected_files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "README.md"));
    assert!(!c["evidence"].as_array().unwrap().is_empty());
    assert!(!c["acceptance_criteria"].as_array().unwrap().is_empty());
    assert!(!c["verification"].as_array().unwrap().is_empty());

    assert_eq!(c["hydration_used"], true);
    assert_eq!(c["hydration_match_method"], "id");
}

#[test]
fn candidate_artifacts_are_unique_and_never_overwritten() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    for _ in 0..2 {
        bin()
            .args([
                "candidates",
                "--gate-artifact",
                gate.to_str().unwrap(),
                "--include-warnings",
                "--out-root",
                out_root.to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    let root = out_root.join("scout-to-backlog-candidates");
    let dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();

    assert_eq!(dirs.len(), 2);
    assert_ne!(dirs[0], dirs[1]);
}

#[test]
fn price_guard_allows_active_default() {
    let tmp = write_fixture_dir();
    let watchlist = tmp.path().join("model_watchlist.json");

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "deepseek/deepseek-v4-flash",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("allowed"));
}

#[test]
fn price_guard_blocks_unavailable_model() {
    let tmp = write_fixture_dir();
    let watchlist = tmp.path().join("model_watchlist.json");

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "qwen/qwen3-235b-a22b-2507",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn work_trust_mode_blocks_provider_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("work-readonly.toml");

    fs::write(
        &cfg,
        r#"
[repo]
name = "work/private-repo"
provider = "github"
trust_mode = "read_only"
allow_provider_mutation = false
allow_push = false
allow_draft_pr = false
allow_issue_write = false
allow_project_write = false
"#,
    )
    .unwrap();

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "open-draft-pr",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn personal_draft_pr_mode_allows_only_draft_pr() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("personal-draft.toml");

    fs::write(
        &cfg,
        r#"
[repo]
name = "personal/repo"
provider = "github"
trust_mode = "draft_pr_allowed"
allow_provider_mutation = true
allow_push = true
allow_draft_pr = true
allow_issue_write = false
allow_project_write = false
"#,
    )
    .unwrap();

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "open-draft-pr",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("allowed"));

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "edit-issue",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

// ── dispatch / profile regression tests ──────────────────────────────────────

fn write_dispatch_config(tmp: &TempDir) -> std::path::PathBuf {
    let cfg = tmp.path().join("gah-config.toml");
    fs::write(
        &cfg,
        r#"
[defaults]
artifact_root = "/tmp/gah-test-artifacts"
worktree_base = "/tmp/gah-test-worktrees"
llm_base_url  = "http://localhost:4000"
llm_model_local = "local/test"
llm_model_cloud = "cloud/test"

[profiles.test-repo]
display_name          = "Test Repo"
repo_id               = "test-repo"
provider              = "github"
repo                  = "owner/test-repo"
local_path            = "/tmp/nonexistent-repo"
artifact_root         = "/tmp/gah-test-artifacts/test-repo"
default_target_branch = "main"
claude_args           = ["--allowedTools", "Edit,Write,Bash"]
"#,
    )
    .unwrap();
    cfg
}

#[test]
fn profile_list_shows_configured_profiles() {
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
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
fn doctor_passes_for_valid_profile() {
    let tmp = tempfile::tempdir().unwrap();
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
fn doctor_fails_when_manager_memory_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
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
fn dispatch_pm_writes_ledger_entry() {
    let tmp = tempfile::tempdir().unwrap();
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
fn prune_dry_run_reports_old_sessions_and_worktrees() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let session = tmp.path().join("artifacts/real/sessions/20240101");
    fs::create_dir_all(&session).unwrap();

    let worktree_root = tmp.path().join("worktrees");
    fs::create_dir_all(&worktree_root).unwrap();
    let worktree = worktree_root.join("gah-real-old");
    ProcessCommand::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "gah/real-old",
            worktree.to_str().unwrap(),
            "HEAD",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();

    ProcessCommand::new("touch")
        .args([
            "-t",
            "202401010000",
            session.to_str().unwrap(),
            worktree.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    bin()
        .args([
            "prune",
            "--profile",
            "real",
            "--older-than",
            "1",
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("would remove session"))
        .stdout(predicate::str::contains("would remove worktree"));
}

#[test]
fn dispatch_pm_target_parses_structured_plan_and_writes_ticket() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\npm_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\nprintf '%s\n' '{\"title\":\"Plan\",\"summary\":\"Summary\",\"tickets\":[{\"title\":\"Fix auth\",\"summary\":\"Tighten auth checks\",\"difficulty\":\"easy\",\"risk\":\"low\",\"recommended_backend\":\"codex\",\"duplicate_evidence\":[],\"affected_files\":[\"src/auth.rs\"],\"acceptance_criteria\":[\"auth rejects invalid token\"],\"verification_commands\":[\"pytest tests/test_auth.py -x\"],\"uncovered_reason\":\"No open MR or ticket covers this auth edge case.\"}]}'\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "pm",
            "--target",
            "Plan auth work",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("Created 1 ticket"));

    let tickets_dir = repo.join("docs/tickets");
    let entries: Vec<_> = fs::read_dir(&tickets_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(entries.iter().any(|name| name.contains("fix-auth")));
}

#[test]
fn ledger_summary_reports_recent_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("gah.toml");
    fs::write(
        &cfg,
        format!(
            r#"
[defaults]
artifact_root = "{root}/artifacts"
worktree_base = "{root}/worktrees"
llm_base_url = ""
llm_model_local = ""
llm_model_cloud = ""
"#,
            root = tmp.path().display()
        ),
    )
    .unwrap();
    let ledger_dir = tmp.path().join("artifacts");
    fs::create_dir_all(&ledger_dir).unwrap();
    fs::write(
        ledger_dir.join("ledger.jsonl"),
        "{\"timestamp\":\"2099-01-01T00:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"claude\",\"requested_backend\":\"claude\",\"effective_backend\":\"claude\",\"requested_model\":null,\"effective_model\":null,\"routing_reason\":\"explicit\",\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"mode\":\"pm\",\"target_summary\":\"x\",\"branch\":null,\"session_dir\":null,\"duration_seconds\":1.0,\"backend_exit_code\":0,\"validation_result\":\"not_run\",\"commit_attempted\":false,\"commit_created\":false,\"push_attempted\":false,\"push_succeeded\":false,\"mr_attempted\":false,\"mr_created\":false,\"mr_url\":null,\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"usage_source\":null}}\n",
    )
    .unwrap();

    bin()
        .args([
            "ledger",
            "summary",
            "--since",
            "7d",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Entries: 1"))
        .stdout(predicate::str::contains("By mode:"))
        .stdout(predicate::str::contains("pm"));
}
