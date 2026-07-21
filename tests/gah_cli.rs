#[path = "gah_cli/already_satisfied.rs"]
mod already_satisfied;
#[path = "gah_cli/args.rs"]
mod args;
#[path = "gah_cli/availability.rs"]
mod availability_cli;
#[path = "gah_cli/conflict_resolution.rs"]
mod conflict_resolution;
#[path = "gah_cli/doctor_json.rs"]
mod doctor_json;
#[path = "gah_cli/gitlab_review.rs"]
mod gitlab_review;
#[path = "gah_cli/pm.rs"]
mod pm;
#[path = "gah_cli/review_format_retry.rs"]
mod review_format_retry;
#[path = "gah_cli/route_approval.rs"]
mod route_approval;
#[path = "gah_cli/stall_retry.rs"]
mod stall_retry;
mod support;
#[path = "gah_cli/validation_gate.rs"]
mod validation_gate;
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use support::{isolate_command, test_tempdir, FakeBackend, IsolatedCommand, Scenario};
use tempfile::TempDir;
fn bin() -> IsolatedCommand<Command> {
    let cmd = Command::cargo_bin("gah").unwrap();
    // CLI integration tests may run under the real systemd loop, which sets
    // XDG_STATE_HOME to the operator's persistent state directory. Never let
    // fake profiles and work claims leak into (or inherit from) that state.
    isolate_command(cmd, |cmd, root| {
        let tmp = root.join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        cmd.env("XDG_STATE_HOME", root.join("xdg-state"));
        cmd.env("GAH_AVAILABILITY_PATH", root.join("availability.json"));
        cmd.env(
            "GAH_VALIDATION_CHECK_PATH",
            root.join("validation-check.json"),
        );
        cmd.env("TMPDIR", tmp);
    })
}

fn spawn_bin() -> IsolatedCommand<ProcessCommand> {
    let cmd = ProcessCommand::new(
        std::env::var("CARGO_BIN_EXE_gah").unwrap_or_else(|_| "target/debug/gah".into()),
    );
    isolate_command(cmd, |cmd, root| {
        let tmp = root.join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        cmd.env("XDG_STATE_HOME", root.join("xdg-state"));
        cmd.env("GAH_AVAILABILITY_PATH", root.join("availability.json"));
        cmd.env(
            "GAH_VALIDATION_CHECK_PATH",
            root.join("validation-check.json"),
        );
        cmd.env("TMPDIR", tmp);
    })
}

fn write_fixture_dir() -> TempDir {
    let tmp = test_tempdir();

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

fn make_fake_github_review_api(dir: &std::path::Path) {
    make_fake_bin_with_body(
        dir,
        "gh",
        r#"#!/bin/sh
case "$1 $2 $3 $4" in
  "api --method GET repos/owner/real/pulls") printf '[{"number":7}]\n' ;;
  "pr view 7 --repo") printf '{"number":7,"url":"https://github.com/owner/real/pull/7","title":"Draft: [GAH] Fix","body":"MR body","headRefName":"feature/review","baseRefName":"main","statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}]}\n' ;;
  "api --method GET repos/owner/real/issues/7/comments") printf '[]\n' ;;
  "api --method POST repos/owner/real/issues/7/comments") exit 0 ;;
  "api repos/owner/real/issues/7/labels --jq "*) exit 0 ;;
  "api repos/owner/real/issues/7/labels -f "*) exit 0 ;;
  "api --method DELETE repos/owner/real/issues/7/labels/"*) exit 0 ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
}

fn prepend_path(dir: &std::path::Path) -> String {
    let old = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), old)
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: i32) {
    unsafe {
        let _ = libc::kill(pid as i32, signal);
    }
}

fn wait_for_backend_call(backend: &FakeBackend, child: &mut std::process::Child, call: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while backend.call_count() < call {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("child exited before backend call {call} started: {status:?}");
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for backend call {call} to start");
        }
        thread::sleep(Duration::from_millis(20));
    }
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
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
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

#[test]
fn prune_dry_run_reports_old_sessions_and_worktrees() {
    let tmp = test_tempdir();
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
fn prune_retains_dirty_worktree_even_after_retention() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let worktree_root = tmp.path().join("worktrees");
    fs::create_dir_all(&worktree_root).unwrap();
    let worktree = worktree_root.join("gah-real-dirty");
    ProcessCommand::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "gah/real-dirty",
            worktree.to_str().unwrap(),
            "HEAD",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    fs::write(worktree.join("README.md"), "unpublished recovery work\n").unwrap();

    bin()
        .args([
            "prune",
            "--profile",
            "real",
            "--older-than",
            "0",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("retained dirty worktree"));

    assert!(worktree.exists());
}

#[test]
fn ledger_summary_reports_recent_counts() {
    let tmp = test_tempdir();
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

/// Data source for the frontend attempt-timeline view (Work detail page).
/// Thin wrapper around `ledger::entries_for_work_id` -- this test proves
/// the CLI wiring (flag parsing, filtering by work_id, JSON shape), not the
/// filtering logic itself, which already has its own unit tests in
/// src/ledger/mod.rs.
#[test]
fn ledger_work_filters_to_one_work_id_and_supports_json() {
    let tmp = test_tempdir();
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
        "{\"timestamp\":\"2026-07-04T10:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"codex\",\"requested_backend\":\"codex\",\"effective_backend\":\"codex\",\"requested_model\":null,\"effective_model\":\"gpt-5.4\",\"routing_reason\":null,\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"mode\":\"fix\",\"target_summary\":null,\"work_id\":\"TICKET-042\",\"branch\":\"gah/test-1\",\"session_dir\":null,\"duration_seconds\":42.0,\"backend_exit_code\":0,\"validation_result\":\"passed\",\"commit_attempted\":true,\"commit_created\":true,\"push_attempted\":true,\"push_succeeded\":true,\"mr_attempted\":true,\"mr_created\":true,\"mr_url\":\"https://example/pr/1\",\"files_changed\":3,\"insertions\":10,\"deletions\":2,\"error_summary\":null,\"usage\":{\"input_tokens\":100,\"output_tokens\":50,\"total_tokens\":150,\"estimated_cost_usd\":0.02,\"actual_cost_usd\":null,\"usage_source\":\"codex\"}}\n{\"timestamp\":\"2026-07-04T11:00:00Z\",\"session_id\":\"2\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"codex\",\"requested_backend\":\"codex\",\"effective_backend\":\"codex\",\"requested_model\":null,\"effective_model\":null,\"routing_reason\":null,\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"mode\":\"fix\",\"target_summary\":null,\"work_id\":\"TICKET-999-other\",\"branch\":null,\"session_dir\":null,\"duration_seconds\":null,\"backend_exit_code\":0,\"validation_result\":null,\"commit_attempted\":false,\"commit_created\":false,\"push_attempted\":false,\"push_succeeded\":false,\"mr_attempted\":false,\"mr_created\":false,\"mr_url\":null,\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"actual_cost_usd\":null,\"usage_source\":null}}\n",
    )
    .unwrap();

    bin()
        .args([
            "ledger",
            "work",
            "TICKET-042",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 entries"))
        .stdout(predicate::str::contains("codex/gpt-5.4"))
        .stdout(predicate::str::contains("$0.0200"))
        .stdout(predicate::str::contains("TICKET-999-other").not());

    let out = bin()
        .args([
            "ledger",
            "work",
            "TICKET-042",
            "--config-path",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let entries = parsed.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["work_id"], "TICKET-042");
    assert_eq!(entries[0]["usage"]["estimated_cost_usd"], 0.02);
}

#[test]
fn ledger_work_with_no_matching_entries_reports_none_found() {
    let tmp = test_tempdir();
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

    bin()
        .args([
            "ledger",
            "work",
            "TICKET-does-not-exist",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("No ledger entries found"));
}

#[test]
fn review_routes_to_agy_candidate_and_writes_verdict() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "agy",
        &format!(
            "#!/bin/sh\nprintf '%s\n' \"$@\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_candidates = [{ backend = \"agy\", model = \"Claude Sonnet 4.6 (Thinking)\" }, { backend = \"claude\" }]\n",
        "",
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
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
    assert!(verdict.contains("\"reviewer_backend\": \"agy\""));
    assert!(verdict.contains("\"reviewer_model\": \"Claude Sonnet 4.6 (Thinking)\""));
    assert!(prompt.contains("--print"));
    assert!(prompt.contains("--model"));
    assert!(prompt.contains("Claude Sonnet 4.6 (Thinking)"));
}

/// Regression: review mode used to fail the whole dispatch outright on an
/// empty-output AGY failure (quota exhaustion, exit=0) even when
/// review_candidates listed a real fallback -- the candidate list was
/// consulted once for the initial route, then never touched again. Fakes
/// AGY returning empty stdout with a RESOURCE_EXHAUSTED cli.log (the exact
/// live failure signature) and confirms review actually falls through to
/// the next candidate instead of erroring.
#[test]
fn review_falls_back_to_next_candidate_on_agy_empty_output() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_home = tmp.path().join("fake-home");
    fs::create_dir_all(fake_home.join(".gemini/antigravity-cli")).unwrap();
    fs::write(
        fake_home.join(".gemini/antigravity-cli/cli.log"),
        "E0000 00:00:00.000000 1 log.go:398] RESOURCE_EXHAUSTED (code 429): \
         Individual quota reached. Resets in 114h2m37s.\n",
    )
    .unwrap();

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    // Exit 0, empty stdout -- the real live failure mode, not a nonzero exit.
    make_fake_bin_with_body(&fake_bin, "agy", "#!/bin/sh\nexit 0\n");
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}\nEOF\n",
    );
    make_fake_github_review_api(&fake_bin);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_candidates = [{ backend = \"agy\", model = \"Claude Sonnet 4.6 (Thinking)\" }, { backend = \"claude\" }]\n",
        "",
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
        .env("HOME", &fake_home)
        .env(
            "GAH_AVAILABILITY_PATH",
            tmp.path().join("availability.json"),
        )
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Backend unavailable; retrying review",
        ));

    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
    assert!(verdict.contains("\"reviewer_backend\": \"claude\""));
}

/// AGY's subscription CLI may emit quota exhaustion only on stderr with a
/// nonzero exit.  That failure must make the exact review route unavailable
/// before selecting the fallback; otherwise every loop cycle repeats AGY.
#[test]
fn review_falls_back_when_agy_quota_is_only_on_stderr() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "agy",
        "#!/bin/sh\nprintf 'Individual quota reached. Resets in 2h 15m.\\n' >&2\nexit 23\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}\nEOF\n",
    );
    make_fake_github_review_api(&fake_bin);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_candidates = [{ backend = \"agy\", model = \"Claude Sonnet 4.6 (Thinking)\" }, { backend = \"claude\" }]\n",
        "",
    );
    let availability_path = tmp.path().join("availability.json");

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
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Backend unavailable; retrying review with claude instead of agy/Claude Sonnet 4.6 (Thinking)",
        ));

    let availability = fs::read_to_string(availability_path).unwrap();
    assert!(availability.contains("Claude Sonnet 4.6 (Thinking)"));
    assert!(availability.contains("quota_exhausted"));
    let session = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"reviewer_backend\": \"claude\""));
}

#[test]
fn review_uses_explicit_claude_path() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    let explicit_claude = tmp.path().join("tools/claude-explicit");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(explicit_claude.parent().unwrap()).unwrap();
    make_fake_bin_with_body(
        explicit_claude.parent().unwrap(),
        "claude-explicit",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        &format!(
            "claude_path = \"{}\"\n[profiles.real.routing]\nreview_backend = \"claude\"\n",
            explicit_claude.display()
        ),
        "",
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

    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("Source: feature/review"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

fn setup_review_repo_and_gh(
    tmp: &TempDir,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_github_review_api(&fake_bin);
    (repo, fake_bin, tmp.path().join("home"))
}

/// TICKET-109/105: reviewing with a required-but-uninstalled capability must
/// stop the review outright, not silently degrade to an ordinary one.
/// Uses an isolated HOME with no `.claude/plugins/cache/` at all, so this
/// doesn't depend on whether the real dev machine happens to have Ponytail
/// installed.
#[test]
fn review_fails_when_required_capability_not_installed() {
    let tmp = test_tempdir();
    let (repo, fake_bin, home) = setup_review_repo_and_gh(&tmp);
    fs::create_dir_all(&home).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\necho should never run\nexit 1\n",
    );
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\nreview_required_capabilities = { claude = [\"ponytail\"] }\n",
        "",
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
        .env("HOME", &home)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("ponytail").and(predicate::str::contains("not installed")),
        );
}

/// TICKET-109: when the required capability IS installed (fake plugin-cache
/// directory under an isolated HOME), the review prompt must contain the
/// activation text, and the verdict must record it in applied_capabilities.
#[test]
fn review_activates_and_records_capability_when_installed() {
    let tmp = test_tempdir();
    let (repo, fake_bin, home) = setup_review_repo_and_gh(&tmp);
    fs::create_dir_all(home.join(".claude/plugins/cache/ponytail")).unwrap();
    let prompt_log = tmp.path().join("review-prompt.txt");
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\nreview_required_capabilities = { claude = [\"ponytail\"] }\n",
        "",
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
        .env("HOME", &home)
        .assert()
        .success();

    let prompt = fs::read_to_string(&prompt_log).unwrap();
    assert!(prompt.starts_with("/ponytail full"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict_text = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    let verdict: Value = serde_json::from_str(&verdict_text).unwrap();
    assert_eq!(verdict["verdict"], serde_json::json!("APPROVE"));
    assert_eq!(
        verdict["applied_capabilities"],
        serde_json::json!(["ponytail"])
    );
}

/// TICKET-105: a capability that IS installed (plugin dir present) but that
/// GAH has no known activation mapping for must refuse with "reviewer
/// degraded", not silently run an ordinary review.
#[test]
fn review_fails_as_degraded_when_capability_has_no_known_activation() {
    let tmp = test_tempdir();
    let (repo, fake_bin, home) = setup_review_repo_and_gh(&tmp);
    fs::create_dir_all(home.join(".claude/plugins/cache/some-future-skill")).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\necho should never run\nexit 1\n",
    );
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\nreview_required_capabilities = { claude = [\"some-future-skill\"] }\n",
        "",
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
        .env("HOME", &home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("reviewer degraded"));
}

#[test]
fn review_parse_failure_preserves_raw_report_and_records_bounded_reroute() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n\n[profiles.real.publishing]\nallow_issue_comments = false\n",
        "",
    );
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false\nEOF\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"number\":7}]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"number\":7,\"url\":\"https://github.com/owner/real/pull/7\",\"title\":\"Draft: [GAH] Fix\",\"body\":\"MR body\",\"headRefName\":\"feature/review\",\"baseRefName\":\"main\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"SUCCESS\"}]}'; exit 0; fi\nexit 0\n",
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
        .success()
        .stdout(predicate::str::contains("bounded reviewer reroute"));
    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let report = fs::read_to_string(session.join("review-report.md")).unwrap();
    assert!(report.contains("Review notes"));
    let verdict: serde_json::Value =
        serde_json::from_slice(&fs::read(session.join("review-verdict.json")).unwrap()).unwrap();
    assert_eq!(verdict["verdict"], "REVIEW_OUTPUT_INVALID");
}

#[test]
fn review_shutdown_records_cancelled_shutdown_and_dispatch_finished_event() {
    let tmp = test_tempdir();
    let (repo, fake_bin, home) = setup_review_repo_and_gh(&tmp);
    fs::create_dir_all(&home).unwrap();
    let claude = FakeBackend::new(tmp.path(), "claude");
    claude.install(Scenario::success().with_delay_ms(30_000));
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    // Keep the isolated environment alive until the spawned process exits.
    let mut command = spawn_bin();
    let mut child = command
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
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    wait_for_backend_call(&claude, &mut child, 1);
    #[cfg(unix)]
    send_signal(child.id(), libc::SIGINT);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(claude.call_count(), 1);

    let ledger_text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(ledger_text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "harness_error");
    assert_eq!(entry["failure_stage"], "review");
    assert_eq!(entry["validation_result"], "cancelled_shutdown");

    let events_text = fs::read_to_string(&events_path).unwrap();
    assert!(events_text.contains("dispatch_started"));
    assert!(events_text.contains("dispatch_finished"));
    assert!(events_text.contains("shutdown requested while claude was running"));
}

/// A review backend failure must propagate through the controller-facing
/// dispatch path.  Otherwise `gah loop --once` records `review: success`
/// even though the ledger correctly says backend_error, which can conceal a
/// failed review from the operator and the next controller observation.
#[test]
fn loop_reports_nonzero_review_backend_as_failure_not_success() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    ProcessCommand::new("git")
        .args(["branch", "gah/real-review", "feature/review"])
        .current_dir(&repo)
        .output()
        .unwrap();
    ProcessCommand::new("git")
        .args(["push", "origin", "gah/real-review"])
        .current_dir(&repo)
        .output()
        .unwrap();
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
        "#!/bin/sh\nprintf 'subscription quota exhausted\\n' >&2\nexit 23\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\ncase \"$4\" in */pulls?*) echo '[{\"title\":\"[GAH] Fix: TICKET-500\",\"body\":\"MR body\",\"head\":{\"ref\":\"gah/real-review\",\"sha\":\"source-sha\"},\"html_url\":\"https://github.com/owner/real/pull/7\",\"labels\":[],\"number\":7,\"state\":\"open\",\"draft\":true,\"updated_at\":\"2026-07-18T17:22:35-05:00\"}]'; exit 0;; */check-runs?*) echo '{\"total_count\":1,\"check_runs\":[{\"status\":\"completed\",\"conclusion\":\"success\"}]}'; exit 0;; esac\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"number\":7,\"url\":\"https://github.com/owner/real/pull/7\",\"title\":\"[GAH] Fix: TICKET-500\",\"body\":\"MR body\",\"headRefName\":\"gah/real-review\",\"baseRefName\":\"main\",\"headRefOid\":\"source-sha\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"SUCCESS\"}]}'; exit 0; fi\nexit 0\n",
    );

    let events_path = tmp.path().join("events.jsonl");
    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "review backend exited with status 23",
        ));

    let events_text = fs::read_to_string(events_path).unwrap();
    assert!(events_text.contains("dispatch_started"));
    assert!(events_text.contains("review backend exited with status 23"));
    assert!(!events_text.contains("review: success"));
}

#[test]
fn review_gitlab_uses_host_scoped_glab_session_without_pat() {
    let tmp = test_tempdir();
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
    let glab_log = tmp.path().join("glab.log");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\ncat <<'EOF'\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}\nEOF\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\ncase \"$1 $2\" in\n  \"api projects/42/merge_requests\")\n    printf '%s\\n' '[{{\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"iid\":7,\"source_branch\":\"feature/review\",\"target_branch\":\"main\"}}]'\n    ;;\n  \"api projects/42/merge_requests/7/notes\") printf '%s\\n' '{{\"id\":1}}' ;;\n  \"api projects/42/merge_requests/7\") printf '%s\\n' '{{\"iid\":7}}' ;;\n  *) echo \"unexpected glab invocation: $*\" >&2; exit 1 ;;\n esac\n",
            glab_log.display()
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
        .env_remove("GITLAB_PAT")
        .env_remove("GITLAB_PAT2")
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Resolved MR: https://gitlab.example.com/owner/real/-/merge_requests/7",
        ));

    let glab_log = fs::read_to_string(glab_log).unwrap();
    assert!(glab_log.contains("--hostname gitlab.example.com"));
    assert!(!glab_log.contains("PRIVATE-TOKEN"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

#[test]
fn review_by_mr_uses_provider_metadata_even_when_repo_is_on_main() {
    let tmp = test_tempdir();
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
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "glab",
        "#!/bin/sh\ncase \"$1 $2\" in\n  \"api projects/42/merge_requests/7\") printf '%s\\n' '{\"iid\":7,\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"source_branch\":\"feature/review\",\"target_branch\":\"main\",\"title\":\"Draft: [GAH] Fix\",\"description\":\"MR body\",\"detailed_merge_status\":\"mergeable\"}' ;;\n  \"api projects/42/merge_requests\") printf '%s\\n' '[{\"web_url\":\"https://gitlab.example.com/owner/real/-/merge_requests/7\",\"iid\":7,\"source_branch\":\"feature/review\",\"target_branch\":\"main\",\"title\":\"Draft: [GAH] Fix\",\"description\":\"MR body\",\"detailed_merge_status\":\"mergeable\"}]' ;;\n  \"api projects/42/merge_requests/7/notes\") printf '%s\\n' '{\"id\":1}' ;;\n  *) echo \"unexpected glab invocation: $*\" >&2; exit 1 ;;\n esac\n",
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
        .env_remove("GITLAB_PAT")
        .assert()
        .success();

    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("MR: 7"));
    assert!(prompt.contains("Source: feature/review"));
    assert!(prompt.contains("Target: main"));
    assert!(prompt.contains("MR title: Draft: [GAH] Fix"));
    assert!(prompt.contains("MR body:\n  MR body"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

#[test]
fn review_uses_profile_repo_not_current_worktree() {
    let tmp = test_tempdir();
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
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);

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

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    let verdict = fs::read_to_string(session_dir.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

#[test]
fn review_empty_diff_fails_loudly() {
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
    let tmp = test_tempdir();
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
        "# TICKET-058: Descriptive Title Here\n\nGoal: Generate a descriptive MR body\nDifficulty: easy\nRisk: low\n\n## Problem\n\nThe old MR body is too sparse.\n",
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
    assert!(gh_log.contains("## Work Item"));
    assert!(gh_log.contains("ID: `TICKET-058`"));
    assert!(gh_log.contains("## Problem"));
    assert!(gh_log.contains("The old MR body is too sparse."));
    assert!(gh_log.contains("## Goal"));
    assert!(gh_log.contains("Generate a descriptive MR body"));
    assert!(gh_log.contains("## Validation"));
    assert!(gh_log.contains("## Backend / Model"));
}

/// Set up a local repo and bare origin for fix-dispatch integration tests.
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
    let tmp = test_tempdir();
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "0",
            // TICKET-111: this test's baseline deliberately fails ("false")
            // to reach post-attempt validation-exhaustion behavior, not to
            // test baseline-stop policy itself.
            "--allow-unknown-red-baseline",
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
    // TICKET-172: validation failure must leave the generated patch on the
    // local dispatch branch for recovery, even though no push/MR occurred.
    let recovered = ProcessCommand::new("git")
        .args(["show", &format!("{branch}:README.md")])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(recovered.status.success(), "WIP branch {branch} was lost");
    assert!(
        String::from_utf8_lossy(&recovered.stdout).contains("agent edit"),
        "terminal validation failure should retain the agent's patch"
    );
}

/// TICKET-237: OpenCode can report provider rate limits only in its own
/// internal log while returning exit 0 and leaving no diff. That must be
/// classified as a backend availability failure, not agent_no_progress, and
/// the bounded retry must select the configured fallback in the same dispatch.
#[test]
fn dispatch_fix_opencode_internal_rate_limit_marks_unavailable_and_reroutes() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let config = fs::read_to_string(&cfg).unwrap().replace(
        "improve_backend = \"codex\"",
        "improve_backend = \"opencode\"\nimprove_candidates = [{ backend = \"opencode\", model = \"opencode/hy3-free\" }, { backend = \"codex\", model = \"gpt-5.4-mini\" }]",
    );
    fs::write(&cfg, config).unwrap();

    let ledger_path = tmp.path().join("ledger.jsonl");
    let availability_path = tmp.path().join("availability.json");
    let data_home = tmp.path().join("xdg-data");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "opencode",
        "#!/bin/sh\nmkdir -p \"$XDG_DATA_HOME/opencode/log\"\nprintf '%s\\n' 'timestamp=now level=ERROR message=\"AI_APICallError: Rate limit exceeded. Please try again later.\"' >> \"$XDG_DATA_HOME/opencode/log/opencode.log\"\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'fallback edit\\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nprintf 'https://github.com/owner/real/pull/1\\n'\n",
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
            "1",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &data_home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Backend unavailable after no-progress result; retrying next attempt with codex/gpt-5.4-mini instead of opencode/opencode/hy3-free",
        ));

    let availability: Value =
        serde_json::from_str(&fs::read_to_string(&availability_path).unwrap()).unwrap();
    let records = availability["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["backend"], "opencode");
    assert_eq!(records[0]["model"], "opencode/hy3-free");
    assert_eq!(records[0]["reason"], "rate_limited");

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "codex");
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["backend"], "opencode");
    assert_eq!(attempts[0]["failure_class"], "backend_error");
    assert_eq!(
        attempts[0]["validation_result"],
        "not_run_backend_unavailable"
    );
    assert_eq!(attempts[1]["backend"], "codex");
    assert_eq!(attempts[1]["validation_result"], "passed");
}

#[test]
fn dispatch_reroute_continues_partial_tree_after_billing_exhaustion() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let config = fs::read_to_string(&cfg).unwrap().replace(
        "improve_backend = \"codex\"",
        "improve_backend = \"opencode\"\nimprove_candidates = [{ backend = \"opencode\", model = \"opencode/hy3-free\" }, { backend = \"codex\", model = \"gpt-5.4-mini\" }]",
    );
    fs::write(&cfg, config).unwrap();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let availability_path = tmp.path().join("availability.json");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "opencode",
        "#!/bin/sh\nprintf 'opencode-partial-progress\\n' >> README.md\nprintf 'Forbidden: Sorry, your account balance is insufficient\\n' >&2\nexit 1\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\ngrep -q 'opencode-partial-progress' README.md || exit 19\nprintf 'codex-completed-progress\\n' >> README.md\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; fi\nexit 0\n",
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
            "continue rerouted work",
            "--retries",
            "1",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Backend unavailable; retrying next attempt with codex/gpt-5.4-mini instead of opencode/opencode/hy3-free (QuotaExhausted)",
        ));

    let entry: Value = serde_json::from_str(
        fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(entry["attempts"][0]["backend"], "opencode");
    assert_eq!(entry["attempts"][1]["backend"], "codex");
    let branch = entry["branch"].as_str().unwrap();
    let readme = ProcessCommand::new("git")
        .args(["show", &format!("{branch}:README.md")])
        .current_dir(repo)
        .output()
        .unwrap();
    let readme = String::from_utf8_lossy(&readme.stdout);
    assert!(readme.contains("opencode-partial-progress"));
    assert!(readme.contains("codex-completed-progress"));

    let availability: Value =
        serde_json::from_str(&fs::read_to_string(availability_path).unwrap()).unwrap();
    assert_eq!(availability["records"][0]["reason"], "quota_exhausted");
}

/// TICKET-250: no-progress uses the same bounded retry policy as other
/// recoverable agent failures. Every failed no-change attempt remains visible
/// in the ledger, and only the final one marks the dispatch terminally failed.
#[test]
fn dispatch_fix_retries_no_change_before_terminal_failure() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let invocation_log = tmp.path().join("codex-invocations.log");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nprintf 'attempt\\n' >> \"{}\"\nexit 0\n",
            invocation_log.display()
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
            "1",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "attempt 2 but produced no worktree changes",
        ));

    assert_eq!(
        fs::read_to_string(&invocation_log).unwrap().lines().count(),
        2
    );
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "agent_no_progress");
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().all(|attempt| {
        attempt["failure_class"] == "agent_no_progress"
            && attempt["validation_result"] == "not_run_no_changes"
    }));
}

/// TICKET-172: retry cleanup must not destroy a failed attempt's patch. The
/// retry starts clean, so its final WIP belongs on the dispatch branch while
/// the previous attempt remains reachable from a dedicated local checkpoint.
#[test]
fn dispatch_fix_validation_retry_retains_each_failed_wip_tree() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        r#"validation_commands = ["sh -c 'if grep -q \"first attempt\" README.md; then echo first; false; elif grep -q \"second attempt\" README.md; then echo second; false; else echo baseline; false; fi'"]
"#,
    );
    let ledger_path = tmp.path().join("ledger.jsonl");
    let fake_bin = tmp.path().join("bin");
    let invocation_marker = tmp.path().join("agent-ran-once");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nif test -f '{marker}'; then printf 'second attempt\\n' >> README.md; else touch '{marker}'; printf 'first attempt\\n' >> README.md; fi\nexit 0\n",
            marker = invocation_marker.display(),
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
            "1",
            "--skip-validation-gate",
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "validation failed after 2 attempt",
        ));

    let entry: Value = serde_json::from_str(
        fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let dispatch_branch = entry["branch"].as_str().unwrap();
    let dispatch_tree = ProcessCommand::new("git")
        .args(["show", &format!("{dispatch_branch}:README.md")])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(dispatch_tree.status.success());
    assert!(String::from_utf8_lossy(&dispatch_tree.stdout).contains("second attempt"));

    let checkpoints = ProcessCommand::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/gah-wip",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    let checkpoint = String::from_utf8_lossy(&checkpoints.stdout)
        .lines()
        .next()
        .expect("first failed retry should leave a WIP checkpoint")
        .to_string();
    let checkpoint_tree = ProcessCommand::new("git")
        .args(["show", &format!("{checkpoint}:README.md")])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(checkpoint_tree.status.success());
    assert!(String::from_utf8_lossy(&checkpoint_tree.stdout).contains("first attempt"));
}

/// TICKET-064, test 1: a one-shot success (no validation failures at all)
/// must record exactly one attempt, started and completed.
#[test]
fn dispatch_fix_one_shot_success_records_one_attempt() {
    let tmp = test_tempdir();
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

/// TICKET-073: a config change (here, the very first run, since nothing is
/// recorded yet) must trigger exactly one fresh-worktree self-check, record
/// the new hash + last_verified_ok=true, and a *second* dispatch with an
/// unchanged validation_commands list must take the fast path (hash compare
/// only) — no "[validation-gate] commands changed" message, no second worktree
/// spin-up.
#[test]
fn dispatch_runs_validation_gate_once_per_config_change_then_skips() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let state_path = tmp.path().join("validation_check.json");

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
        "#!/bin/sh\nprintf 'https://github.com/owner/real/pull/1\\n'\n",
    );

    let run = || -> assert_cmd::assert::Assert {
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
                "noop",
                "--retries",
                "0",
            ])
            .env("PATH", prepend_path(&fake_bin))
            .env("HOME", &home)
            .env("GITHUB_TOKEN", "token")
            .env("GAH_VALIDATION_CHECK_PATH", &state_path)
            .assert()
    };

    // First dispatch: nothing recorded yet → gate runs, passes, records.
    // The gate logs to stdout.
    let first = run();
    first
        .success()
        .stdout(predicate::str::contains(
            "[validation-gate] commands changed",
        ))
        .stdout(predicate::str::contains("Baseline validation on pristine worktree").not());

    // State now records last_verified_ok = true for profile "real".
    let state_text = fs::read_to_string(&state_path).unwrap();
    assert!(
        state_text.contains("\"last_verified_ok\": true") && state_text.contains("\"real\""),
        "gate should have recorded a passing check: {}",
        state_text
    );

    // Second dispatch: config unchanged → fast path, no gate re-run message.
    // Sleep 1s so the dispatch worktree branch timestamp differs from the
    // first run (the previous worktree is cleaned up but its branch ref
    // lingers until pruned) and the two runs don't collide.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let second = run();
    second
        .success()
        .stdout(predicate::str::contains("[validation-gate] commands changed").not())
        .stdout(predicate::str::contains("Baseline validation on pristine worktree").not());
}

/// TICKET-073: --skip-validation-gate deliberately bypasses the gate even when
/// validation_commands is broken, recording nothing new and letting dispatch
/// proceed (so an operator who has acknowledged a known-broken gate can still
/// dispatch real work).
#[test]
fn dispatch_skip_validation_gate_bypasses_gate() {
    let tmp = test_tempdir();
    // validation_commands passes baseline; we are only testing that the
    // --skip-validation-gate opt-out suppresses the gate self-check entirely.
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let state_path = tmp.path().join("validation_check.json");

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
        "#!/bin/sh\nprintf 'https://github.com/owner/real/pull/1\\n'\n",
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
            "noop",
            "--retries",
            "0",
            "--skip-validation-gate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_VALIDATION_CHECK_PATH", &state_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "[validation-gate] skipped by explicit --skip-validation-gate",
        ));

    // Bypass means no check was recorded for this profile.
    assert!(
        !state_path.exists()
            || !fs::read_to_string(&state_path)
                .unwrap()
                .contains("\"real\""),
        "skipping the gate must not record a check for the profile"
    );
}

/// TICKET-101: usage the backend reports on stdout for a given attempt is
/// captured onto that specific attempt record, not just aggregated
/// somewhere else -- and a backend that reports nothing leaves it
/// genuinely unknown (None), never a fabricated zero.
#[test]
fn dispatch_fix_records_per_attempt_usage_from_backend_output() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nprintf 'input_tokens: 500\\noutput_tokens: 120\\nestimated_cost_usd: 0.02\\n'\nexit 0\n",
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
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["usage"]["input_tokens"], 500);
    assert_eq!(attempts[0]["usage"]["output_tokens"], 120);
    assert_eq!(attempts[0]["usage"]["total_tokens"], 620);
    assert_eq!(attempts[0]["usage"]["estimated_cost_usd"], Value::Null);
}

#[test]
fn dispatch_fix_shutdown_records_cancelled_shutdown_and_dispatch_finished_event() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    let codex = FakeBackend::new(tmp.path(), "codex");
    codex.install(Scenario::success().with_delay_ms(30_000));
    make_fake_bin_with_body(
        &tmp.path().join("bin"),
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
    );

    // Keep the isolated environment alive until the spawned process exits.
    let mut command = spawn_bin();
    let mut child = command
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
        .env("PATH", prepend_path(&tmp.path().join("bin")))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    wait_for_backend_call(&codex, &mut child, 1);
    #[cfg(unix)]
    send_signal(child.id(), libc::SIGINT);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(codex.call_count(), 1);

    let ledger_text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(ledger_text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "harness_error");
    assert_eq!(entry["failure_stage"], "agent_run");
    assert_eq!(entry["validation_result"], "cancelled_shutdown");
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["failure_class"], "harness_error");
    assert_eq!(attempts[0]["failure_stage"], "agent_run");
    assert_eq!(attempts[0]["validation_result"], "cancelled_shutdown");

    let events_text = fs::read_to_string(&events_path).unwrap();
    assert!(events_text.contains("dispatch_started"));
    assert!(events_text.contains("dispatch_finished"));
    assert!(events_text.contains("shutdown requested while codex was running"));
}

/// TICKET-079: --escalate seeds the *initial* route decision (not just an
/// internal retry) as a genuine agent-capability failure, so the same
/// TICKET-089 cost-aware escalation logic picks the stronger candidate on
/// the very first attempt.
#[test]
fn dispatch_fix_escalate_flag_picks_stronger_backend_on_first_attempt() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    // setup_fix_dispatch_repo always appends its own single-line
    // `[profiles.real.routing]` table (a second header would be invalid
    // TOML), so patch the candidate list into that same table afterward
    // instead of trying to inject a second `[profiles.real.routing]`.
    let cfg_text = fs::read_to_string(&cfg).unwrap();
    let cfg_text = cfg_text.replace(
        "improve_backend = \"codex\"",
        "improve_backend = \"codex\"\nimprove_candidates = [{ backend = \"openhands\", model = \"deepseek-flash\" }, { backend = \"codex\", model = \"gpt-5.4\" }]",
    );
    fs::write(&cfg, cfg_text).unwrap();
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
        "openhands",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
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
            "--backend",
            "auto",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "fix the thing",
            "--escalate",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "codex");
    assert_eq!(entry["effective_model"], "gpt-5.4");
}

/// TICKET-064, test 2: an attempt that fails validation (differently from
/// baseline, so it retries) followed by a passing attempt must record
/// exactly two attempts, with attempt 1's failure and attempt 2's success
/// both preserved — not just the final outcome.
#[test]
fn dispatch_fix_fail_then_success_records_two_attempts() {
    let tmp = test_tempdir();
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
            "--skip-validation-gate",
            "--target",
            "fix the marker file",
            "--retries",
            "2",
            // TICKET-111: baseline fails (marker.txt missing on pristine
            // tree) purely to set up the retry-loop scenario under test.
            "--allow-unknown-red-baseline",
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

#[test]
fn dispatch_backend_retry_continues_checkpointed_progress() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let counter = tmp.path().join("codex-call-count");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{counter}' ] && cat '{counter}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{counter}'\nif [ \"$n\" -eq 1 ]; then printf 'first-attempt-progress\\n' >> README.md; exit 17; fi\ngrep -q 'first-attempt-progress' README.md || exit 19\nprintf 'second-attempt-completion\\n' >> README.md\nexit 0\n",
            counter = counter.display(),
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; fi\nexit 0\n",
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
            "--skip-validation-gate",
            "--target",
            "continue partial backend work",
            "--retries",
            "1",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let entry: Value = serde_json::from_str(
        fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(entry["attempts_started"], 2);
    assert_eq!(entry["attempts"][0]["exit_code"], 17);
    assert_eq!(entry["attempts"][1]["validation_result"], "passed");
    let branch = entry["branch"].as_str().unwrap();
    let readme = ProcessCommand::new("git")
        .args(["show", &format!("{branch}:README.md")])
        .current_dir(repo)
        .output()
        .unwrap();
    let readme = String::from_utf8_lossy(&readme.stdout);
    assert!(readme.contains("first-attempt-progress"));
    assert!(readme.contains("second-attempt-completion"));
}

/// TICKET-064, test 3: a no-progress abort (TICKET-062) must record exactly
/// the attempts that were actually consumed, not the full retry budget.
/// `--retries 2` gives 3 attempts available; only 1 should be consumed
/// since attempt 1 already matches the baseline.
#[test]
fn dispatch_fix_no_progress_abort_records_exact_consumed_attempts() {
    let tmp = test_tempdir();
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "2",
            // TICKET-111: baseline fails ("false") to set up the
            // no-progress / attempt-matches-baseline scenario under test.
            "--allow-unknown-red-baseline",
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
    let tmp = test_tempdir();
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "2",
            // TICKET-111: baseline fails ("false") to set up the
            // no-progress / attempt-matches-baseline scenario under test.
            "--allow-unknown-red-baseline",
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
///
/// Also exercises TICKET-110/111's real `BaselineDisposition::ExpectedRed`:
/// the profile explicitly configures `known_baseline_failure_markers`
/// matching the missing-file text, so dispatch proceeds instead of
/// stopping (the default for an unconfigured/unknown_red baseline).
#[test]
fn dispatch_fix_expected_red_baseline_can_still_succeed() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"cat marker.txt; grep -q '^done$' marker.txt\"]\nknown_baseline_failure_markers = [\"marker.txt: No such file or directory\"]\n",
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
            "--skip-validation-gate",
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

/// TICKET-111 AC1: a harness_error baseline (validation command itself
/// cannot run -- POSIX exit 127) must stop dispatch before any attempt runs,
/// regardless of --allow-unknown-red-baseline (that flag only covers
/// unknown_red, not harness/environment errors).
#[test]
fn dispatch_fix_harness_error_baseline_always_stops() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"definitely-not-a-real-command-xyz\"]\n",
    );
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
            "--skip-validation-gate",
            "--target",
            "fix the thing",
            "--retries",
            "2",
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("harness_error"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(
        !session_dir.join("attempt-1").exists(),
        "no attempt should ever run when the baseline is a harness_error"
    );
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "harness_error");
    assert_eq!(entry["failure_stage"], "baseline_validation");
}

/// TICKET-111 AC1: an environment_error baseline (well-known missing-
/// dependency signature) must also stop dispatch before any attempt runs.
#[test]
fn dispatch_fix_environment_error_baseline_stops() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"echo 'ModuleNotFoundError: No module named repo_thing'; exit 1\"]\n",
    );
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
            "--skip-validation-gate",
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
        .stderr(predicate::str::contains("environment_error"));

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(!session_dir.join("attempt-1").exists());
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["failure_class"], "environment_error");
}

/// TICKET-111 AC2: unknown_red (a baseline failure matching none of the
/// known signatures, and not explicitly configured as expected) stops by
/// default -- proving --allow-unknown-red-baseline in the other tests in
/// this file is opting into real, non-default behavior rather than masking
/// a no-op.
#[test]
fn dispatch_fix_unknown_red_baseline_stops_without_override() {
    let tmp = test_tempdir();
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
            "--skip-validation-gate",
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
        .stderr(
            predicate::str::contains("unknown_red")
                .and(predicate::str::contains("--allow-unknown-red-baseline")),
        );

    let session_dir = latest_child_dir(&tmp.path().join("artifacts/real/sessions"));
    assert!(!session_dir.join("attempt-1").exists());
}

/// TICKET-063: a representative dispatch failure (backend exits nonzero)
/// must populate structured failure_class/failure_stage on the ledger
/// entry, not just the old free-text error_summary.
#[test]
fn dispatch_fix_backend_nonzero_exit_records_structured_failure_attribution() {
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let availability_path = tmp.path().join("availability.json");
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
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("Effective:    codex"))
        .stdout(predicate::str::contains("LLM model:").not())
        .stdout(predicate::str::contains("LLM base:").not());
}

#[test]
fn sync_classifies_open_gah_prs() {
    let tmp = test_tempdir();
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

#[test]
fn sync_classifies_closed_unmerged_github_prs() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-closed\",\"url\":\"https://example/pr/closed\",\"labels\":[{\"name\":\"gah-ready-for-human\"}],\"state\":\"CLOSED\",\"isDraft\":true,\"mergeStateStatus\":\"DIRTY\",\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"FAILURE\"}]}]'; exit 0; fi\nexit 0\n",
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
        .stdout(predicate::str::contains("CLOSED_UNMERGED"))
        .stdout(predicate::str::contains("recommended: none"))
        .stdout(predicate::str::contains("gah/test-closed"));
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
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
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

#[test]
fn sync_gitlab_closed_unmerged_mr_is_terminal() {
    let tmp = test_tempdir();
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
        .stdout(predicate::str::contains("CLOSED_UNMERGED"))
        .stdout(predicate::str::contains("recommended: none"))
        .stdout(predicate::str::contains("gah/test-3"));
}

#[test]
fn status_json_excludes_closed_unmerged_history() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] fix\",\"headRefName\":\"gah/test-status\",\"url\":\"https://example/pr/status\",\"labels\":[{\"name\":\"gah-human-review\"}],\"state\":\"closed\",\"isDraft\":true,\"mergeStateStatus\":\"DIRTY\",\"mergedAt\":null,\"updatedAt\":\"2099-01-01T00:00:00Z\",\"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"FAILURE\"}]}]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "status",
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
    let parsed: Value = serde_json::from_str(&stdout).expect("status stdout must be valid JSON");
    let mrs = parsed["merge_requests"]
        .as_array()
        .expect("merge_requests must be an array");
    assert!(mrs.is_empty());
}

#[test]
fn sync_gitlab_malformed_json_fails_loudly() {
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
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
    let tmp = test_tempdir();
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
fn ledger_summary_json_outputs_machine_readable_counts() {
    let tmp = test_tempdir();
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

#[test]
fn ledger_summary_json_includes_model_and_failure_class_breakdown() {
    let tmp = test_tempdir();
    let ledger_path = tmp.path().join("ledger.jsonl");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    fs::write(
        &ledger_path,
        "{\"timestamp\":\"2099-01-01T00:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"claude\",\"requested_backend\":\"claude\",\"effective_backend\":\"claude\",\"requested_model\":null,\"effective_model\":\"claude-sonnet-4\",\"routing_reason\":\"explicit\",\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":true,\"failure_class\":\"agent_failure\",\"mode\":\"pm\",\"target_summary\":\"x\",\"branch\":null,\"session_dir\":null,\"duration_seconds\":1.0,\"backend_exit_code\":0,\"validation_result\":\"not_run\",\"commit_attempted\":false,\"commit_created\":false,\"push_attempted\":false,\"push_succeeded\":false,\"mr_attempted\":false,\"mr_created\":false,\"mr_url\":null,\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"usage_source\":null}}\n",
    )
    .unwrap();

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
    assert_eq!(parsed["entries"], 1);
    assert_eq!(parsed["by_model"]["claude-sonnet-4"], 1);
    assert_eq!(parsed["by_failure_class"]["agent_failure"], 1);
    assert_eq!(parsed["human_required_count"], 1);
}

#[test]
fn status_reports_human_and_json_views() {
    let tmp = write_fixture_dir();
    let cfg = write_dispatch_config(&tmp);
    let root = tmp.path().join("real");
    init_git_repo(&root);
    write_real_repo_config(&tmp, &root, "test-repo");

    let availability_path = tmp.path().join("avail.json");
    let ledger_path = tmp.path().join("ledger.jsonl");

    // Write a mock availability record
    let avail_state = serde_json::json!({
        "version": 1,
        "records": [
            {
                "backend": "claude",
                "model": "claude-3-5",
                "status": "unavailable",
                "reason": "rate_limited",
                "observed_at": "2026-07-04T12:00:00Z",
                "unavailable_until": "2099-01-01T00:00:00Z",
                "source": "backend_error"
            }
        ]
    });
    fs::write(
        &availability_path,
        serde_json::to_string(&avail_state).unwrap(),
    )
    .unwrap();

    // Write a mock ledger entry
    let ledger_entry: Value = serde_json::from_str(
        r#"{
            "timestamp": "2026-07-04T13:00:00Z",
            "profile": "test-repo",
            "display_name": "Test Repo",
            "repo_id": "test-repo",
            "repo": "owner/test-repo",
            "local_path": "/tmp",
            "provider": "github",
            "backend": "claude",
            "requested_backend": "claude",
            "effective_backend": "claude",
            "requested_model": null,
            "effective_model": "claude-3-5",
            "routing_reason": "explicit",
            "fallback_used": false,
            "confidence_impact": null,
            "human_required": false,
            "mode": "improve",
            "target_summary": null,
            "branch": "gah/test-branch",
            "session_dir": null,
            "duration_seconds": null,
            "backend_exit_code": null,
            "validation_result": null,
            "commit_attempted": false,
            "commit_created": false,
            "push_attempted": false,
            "push_succeeded": false,
            "mr_attempted": false,
            "mr_created": false,
            "mr_url": null,
            "files_changed": null,
            "insertions": null,
            "deletions": null,
            "error_summary": null,
            "failure_class": "backend_error",
            "failure_stage": "agent_run",
            "attempts_started": 3,
            "attempts_completed": 2,
            "routing_diagnostics": {
                "policy_reordered_candidates": true,
                "selected_backend": "claude",
                "selected_model": "claude-3-5",
                "selected_quota_pool": "claude-main",
                "selected_pace_band": "normal",
                "selected_cost_class": "included_quota",
                "selected_over": ["codex/gpt-5.4 (paid $0.2500)"],
                "candidates": [
                    {
                        "backend": "claude",
                        "model": "claude-3-5",
                        "quota_pool": "claude-main",
                        "default_order": 1,
                        "consideration_order": 0,
                        "pace_band": "normal",
                        "cost_class": "included_quota",
                        "skip_reason": null,
                        "unavailable_until": null
                    }
                ],
                "human_summary": "selected claude/claude-3-5"
            },
            "usage": {}
        }"#,
    )
    .unwrap();
    fs::write(
        &ledger_path,
        serde_json::to_string(&ledger_entry).unwrap() + "\n",
    )
    .unwrap();

    let out = bin()
        .current_dir(&root)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .args([
            "status",
            "--json",
            "--profile",
            "test-repo",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["profile"]["display_name"], "Test Repo");
    assert_eq!(parsed["profile"]["provider"], "github");
    assert!(parsed["observations"]["sync"]["status"].is_string());
    assert!(parsed["observations"]["availability"]["status"].is_string());
    assert!(parsed["observations"]["ledger"]["status"].is_string());

    // Verify availability fields, specifically observed_at is populated
    let avail = &parsed["availability"][0];
    assert_eq!(avail["backend"], "claude");
    assert_eq!(avail["model"], "claude-3-5");
    assert_eq!(avail["observed_at"], "2026-07-04T12:00:00Z");

    // Verify ledger fields
    let ledger = &parsed["recent_ledger"];
    assert_eq!(ledger["most_recent_failure_class"], "backend_error");
    assert_eq!(ledger["most_recent_failure_stage"], "agent_run");
    assert_eq!(ledger["attempts_started"], 3);
    assert_eq!(ledger["attempts_completed"], 2);
    assert_eq!(
        ledger["routing_diagnostics"]["selected_quota_pool"],
        "claude-main"
    );

    bin()
        .current_dir(&root)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .args([
            "status",
            "--profile",
            "test-repo",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status for Profile: test-repo"))
        .stdout(predicate::str::contains("Observations: Sync="))
        .stdout(predicate::str::contains("Recent Routing:"))
        .stdout(predicate::str::contains("selected claude/claude-3-5"));
}

#[test]
fn dispatch_agy_multi_instance_isolated_execution() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();

    // Setup git stub, just in case
    make_fake_bin_with_body(&fake_bin, "gh", "#!/bin/sh\nexit 0\n");

    // Let's create fake binaries for agy, agy-main, and agy-second.
    // They will write distinct strings to files under tmp so we can verify they executed.
    let agy_log = tmp.path().join("agy.log");
    let agy_main_log = tmp.path().join("agy_main.log");
    let agy_second_log = tmp.path().join("agy_second.log");

    make_fake_bin_with_body(
        &fake_bin,
        "agy",
        &format!(
            "#!/bin/sh\necho \"agy\" | tee -a \"{}\"\nprintf 'agent edit\n' >> README.md\nexit 0\n",
            agy_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "agy-main",
        &format!(
            "#!/bin/sh\necho \"agy-main\" | tee -a \"{}\"\nprintf 'agent edit\n' >> README.md\nexit 0\n",
            agy_main_log.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "agy-second",
        &format!(
            "#!/bin/sh\necho \"agy-second\" | tee -a \"{}\"\nprintf 'agent edit\n' >> README.md\nexit 0\n",
            agy_second_log.display()
        ),
    );

    // 1. Dispatch with backend agy-main
    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "agy-main",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "test target",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    // Verify agy-main was executed, and ledger recorded "agy-main"
    assert!(agy_main_log.exists());
    assert!(!agy_second_log.exists());
    assert!(!agy_log.exists());

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "agy-main");

    // Clear ledger for the next check
    let _ = fs::remove_file(&ledger_path);

    // Sleep for 1.1s to avoid timestamp/worktree conflict
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // 2. Dispatch with backend agy-second
    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "agy-second",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "test target",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    // Verify agy-second was executed, and ledger recorded "agy-second"
    assert!(agy_second_log.exists());
    assert!(!agy_log.exists());

    let text2 = fs::read_to_string(&ledger_path).unwrap();
    let entry2: Value = serde_json::from_str(text2.lines().next().unwrap()).unwrap();
    assert_eq!(entry2["effective_backend"], "agy-second");

    // Clear ledger again
    let _ = fs::remove_file(&ledger_path);

    // Sleep for 1.1s to avoid timestamp/worktree conflict
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // 3. Dispatch with backend agy (fallback)
    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "agy",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "test target",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    // Verify agy was executed, and ledger recorded "agy"
    assert!(agy_log.exists());

    let text3 = fs::read_to_string(&ledger_path).unwrap();
    let entry3: Value = serde_json::from_str(text3.lines().next().unwrap()).unwrap();
    assert_eq!(entry3["effective_backend"], "agy");
    assert_eq!(
        entry3["attempts"][0]["usage"]["quota_window"],
        "AGY individual quota"
    );
    assert_eq!(entry3["usage"]["quota_window"], "AGY individual quota");
}

/// TICKET-072: `gah ledger reconcile` must append a reconciliation entry
/// when a dispatched work item's MR has since merged, and must never
/// rewrite `ledger.jsonl` itself.
#[test]
fn ledger_reconcile_appends_entry_when_mr_state_changed() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let reconciliation_path = tmp.path().join("reconciliation.jsonl");

    fs::write(
        &ledger_path,
        "{\"timestamp\":\"2026-07-01T00:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"codex\",\"requested_backend\":\"codex\",\"effective_backend\":\"codex\",\"requested_model\":null,\"effective_model\":null,\"routing_reason\":\"explicit\",\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"work_id\":\"TICKET-072\",\"mode\":\"fix\",\"target_summary\":\"x\",\"branch\":\"gah/real-1\",\"session_dir\":null,\"duration_seconds\":1.0,\"backend_exit_code\":0,\"validation_result\":\"passed\",\"commit_attempted\":true,\"commit_created\":true,\"push_attempted\":true,\"push_succeeded\":true,\"mr_attempted\":true,\"mr_created\":true,\"mr_url\":\"https://github.com/owner/real/pull/7\",\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"usage_source\":null}}\n",
    )
    .unwrap();

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"title\":\"[GAH] Fix: TICKET-072\",\"headRefName\":\"gah/real-1\",\"url\":\"https://github.com/owner/real/pull/7\",\"labels\":[],\"number\":7,\"state\":\"MERGED\",\"isDraft\":false,\"mergeStateStatus\":\"MERGED\",\"mergedAt\":\"2026-07-05T00:00:00Z\",\"updatedAt\":\"2026-07-05T00:00:00Z\",\"statusCheckRollup\":[]}]'; exit 0; fi\nexit 0\n",
    );

    let out = bin()
        .args([
            "ledger",
            "reconcile",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_RECONCILIATION_PATH", &reconciliation_path)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let entries = parsed["new_entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["work_id"], "TICKET-072");
    assert_eq!(entries[0]["new_state"], "MERGED");
    assert_eq!(entries[0]["previous_state"], Value::Null);

    // ledger.jsonl itself must be untouched (still exactly the one original line).
    let ledger_text = fs::read_to_string(&ledger_path).unwrap();
    assert_eq!(ledger_text.lines().count(), 1);

    // Running again with the same (still-MERGED) state must not append a
    // second, redundant reconciliation entry.
    bin()
        .args([
            "ledger",
            "reconcile",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_RECONCILIATION_PATH", &reconciliation_path)
        .assert()
        .success()
        .stdout("{\"new_entries\":[],\"issue_closure\":{\"already_closed\":[],\"would_close\":[],\"closed\":[],\"ambiguous\":[],\"unmapped\":[\"unknown\"],\"leave_open\":[],\"observation_failed\":[],\"policy_blocked\":[],\"skipped\":[]}}\n");

    let reconciliation_text = fs::read_to_string(&reconciliation_path).unwrap();
    assert_eq!(reconciliation_text.lines().count(), 1);
}

/// TICKET-079: recurring mode is the default; --once remains an explicit
/// bounded/testing mode.
#[test]
fn loop_without_once_is_accepted_as_recurring_mode() {
    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            "/definitely/does/not/exist.toml",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no config found"));
}

/// TICKET-079: nothing to do (no tickets, no MRs, no availability records)
/// must report NoOp and exit successfully -- not error, not hang.
#[test]
fn loop_once_reports_noop_when_nothing_actionable() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    let events_path = tmp.path().join("events.jsonl");
    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("no_op"));

    let events_text = fs::read_to_string(&events_path).unwrap();
    assert!(events_text.contains("observation_completed"));
    assert!(events_text.contains("action_decided"));
    assert!(events_text.contains("loop_stopped"));
}

#[test]
fn loop_once_prune_skips_full_provider_history_and_retains_fresh_worktree() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let worktree_root = tmp.path().join("worktrees");
    fs::create_dir_all(&worktree_root).unwrap();
    let worktree = worktree_root.join("gah-real-no-progress");
    ProcessCommand::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "gah/real-no-progress",
            worktree.to_str().unwrap(),
            "HEAD",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo 'automatic loop must not query full PR history' >&2; exit 97; fi\nif [ \"$1\" = \"api\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .stdout(predicate::str::contains("no_op"));

    assert!(worktree.exists(), "fresh worktree was automatically pruned");
}

/// TICKET-079: an eligible never-dispatched ticket actually gets dispatched
/// (fix mode) -- the full observe -> decide -> execute -> persist path,
/// not just the decision in isolation.
#[test]
fn loop_once_dispatches_an_eligible_ticket() {
    let tmp = test_tempdir();
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

    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    fs::write(
        repo.join("docs/tickets/TICKET-300-loop-test.md"),
        "# TICKET-300: Loop test ticket\n\nGoal: test loop --once dispatch\n\nRecommended backend: codex\n",
    )
    .unwrap();

    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "validation_commands = [\"true\"]\n",
        "",
    );

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
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"api\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
    );

    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("dispatch_ticket"));

    let ledger_text = fs::read_to_string(&ledger_path).unwrap();
    // Parallel workers: the first line is now a "claim" entry (written
    // before any backend work runs, so a concurrent worker sees this
    // ticket is taken immediately rather than only after the dispatch
    // finishes minutes-to-hours later) -- the real completion entry is
    // the first non-claim line.
    let entry: Value = ledger_text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|e| e["mode"] != "claim")
        .expect("a real completion entry after the claim");
    assert_eq!(entry["work_id"], "TICKET-300");
    assert_eq!(entry["validation_result"], "passed");

    let events_text = fs::read_to_string(&events_path).unwrap();
    assert!(events_text.contains("dispatch_started"));
    assert!(events_text.contains("dispatch_finished"));
}

/// Regression for the parallel-batch-abort bug: a terminal decision
/// (NoOp/HumanRequired/WaitUntil) for ONE slot must not stop OTHER slots in
/// the same `--parallel` batch from being tried. Simulated here via a `gh`
/// stub that fails `pr list` on exactly the second call (a transient sync
/// hiccup) -- the middle of 3 slots hits it and legitimately decides NoOp
/// ("observation incomplete"), while slot 1 (before the hiccup) and slot 3
/// (after it clears) each find a distinct, real, dispatchable ticket. Before
/// the fix, the middle slot's NoOp `break`s the whole batch and TICKET-301
/// (only reachable from slot 3) never gets dispatched.
#[test]
fn parallel_loop_slot_terminal_action_does_not_abort_later_slots() {
    let tmp = test_tempdir();
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

    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    fs::write(
        repo.join("docs/tickets/TICKET-300-loop-test.md"),
        "# TICKET-300: Loop test ticket A\n\nGoal: test loop --parallel dispatch\n\nRecommended backend: codex\n",
    )
    .unwrap();
    fs::write(
        repo.join("docs/tickets/TICKET-301-loop-test.md"),
        "# TICKET-301: Loop test ticket B\n\nGoal: test loop --parallel dispatch\n\nRecommended backend: codex\n",
    )
    .unwrap();

    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "validation_commands = [\"true\"]\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    let pr_list_count_file = tmp.path().join("pr_list_count");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
             "#!/bin/sh\n\
             if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then\n\
             \x20\x20lock_dir='{count_file}.lock'\n\
             \x20\x20while ! mkdir \"$lock_dir\" 2>/dev/null; do sleep 0.01; done\n\
             \x20\x20count=$(( $(cat '{count_file}' 2>/dev/null || echo 0) + 1 ))\n\
             \x20\x20echo \"$count\" > '{count_file}'\n\
             \x20\x20rmdir \"$lock_dir\"\n\
             \x20\x20if [ \"$count\" = \"2\" ]; then echo 'simulated transient sync failure' >&2; exit 1; fi\n\
             \x20\x20echo '[]'; exit 0\n\
             fi\n\
             if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\n\
             if [ \"$1\" = \"api\" ]; then echo '[]'; exit 0; fi\n\
             exit 0\n",
            count_file = pr_list_count_file.display()
        ),
    );

    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
            "--parallel",
            "3",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();

    // Both distinct tickets must have a real (non-claim) completion entry --
    // slot 3's dispatch of TICKET-301 must not have been aborted by slot 2's
    // NoOp verdict on the transient sync hiccup.
    let ledger_text = fs::read_to_string(&ledger_path).unwrap_or_else(|err| {
        let events = fs::read_to_string(&events_path)
            .unwrap_or_else(|events_err| format!("<unreadable: {events_err}>"));
        let pr_list_count =
            fs::read_to_string(&pr_list_count_file).unwrap_or_else(|_| "<missing>".into());
        panic!(
            "parallel loop produced no ledger ({err}); pr-list count={}; events={events}",
            pr_list_count.trim()
        );
    });
    let dispatched_work_ids: std::collections::HashSet<String> = ledger_text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|e| e["mode"] != "claim")
        .filter_map(|e| e["work_id"].as_str().map(str::to_string))
        .collect();
    assert!(
        dispatched_work_ids.contains("TICKET-300"),
        "expected TICKET-300 dispatched, got: {dispatched_work_ids:?}"
    );
    assert!(
        dispatched_work_ids.contains("TICKET-301"),
        "expected TICKET-301 dispatched (slot 3, after a middle slot's NoOp), got: {dispatched_work_ids:?}"
    );
}

/// TICKET-084: `gah events` reads back exactly what `gah loop --once`
/// wrote, and `--profile` filters to just that profile's events.
#[test]
fn events_reads_back_loop_once_output_and_filters_by_profile() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nexit 0\n",
    );

    let events_path = tmp.path().join("events.jsonl");
    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();

    let out = bin()
        .args([
            "events",
            "--config-path",
            cfg.to_str().unwrap(),
            "--profile",
            "real",
            "--json",
        ])
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let events = parsed.as_array().unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e["profile"] == "real"));

    let out_other = bin()
        .args([
            "events",
            "--config-path",
            cfg.to_str().unwrap(),
            "--profile",
            "some-other-profile",
            "--json",
        ])
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success();
    let stdout_other = String::from_utf8_lossy(&out_other.get_output().stdout).to_string();
    let parsed_other: Value = serde_json::from_str(&stdout_other).unwrap();
    assert!(parsed_other.as_array().unwrap().is_empty());
}

/// TICKET-081: an MR that keeps landing on the same decision (ReviewMr,
/// unchanged classification each time) must trip the stuck-loop detector
/// on the Nth `--once` invocation instead of re-dispatching a review
/// forever.
#[test]
fn loop_once_stops_on_stuck_loop_instead_of_repeating_forever() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\ncase \"$4\" in */pulls?*) echo '[{\"title\":\"[GAH] Fix: TICKET-500\",\"body\":\"body\",\"head\":{\"ref\":\"gah/real-1\",\"sha\":null},\"html_url\":\"https://github.com/owner/real/pull/1\",\"labels\":[],\"number\":1,\"state\":\"open\",\"draft\":false,\"updated_at\":\"2026-07-18T17:22:35-05:00\"}]'; exit 0;; */check-runs?*) echo '{\"total_count\":0,\"check_runs\":[]}'; exit 0;; esac\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"number\":1,\"url\":\"https://github.com/owner/real/pull/1\",\"title\":\"[GAH] Fix: TICKET-500\",\"body\":\"body\",\"headRefName\":\"gah/real-1\",\"baseRefName\":\"main\",\"statusCheckRollup\":[]}'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"comment\" ]; then exit 0; fi\nexit 0\n",
    );

    let events_path = tmp.path().join("events.jsonl");
    // Pre-seed 3 prior review_mr decisions for this exact work_id -- as if
    // 3 previous `--once` iterations already tried (and re-tried) the same
    // review with nothing else happening in between.
    let mut seeded = String::new();
    for _ in 0..3 {
        seeded.push_str(
            &serde_json::to_string(&serde_json::json!({
                "timestamp": "2026-07-05T00:00:00Z", "event_type": "action_decided",
                "profile": "real",
                "work_id": "TICKET-500",
                "details": "review_mr: MR needs review",
                "review_contract_version": 1
            }))
            .unwrap(),
        );
        seeded.push('\n');
    }
    fs::write(&events_path, seeded).unwrap();

    bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .env("GAH_EVENTS_PATH", &events_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("human_required"))
        .stdout(predicate::str::contains("stuck-loop"));

    let events_text = fs::read_to_string(&events_path).unwrap();
    // No dispatch was triggered for the 4th, stuck iteration.
    assert!(!events_text.contains("dispatch_started"));
}

// ── TICKET-128: per-profile publishing policy ───────────────────────────────
//
// A restricted profile forbids agent-authored repository prose (PR/MR text,
// generated commit messages, issue/MR comments) while preserving autonomous
// code execution and code review. Each axis is configured independently and
// must NOT be overloaded onto `human_required`.

/// Acceptance: publishing disabled + successful fix => no PR/MR API call is
/// issued and the run stops at a deterministic human handoff.
#[test]
fn publishing_disabled_blocks_pr_creation_and_emits_handoff() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\n[profiles.real.publishing]\nallow_pull_request_creation = false\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
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
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        // Deterministic handoff metadata is produced.
        .stdout(predicate::str::contains(
            "GAH human handoff (publishing policy)",
        ))
        .stdout(predicate::str::contains(
            "PR/MR creation or commit-message generation disabled by publishing policy",
        ));

    // No PR/MR was ever attempted (gh was never asked to `pr create`).
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        !gh_text.contains("pr create"),
        "gh was asked to create a PR: {gh_text}"
    );

    // Ledger reflects the handoff, not a publish attempt.
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["mr_attempted"], false);
    assert_eq!(entry["mr_created"], false);
    assert_eq!(entry["push_attempted"], false);
    assert_eq!(entry["push_succeeded"], false);
    assert_eq!(entry["validation_result"], "passed");
    assert_eq!(entry["human_required"], false);
}

/// Acceptance: commit-message generation disabled => the worktree is left
/// uncommitted for human completion (no commit is made / recorded). This axis
/// is configured independently of PR creation; both are combined into a single
/// deterministic human handoff, but the commit ledger must still record that
/// no commit was attempted.
#[test]
fn commit_message_generation_disabled_leaves_worktree_uncommitted() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\n[profiles.real.publishing]\nallow_pull_request_creation = true\nallow_commit_message_generation = false\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
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
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "GAH human handoff (publishing policy)",
        ))
        .stdout(predicate::str::contains(
            "PR/MR creation or commit-message generation disabled by publishing policy",
        ));

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    // The auto-commit step was skipped entirely (no LLM commit-message call):
    // `commit_attempted` is only set when we actually try to stage/commit.
    assert_eq!(entry["commit_attempted"], false);
    assert_eq!(entry["commit_created"], false);
    // No PR was opened either (the combined gate stops before publish).
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        !gh_text.contains("pr create"),
        "gh was asked to create a PR: {gh_text}"
    );
}

/// Acceptance: contribution still reaches the reviewer when publishing is
/// disabled. The reviewer runs and a deterministic verdict is produced.
#[test]
fn publishing_disabled_still_runs_reviewer() {
    let tmp = test_tempdir();
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
        concat!(
            "[profiles.real.routing]\nreview_backend = \"claude\"\n",
            "[profiles.real.publishing]\nallow_pull_request_creation = false\n",
        ),
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);

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

    // Reviewer actually executed and produced a structured verdict.
    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let report = fs::read_to_string(session.join("review-report.md")).unwrap();
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    assert!(report.contains("Review notes"));
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
    // The prompt was still written for the reviewer (review is not disabled).
    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(prompt.contains("Source: feature/review"));
}

/// Acceptance: APPROVE + CI pass + PR creation disabled => no auto-merge path
/// is entered. With publishing disabled, `MergeMr` must not be selected by the
/// controller.
#[test]
fn approve_with_pr_disabled_skips_auto_merge_in_loop() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        concat!(
            "validation_commands = [\"true\"]\n",
            "[profiles.real.publishing]\nallow_pull_request_creation = false\n",
        ),
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
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
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    // No merge command (gh pr merge / glab mr merge) was issued.
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        !gh_text.contains("merge"),
        "gh was asked to merge: {gh_text}"
    );
    // The snapshot the controller consulted reflected the disabled policy.
    // (We assert indirectly: the run still succeeded and stopped at handoff.)
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["mr_created"], false);
}

/// Acceptance: issue comments disabled => no tracker comment API call is made,
/// even though review still runs and produces a verdict.
#[test]
fn issue_comments_disabled_skips_tracker_comment() {
    let tmp = test_tempdir();
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
        concat!(
            "[profiles.real.routing]\nreview_backend = \"claude\"\n",
            "[profiles.real.publishing]\nallow_issue_comments = false\n",
        ),
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{{\"number\":7}}]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{{\"number\":7,\"url\":\"https://github.com/owner/real/pull/7\",\"title\":\"Draft: [GAH] Fix\",\"body\":\"MR body\",\"headRefName\":\"feature/review\",\"baseRefName\":\"main\"}}'; exit 0; fi\nexit 0\n",
            gh_log.display()
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
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Publishing policy forbids agent-authored issue/MR comments",
        ));

    // No `pr comment` (tracker comment) call was made.
    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        !gh_text.contains("pr comment") && !gh_text.contains("comment"),
        "gh was asked to comment: {gh_text}"
    );
    // Reviewer still produced a verdict locally.
    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
}

/// Acceptance: a pet-project profile with publishing enabled keeps the
/// existing autonomous behavior (PR is actually created). This guards against
/// the default flipping to restrictive.
#[test]
fn pet_project_publishing_enabled_creates_pr() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        concat!(
            "validation_commands = [\"true\"]\n",
            "[profiles.real.publishing]\n",
            "allow_pull_request_creation = true\n",
            "allow_commit_message_generation = true\n",
            "allow_issue_comments = true\n",
        ),
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
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

    let gh_text = fs::read_to_string(&gh_log).unwrap_or_default();
    assert!(
        gh_text.contains("pr create"),
        "gh was NOT asked to create a PR: {gh_text}"
    );
    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["mr_created"], true);
}

/// Acceptance: a restricted profile still produces deterministic human-handoff
/// metadata (branch, changed files, validation status, artifact paths, verdict).
#[test]
fn restricted_profile_emits_deterministic_handoff_metadata() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"true\"]\n[profiles.real.publishing]\nallow_pull_request_creation = false\n",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\\n' >> README.md\nexit 0\n",
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
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "=== GAH human handoff (publishing policy) ===",
        ))
        .stdout(predicate::str::contains("validation_status"))
        .stdout(predicate::str::contains("changed_files"))
        .stdout(predicate::str::contains("branch:"))
        .stdout(predicate::str::contains(
            "PR/MR creation or commit-message generation disabled by publishing policy",
        ));
}
