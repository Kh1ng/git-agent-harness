#[path = "gah_cli/already_satisfied.rs"]
mod already_satisfied;
#[path = "gah_cli/args.rs"]
mod args;
#[path = "gah_cli/availability.rs"]
mod availability_cli;
#[path = "gah_cli/config.rs"]
mod config;
#[path = "gah_cli/conflict_resolution.rs"]
mod conflict_resolution;
#[path = "gah_cli/controller.rs"]
mod controller;
#[path = "gah_cli/dispatch.rs"]
mod dispatch;
#[path = "gah_cli/doctor.rs"]
mod doctor;
#[path = "gah_cli/gitlab_review.rs"]
mod gitlab_review;
#[path = "gah_cli/init.rs"]
mod init;
#[path = "gah_cli/pm.rs"]
mod pm;
#[path = "gah_cli/profile.rs"]
mod profile;
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

fn make_fake_glab(dir: &std::path::Path, mr_list_json: &str) {
    make_fake_bin_with_body(
        dir,
        "glab",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/merge_requests\" ]; then echo '{}'; exit 0; fi\nexit 0\n",
            mr_list_json.replace('\'', "'\\''"),
        ),
    );
}

// ── TDD: machine-readable state for autonomous manager agents ──────────────
// These define the contract for junior-agent tickets. Remove #[ignore] when
// implementing.

// ── TICKET-128: per-profile publishing policy ───────────────────────────────
//
// A restricted profile forbids agent-authored repository prose (PR/MR text,
// generated commit messages, issue/MR comments) while preserving autonomous
// code execution and code review. Each axis is configured independently and
// must NOT be overloaded onto `human_required`.
