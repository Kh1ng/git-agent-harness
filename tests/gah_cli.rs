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

fn add_origin_and_feature_commit(repo: &std::path::Path) {
    let bare = repo.parent().unwrap().join("origin.git");
    ProcessCommand::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["remote", "add", "origin", bare.to_str().unwrap()])
        .current_dir(repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["checkout", "-b", "feature/review"])
        .current_dir(repo)
        .output()
        .unwrap();
    fs::write(repo.join("src.txt"), "changed\n").unwrap();
    ProcessCommand::new("git")
        .args(["add", "src.txt"])
        .current_dir(repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["commit", "-m", "feature change"])
        .current_dir(repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "feature/review"])
        .current_dir(repo)
        .output()
        .unwrap();
}

fn checkout_branch(repo: &std::path::Path, branch: &str) {
    ProcessCommand::new("git")
        .args(["checkout", branch])
        .current_dir(repo)
        .output()
        .unwrap();
}

fn configure_git_url_instead_of(home: &std::path::Path, from: &str, to: &str) {
    fs::write(
        home.join(".gitconfig"),
        format!("[url \"{}\"]\n\tinsteadOf = {}\n", to, from),
    )
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

/// TICKET-069: CLI-level smoke test for `gah availability`. The module's
/// own tests cover eligibility/list_scopes logic exhaustively; this just
/// proves the subcommand is actually wired to GAH_AVAILABILITY_PATH and
/// produces the expected human/JSON shapes end to end.
#[test]
fn availability_human_and_json_views() {
    let tmp = tempfile::tempdir().unwrap();
    let state_path = tmp.path().join("availability.json");
    fs::write(
        &state_path,
        r#"{"version":1,"records":[
            {"backend":"claude","status":"unavailable","reason":"quota_exhausted","observed_at":"2026-07-04T13:00:00Z","unavailable_until":"2099-01-01T00:00:00Z","source":"backend_error","last_error_summary":"quota exhausted"},
            {"backend":"codex","status":"available","reason":"unknown","observed_at":"2026-07-04T13:00:00Z","source":"manual"}
        ]}"#,
    )
    .unwrap();

    bin()
        .args(["availability"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("claude"))
        .stdout(predicate::str::contains("unavailable"))
        .stdout(predicate::str::contains("quota_exhausted"))
        .stdout(predicate::str::contains("codex"))
        .stdout(predicate::str::contains("available"));

    let out = bin()
        .args(["availability", "--json"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let rows = parsed.as_array().unwrap();
    let claude = rows.iter().find(|r| r["backend"] == "claude").unwrap();
    assert_eq!(claude["eligible"], false);
    assert_eq!(claude["reason"], "quota_exhausted");
    assert_eq!(claude["source"], "backend_error");
    let codex = rows.iter().find(|r| r["backend"] == "codex").unwrap();
    assert_eq!(codex["eligible"], true);
}

#[test]
fn availability_with_no_state_file_reports_eligible_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let state_path = tmp.path().join("does-not-exist.json");

    bin()
        .args(["availability"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("eligible by default"));

    bin()
        .args(["availability", "--json"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success()
        .stdout("[]\n");
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
fn dispatch_records_effective_model_for_routed_runs() {
    let tmp = tempfile::tempdir().unwrap();
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
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "claude");

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
fn dispatch_pm_skips_unavailable_preferred_backend() {
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
    let claude_marker = tmp.path().join("claude-launched.txt");
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\ntouch '{}'\nprintf '%s\n' '{{\"title\":\"Wrong\",\"summary\":\"Wrong\",\"tickets\":[]}}'\n",
            claude_marker.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf '%s\n' '{\"title\":\"Plan\",\"summary\":\"Summary\",\"tickets\":[{\"title\":\"Fallback ticket\",\"summary\":\"Handled by codex fallback\",\"difficulty\":\"easy\",\"risk\":\"low\",\"recommended_backend\":\"codex\",\"duplicate_evidence\":[],\"affected_files\":[],\"acceptance_criteria\":[\"ticket exists\"],\"verification_commands\":[\"test -f docs/tickets\"],\"uncovered_reason\":\"No duplicate work found.\"}]}'\n",
    );

    let availability_path = tmp.path().join("availability.json");
    fs::write(
        &availability_path,
        "{\"version\":1,\"records\":[{\"backend\":\"claude\",\"status\":\"unavailable\",\"reason\":\"quota_exhausted\",\"observed_at\":\"2099-01-01T00:00:00Z\",\"unavailable_until\":\"2099-01-02T00:00:00Z\",\"source\":\"backend_error\"}]}",
    )
    .unwrap();
    let ledger_path = tmp.path().join("ledger.jsonl");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "pm",
            "--target",
            "Plan fallback work",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("Created 1 ticket"));

    assert!(
        !claude_marker.exists(),
        "preferred unavailable backend should not be launched"
    );
    let ledger = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "codex");
    assert_eq!(entry["fallback_used"], true);
    assert!(entry["routing_reason"]
        .as_str()
        .unwrap()
        .contains("quota_exhausted"));
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

#[test]
fn review_writes_structured_verdict_and_posts_comment() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE_STRONG\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"number\":7}]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"number\":7,\"url\":\"https://github.com/owner/real/pull/7\",\"title\":\"Draft: [GAH] Fix\",\"body\":\"MR body\",\"headRefName\":\"feature/review\",\"baseRefName\":\"main\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"SUCCESS\"}]}'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"comment\" ]; then exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"edit\" ]; then exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "feature/review",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();

    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let report = fs::read_to_string(session.join("review-report.md")).unwrap();
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(report.contains("Review notes"));
    assert!(verdict.contains("\"verdict\": \"APPROVE_STRONG\""));
    assert!(verdict.contains("\"reviewer_backend\": \"claude\""));
    assert!(prompt.contains("Source: feature/review"));
    assert!(prompt.contains("Target: main"));
    assert!(prompt.contains("Changed files:\nsrc.txt"));
}

#[test]
fn review_gitlab_posts_comment_by_branch_and_adds_ready_label() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "gitlab",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    let curl_log = tmp.path().join("curl.log");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE_STRONG\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"]}\nEOF\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "curl",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\ncase \"$*\" in\n  *\"merge_requests?state=opened&source_branch=feature/review\"*)\n    printf '%s\\n' '[{{\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"iid\":7}}]'\n    ;;\n  *\"/merge_requests/7/notes\"*)\n    printf '%s\\n' '{{\"id\":1}}'\n    ;;\n  *\"/merge_requests/7\"*)\n    printf '%s\\n' '{{\"iid\":7}}'\n    ;;\n  *)\n    printf '%s\\n' '{{}}'\n    ;;\n esac\n",
            curl_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "feature/review",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITLAB_PAT", "token")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Resolved MR: https://gitlab.example.com/owner/real/-/merge_requests/7",
        ));

    let curl_log = fs::read_to_string(curl_log).unwrap();
    assert!(curl_log.contains("merge_requests?state=opened&source_branch=feature/review"));
    assert!(curl_log.contains("/merge_requests/7/notes"));
    assert!(curl_log.contains("add_labels\":\"gah-ready-for-human\""));
}

#[test]
fn review_by_mr_uses_provider_metadata_even_when_repo_is_on_main() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt-mr.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "gitlab",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE_STRONG\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "curl",
        "#!/bin/sh\ncase \"$*\" in\n  *\"/merge_requests/7\"*) printf '%s\\n' '{\"iid\":7,\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"source_branch\":\"feature/review\",\"target_branch\":\"main\",\"title\":\"Draft: [GAH] Fix\",\"description\":\"MR body\",\"detailed_merge_status\":\"mergeable\"}' ;;\n  *\"merge_requests?state=opened&source_branch=feature/review\"*) printf '%s\\n' '[{\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"iid\":7,\"source_branch\":\"feature/review\",\"target_branch\":\"main\",\"title\":\"Draft: [GAH] Fix\",\"description\":\"MR body\",\"detailed_merge_status\":\"mergeable\"}]' ;;\n  *\"/merge_requests/7/notes\"*) printf '%s\\n' '{\"id\":1}' ;;\n  *) printf '%s\\n' '{}' ;;\n esac\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--mr",
            "7",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITLAB_PAT", "token")
        .assert()
        .success();

    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("MR: 7"));
    assert!(prompt.contains("Source: feature/review"));
    assert!(prompt.contains("Target: main"));
    assert!(prompt.contains("MR title: Draft: [GAH] Fix"));
    assert!(prompt.contains("MR body:\nMR body"));
}

#[test]
fn review_uses_profile_repo_not_current_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let worktree = tmp.path().join("review-wt");
    let prompt_log = tmp.path().join("review-prompt-worktree.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    ProcessCommand::new("git")
        .args([
            "worktree",
            "add",
            worktree.to_str().unwrap(),
            "feature/review",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE_STRONG\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"number\":7}]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"number\":7,\"url\":\"https://github.com/owner/real/pull/7\",\"title\":\"Draft: [GAH] Fix\",\"body\":\"MR body\",\"headRefName\":\"feature/review\",\"baseRefName\":\"main\",\"statusCheckRollup\":[]}'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"comment\" ]; then exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"edit\" ]; then exit 0; fi\nexit 0\n",
    );

    bin()
        .current_dir(&worktree)
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "feature/review",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();

    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("Source: feature/review"));
}

#[test]
fn review_empty_diff_fails_loudly() {
    let tmp = tempfile::tempdir().unwrap();
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
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "claude");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "main",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .failure()
        .stderr(predicate::str::contains("empty review diff"))
        .stderr(predicate::str::contains("profile.local_path"))
        .stderr(predicate::str::contains("source branch: main"))
        .stderr(predicate::str::contains("target branch: main"));
}

#[test]
fn fix_mode_uses_ticket_title_in_mr_title() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let github_root = tmp.path().join("github-root");
    let origin = github_root.join("owner/real.git");
    let ticket = repo.join("docs/tickets/TICKET-058-descriptive-mr-titles.md");
    let gh_log = tmp.path().join("gh.log");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&home).unwrap();
    init_git_repo(&repo);
    fs::create_dir_all(origin.parent().unwrap()).unwrap();
    ProcessCommand::new("git")
        .args(["init", "--bare", origin.to_str().unwrap()])
        .output()
        .unwrap();
    configure_git_url_instead_of(
        &home,
        "https://github.com/",
        &format!("file://{}/", github_root.display()),
    );
    ProcessCommand::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/owner/real.git",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&repo)
        .env("HOME", &home)
        .output()
        .unwrap();
    fs::create_dir_all(ticket.parent().unwrap()).unwrap();
    fs::write(
        &ticket,
        "# TICKET-058: Descriptive Title Here\n\nDifficulty: easy\nRisk: low\n",
    )
    .unwrap();

    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nimprove_backend = \"codex\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'ticket context update\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf '%s\\n' 'https://github.com/owner/real/pull/7'; exit 0; fi\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            ticket.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", home)
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success();

    let gh_log = fs::read_to_string(gh_log).unwrap();
    assert!(gh_log.contains("--title Draft: [GAH] Fix: TICKET-058 Descriptive Title Here"));
    assert!(gh_log.contains("Backend/model: `codex` / `local/test`"));
    assert!(gh_log.contains("Ticket: TICKET-058 Descriptive Title Here"));
}

/// Sets up a real local repo pushed to a bare "origin.git" that GitHub-style
/// URLs are redirected to via git's `insteadOf`, matching the pattern in
/// `fix_mode_uses_ticket_title_in_mr_title`. Returns (repo, home, cfg).
fn setup_fix_dispatch_repo(
    tmp: &TempDir,
    extra_profile: &str,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let github_root = tmp.path().join("github-root");
    let origin = github_root.join("owner/real.git");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&home).unwrap();
    init_git_repo(&repo);
    fs::create_dir_all(origin.parent().unwrap()).unwrap();
    ProcessCommand::new("git")
        .args(["init", "--bare", origin.to_str().unwrap()])
        .output()
        .unwrap();
    configure_git_url_instead_of(
        &home,
        "https://github.com/",
        &format!("file://{}/", github_root.display()),
    );
    ProcessCommand::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/owner/real.git",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&repo)
        .env("HOME", &home)
        .output()
        .unwrap();

    // Plain keys must appear before the nested [profiles.real.routing] table
    // in TOML, or they get parsed as belonging to that subtable instead.
    let cfg = write_real_repo_config_with_extra(
        tmp,
        &repo,
        "github",
        &format!(
            "{}\n[profiles.real.routing]\nimprove_backend = \"codex\"\n",
            extra_profile
        ),
        "",
    );
    (repo, home, cfg)
}

fn branch_exists_on_bare_origin(github_root: &std::path::Path, branch: &str) -> bool {
    let origin = github_root.join("owner/real.git");
    let out = ProcessCommand::new("git")
        .args(["branch", "--list", branch])
        .current_dir(&origin)
        .output()
        .unwrap();
    !String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

/// Priority-3 coverage: validation that never passes must not produce a
/// false success. No push, no MR, and the CLI itself must exit nonzero with
/// actionable output — the exact class of silent-waste bug this harness
/// exists to prevent (see baseline-validation work in dispatch.rs).
#[test]
fn dispatch_fix_validation_never_passes_records_no_push_no_mr() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"false\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
            "--retries",
            "0",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("validation failed after"));

    // No MR was ever attempted.
    assert!(!gh_log.exists() || !fs::read_to_string(&gh_log).unwrap().contains("pr create"));

    // The push never happened: the branch GAH created does not exist on origin.
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["push_attempted"], false);
    assert_eq!(entry["push_succeeded"], false);
    assert_eq!(entry["mr_attempted"], false);
    assert_eq!(entry["mr_created"], false);
    assert!(entry["error_summary"]
        .as_str()
        .unwrap()
        .contains("validation failed"));
    let branch = entry["branch"].as_str().unwrap();
    assert!(!branch_exists_on_bare_origin(
        &repo.parent().unwrap().join("github-root"),
        branch
    ));
}

/// TICKET-064, test 1: a one-shot success (no validation failures at all)
/// must record exactly one attempt, started and completed.
#[test]
fn dispatch_fix_one_shot_success_records_one_attempt() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["attempts_started"], 1);
    assert_eq!(entry["attempts_completed"], 1);
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["attempt_number"], 1);
    assert_eq!(attempts[0]["validation_result"], "passed");
    assert_eq!(attempts[0]["failure_class"], Value::Null);
}

/// TICKET-064, test 2: an attempt that fails validation (differently from
/// baseline, so it retries) followed by a passing attempt must record
/// exactly two attempts, with attempt 1's failure and attempt 2's success
/// both preserved — not just the final outcome.
#[test]
fn dispatch_fix_fail_then_success_records_two_attempts() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"cat marker.txt; grep -q '^done$' marker.txt\"]\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let counter = tmp.path().join("codex-call-count");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{counter}' ] && cat '{counter}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{counter}'\nif [ \"$n\" -eq 1 ]; then echo partial > marker.txt; else echo done > marker.txt; fi\nexit 0\n",
            counter = counter.display(),
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the marker file",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["attempts_started"], 2);
    assert_eq!(entry["attempts_completed"], 2);
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["attempt_number"], 1);
    assert_eq!(attempts[0]["validation_result"], "failed");
    assert_eq!(attempts[0]["failure_class"], "validation_failure");
    assert!(attempts[0]["diff_path"]
        .as_str()
        .unwrap()
        .contains("attempt-diff.patch"));
    assert_eq!(attempts[1]["attempt_number"], 2);
    assert_eq!(attempts[1]["validation_result"], "passed");
}

/// TICKET-064, test 3: a no-progress abort (TICKET-062) must record exactly
/// the attempts that were actually consumed, not the full retry budget.
/// `--retries 2` gives 3 attempts available; only 1 should be consumed
/// since attempt 1 already matches the baseline.
#[test]
fn dispatch_fix_no_progress_abort_records_exact_consumed_attempts() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"false\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["attempts_started"], 1);
    assert_eq!(entry["attempts_completed"], 1);
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["failure_class"], "agent_no_progress");
    assert_eq!(entry["failure_class"], "agent_no_progress");
}

/// TICKET-062: a validation failure identical to the pristine-tree baseline
/// on attempt 1 must abort immediately — there is no "previous attempt" yet
/// to compare against, so the old prev_failure-only comparison couldn't
/// catch this and would burn a second paid attempt for free. `--retries 2`
/// (3 attempts available) proves only ONE was actually consumed: no
/// attempt-2 session directory is ever created.
#[test]
fn dispatch_fix_aborts_on_first_attempt_when_failure_matches_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"false\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );

    let out = bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("pristine-tree baseline"));

    // Only attempt-1 ever ran. If the baseline/previous-attempt distinction
    // regressed back to prev_failure-only comparison, this would burn a
    // second attempt before aborting and attempt-2 would exist.
    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(session_dir.join("attempt-1").exists());
    assert!(!session_dir.join("attempt-2").exists());

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["push_attempted"], false);
    let _ = out;
}

/// TICKET-062, test case 4: an "expected red" ticket — where the baseline
/// is genuinely broken and the ticket's job is to fix it — must still be
/// able to succeed. Attempt 1 changes the failure text (real progress, not
/// a no-op), so it must retry rather than abort; attempt 2 then passes.
#[test]
fn dispatch_fix_expected_red_baseline_can_still_succeed() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"cat marker.txt; grep -q '^done$' marker.txt\"]\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    // marker.txt does not exist on the pristine branch, so the baseline
    // validation fails ("No such file or directory"). The fake backend
    // tracks its own call count in a file outside the worktree (the
    // worktree gets git-reset between attempts) and writes progressively
    // closer output: attempt 1 writes "partial" (still fails, but with
    // different captured output than the missing-file baseline — real
    // progress); attempt 2 writes "done" (passes).
    let counter = tmp.path().join("codex-call-count");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{counter}' ] && cat '{counter}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{counter}'\nif [ \"$n\" -eq 1 ]; then echo partial > marker.txt; else echo done > marker.txt; fi\nexit 0\n",
            counter = counter.display(),
        ),
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
            gh_log.display()
        ),
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the marker file",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(session_dir.join("attempt-1").exists());
    assert!(session_dir.join("attempt-2").exists());

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["validation_result"], "passed");
    assert!(gh_log.exists());
    assert!(fs::read_to_string(&gh_log).unwrap().contains("pr create"));
    let _ = repo;
}

/// TICKET-063: a representative dispatch failure (backend exits nonzero)
/// must populate structured failure_class/failure_stage on the ledger
/// entry, not just the old free-text error_summary.
#[test]
fn dispatch_fix_backend_nonzero_exit_records_structured_failure_attribution() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(&fake_bin, "codex", "#!/bin/sh\nexit 1\n");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "backend_error");
    assert_eq!(entry["failure_stage"], "agent_run");
}

/// Priority-3 coverage: the git push can genuinely succeed while the
/// provider CLI (MR creation) fails afterward. That is a real partial
/// completion, not a false success — the ledger must show push_succeeded
/// true and mr_created false, and the CLI must still exit nonzero with the
/// provider's own error text surfaced.
#[test]
fn dispatch_fix_provider_cli_nonzero_after_successful_push() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\necho 'insufficient permission to create pr' >&2\nexit 1\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "insufficient permission to create pr",
        ));

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["push_attempted"], true);
    assert_eq!(entry["push_succeeded"], true);
    assert_eq!(entry["mr_attempted"], true);
    assert_eq!(entry["mr_created"], false);
    let branch = entry["branch"].as_str().unwrap();
    assert!(branch_exists_on_bare_origin(
        &repo.parent().unwrap().join("github-root"),
        branch
    ));
}

#[test]
fn dispatch_dry_run_ticket_metadata_feeds_routing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let ticket = tmp.path().join("ticket.md");
    fs::write(
        &ticket,
        "Difficulty: medium\nRisk: low\nRecommended backend: codex\nRecommended model: test-model\n",
    )
    .unwrap();
    let cfg = write_real_repo_config_with_extra(&tmp, &repo, "github", "", "");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "codex");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "improve",
            "--target",
            ticket.to_str().unwrap(),
            "--dry-run",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("Effective:    codex"));
}

#[test]
fn sync_classifies_open_gah_prs() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-1\",\"url\":\"https://example/pr/1\",\"labels\":[{\"name\":\"gah-ready-for-human\"}],\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[]}]'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("READY_FOR_HUMAN"))
        .stdout(predicate::str::contains(
            "recommended: human review and merge decision",
        ));
}

/// Build a fake `glab` that responds to `mr list` with the given JSON body
/// and exits 0. Anything else exits 0 with no output.
fn make_fake_glab(dir: &std::path::Path, mr_list_json: &str) {
    make_fake_bin_with_body(
        dir,
        "glab",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"mr\" ] && [ \"$2\" = \"list\" ]; then echo '{}'; exit 0; fi\nexit 0\n",
            mr_list_json.replace('\'', "'\\''"),
        ),
    );
}

#[test]
fn sync_gitlab_classifies_open_mr() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_glab(
        &fake_bin,
        r#"[{"title":"[GAH] fix","source_branch":"gah/test-1","web_url":"https://example/mr/1","labels":["gah-ready-for-human"],"state":"opened","merged_at":null,"updated_at":"2099-01-01T00:00:00Z"}]"#,
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("READY_FOR_HUMAN"))
        .stdout(predicate::str::contains("gah/test-1"));
}

#[test]
fn sync_gitlab_classifies_merged_mr() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_glab(
        &fake_bin,
        r#"[{"title":"[GAH] fix","source_branch":"gah/test-2","web_url":"https://example/mr/2","labels":[],"state":"merged","merged_at":"2099-01-01T00:00:00Z","updated_at":"2099-01-01T00:00:00Z"}]"#,
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("MERGED"))
        .stdout(predicate::str::contains("gah/test-2"))
        .stdout(predicate::str::contains("recommended: none"));
}

/// Documents current behavior, not a spec: sync.rs does not parse the
/// GitLab MR `state` field, so a closed-and-unmerged MR is indistinguishable
/// from an open one and is classified the same way (NEEDS_REVIEW here,
/// since it carries no gah-* label and isn't stale). This is a known gap,
/// not a fix — see TODO.md for the failure-taxonomy work. If this test ever
/// starts failing because classify() now reads `state`, that's progress;
/// update the assertion, don't just delete the test.
#[test]
fn sync_gitlab_closed_unmerged_mr_is_indistinguishable_from_open() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_glab(
        &fake_bin,
        r#"[{"title":"[GAH] fix","source_branch":"gah/test-3","web_url":"https://example/mr/3","labels":[],"state":"closed","merged_at":null,"updated_at":"2099-01-01T00:00:00Z"}]"#,
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("NEEDS_REVIEW"))
        .stdout(predicate::str::contains("gah/test-3"));
}

#[test]
fn sync_gitlab_malformed_json_fails_loudly() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        "#!/bin/sh\nif [ \"$1\" = \"mr\" ] && [ \"$2\" = \"list\" ]; then echo 'not json at all'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .failure();
}

#[test]
fn sync_gitlab_no_matching_mr() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        "#!/bin/sh\nif [ \"$1\" = \"mr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(!stdout.contains("gah/test"));
}

#[test]
fn sync_gitlab_fails_when_glab_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", "")
        .assert()
        .failure()
        .stderr(predicate::str::contains("glab mr list"));
}

#[test]
fn sync_gitlab_fails_when_glab_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        "#!/bin/sh\necho \"API ERROR\" >&2\nexit 1\n",
    );

    bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .failure()
        .stderr(predicate::str::contains("API ERROR"));
}

// ── TDD: machine-readable state for autonomous manager agents ──────────────
// These define the contract for junior-agent tickets. Remove #[ignore] when
// implementing.

#[test]
fn sync_json_outputs_machine_readable_classification() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-1\",\"url\":\"https://example/pr/1\",\"labels\":[{\"name\":\"gah-ready-for-human\"}],\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[]}]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let mrs = parsed.as_array().expect("top level must be an array");
    assert_eq!(mrs[0]["classification"], "READY_FOR_HUMAN");
    assert_eq!(mrs[0]["branch"], "gah/test-1");
    assert!(mrs[0]["recommended_action"].is_string());
    assert_eq!(mrs[0]["url"], "https://example/pr/1");
}

/// TICKET-070: the JSON view must expose the richer fields the ticket
/// requires ("at minimum": MR identifier, state, draft, merge status), not
/// just the classification/recommendation floor the contract test checks.
/// Also verifies --json prints ONLY JSON — no "Profile: ..." human header
/// mixed into stdout, since that would break every consumer that expects
/// pure JSON on stdout.
#[test]
fn sync_json_includes_id_state_draft_and_merge_status() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-1\",\"url\":\"https://example/pr/1\",\"labels\":[],\"number\":42,\"state\":\"OPEN\",\"isDraft\":true,\"mergeStateStatus\":\"BEHIND\",\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[]}]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "sync",
            "--profile",
            "real",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(
        !stdout.contains("Profile:"),
        "--json must print only JSON, not the human header: {stdout}"
    );
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    let mrs = parsed.as_array().unwrap();
    assert_eq!(mrs[0]["id"], "42");
    assert_eq!(mrs[0]["state"], "OPEN");
    assert_eq!(mrs[0]["draft"], true);
    assert_eq!(mrs[0]["merge_status"], "BEHIND");
    assert_eq!(mrs[0]["profile"], "real");
}

#[test]
#[ignore = "TICKET: gah ledger summary --json"]
fn ledger_summary_json_outputs_machine_readable_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    // Empty ledger: still valid JSON with zero counts
    fs::write(&ledger_path, "").unwrap();
    let out = bin()
        .args([
            "ledger",
            "summary",
            "--since",
            "7d",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_LEDGER_PATH", ledger_path.to_str().unwrap())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(parsed["entries"], 0);
    assert!(parsed["by_mode"].is_object());
    assert!(parsed["by_backend"].is_object());
}
