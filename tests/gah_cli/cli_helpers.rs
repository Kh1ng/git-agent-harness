#![allow(dead_code)]

use super::{isolate_gah_command, test_tempdir, FakeBackend, IsolatedCommand};
use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub(crate) fn bin() -> IsolatedCommand<Command> {
    let cmd = Command::cargo_bin("gah").unwrap();
    // CLI integration tests may run under the real systemd loop, which sets
    // XDG_STATE_HOME to the operator's persistent state directory. Never let
    // fake profiles and work claims leak into (or inherit from) that state.
    isolate_gah_command(cmd)
}

pub(crate) fn spawn_bin() -> IsolatedCommand<std::process::Command> {
    let cmd = std::process::Command::new(
        std::env::var("CARGO_BIN_EXE_gah").unwrap_or_else(|_| "target/debug/gah".into()),
    );
    isolate_gah_command(cmd)
}

pub(crate) fn write_fixture_dir() -> TempDir {
    let tmp = test_tempdir();

    let scout_dir = tmp.path().join("scout");
    fs::create_dir_all(&scout_dir).unwrap();

    let scout = include_str!("../fixtures/scout_readme_missing.json");
    fs::write(scout_dir.join("scout.json"), scout).unwrap();

    let gate_dir = tmp.path().join("gate");
    fs::create_dir_all(&gate_dir).unwrap();

    let gate = include_str!("../fixtures/gate_readme_warn_sparse.json")
        .replace("__SCOUT_ARTIFACT__", scout_dir.to_str().unwrap());
    fs::write(gate_dir.join("gate.json"), gate).unwrap();

    let watchlist = include_str!("../fixtures/model_watchlist.json");
    fs::write(tmp.path().join("model_watchlist.json"), watchlist).unwrap();

    tmp
}

pub(crate) fn latest_child_dir(root: &std::path::Path) -> std::path::PathBuf {
    let mut dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.pop().unwrap()
}

pub(crate) fn make_fake_bin(dir: &Path, name: &str) {
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

pub(crate) fn make_fake_bin_with_body(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

pub(crate) fn make_fake_github_review_api(dir: &Path) {
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

pub(crate) fn prepend_path(dir: &Path) -> String {
    let old = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), old)
}

#[cfg(unix)]
pub(crate) fn send_signal(pid: u32, signal: i32) {
    unsafe {
        let _ = libc::kill(pid as i32, signal);
    }
}

pub(crate) fn wait_for_backend_call(
    backend: &FakeBackend,
    child: &mut std::process::Child,
    call: u32,
) {
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

pub(crate) fn init_git_repo(path: &Path) {
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

pub(crate) fn add_origin_and_feature_commit(repo: &Path) {
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

pub(crate) fn checkout_branch(repo: &Path, branch: &str) {
    ProcessCommand::new("git")
        .args(["checkout", branch])
        .current_dir(repo)
        .output()
        .unwrap();
}

pub(crate) fn configure_git_url_instead_of(home: &Path, from: &str, to: &str) {
    fs::write(
        home.join(".gitconfig"),
        format!("[url \"{}\"]\n\tinsteadOf = {}\n", to, from),
    )
    .unwrap();
}

pub(crate) fn write_real_repo_config(
    tmp: &TempDir,
    repo: &Path,
    provider: &str,
) -> std::path::PathBuf {
    write_real_repo_config_with_extra(tmp, repo, provider, "", "")
}

pub(crate) fn write_real_repo_config_with_extra(
    tmp: &TempDir,
    repo: &Path,
    provider: &str,
    extra_profile: &str,
    extra_defaults: &str,
) -> std::path::PathBuf {
    let cfg = tmp.path().join("gah-config-real.toml");
    let extra = match provider {
        "gitlab" => {
            "provider_api_base = \"https://gitlab.example.com/api/v4\"\nprovider_project_id = \"42\"\n"
        }
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

pub(crate) fn write_dispatch_config(tmp: &TempDir) -> std::path::PathBuf {
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

pub(crate) fn write_dispatch_config_with_validation(tmp: &TempDir) -> std::path::PathBuf {
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

pub(crate) fn setup_review_repo_and_gh(
    tmp: &TempDir,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let fake_bin = tmp.path().join("bin");
    std::fs::create_dir_all(&fake_bin).unwrap();
    make_fake_github_review_api(&fake_bin);
    (repo, fake_bin, tmp.path().join("home"))
}

pub(crate) fn setup_fix_dispatch_repo(
    tmp: &TempDir,
    extra_profile: &str,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let github_root = tmp.path().join("github-root");
    let origin = github_root.join("owner/real.git");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    init_git_repo(&repo);
    std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
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

pub(crate) fn branch_exists_on_bare_origin(github_root: &Path, branch: &str) -> bool {
    let origin = github_root.join("owner/real.git");
    let out = ProcessCommand::new("git")
        .args(["branch", "--list", branch])
        .current_dir(&origin)
        .output()
        .unwrap();
    !String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

pub(crate) fn make_fake_glab(dir: &Path, mr_list_json: &str) {
    make_fake_bin_with_body(
        dir,
        "glab",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"projects/42/merge_requests\" ]; then echo '{}'; exit 0; fi\nexit 0\n",
            mr_list_json.replace('\'', "'\\''"),
        ),
    );
}
