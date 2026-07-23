use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

mod support;

fn bin() -> Command {
    static COMMAND_COUNTER: AtomicU64 = AtomicU64::new(0);
    let invocation_id = COMMAND_COUNTER.fetch_add(1, Ordering::Relaxed);
    let invocation_dir = support::test_temp_root().join(format!(
        "gah-gitlab-publication-{}-{invocation_id}",
        std::process::id()
    ));
    fs::create_dir_all(&invocation_dir).unwrap();
    let mut cmd = Command::cargo_bin("gah").unwrap();
    cmd.env("XDG_STATE_HOME", invocation_dir.join("state"));
    cmd.env("TMPDIR", support::test_temp_root());
    cmd.env(
        "GAH_AVAILABILITY_PATH",
        "/nonexistent-availability-path.json",
    );
    cmd.env(
        "GAH_VALIDATION_CHECK_PATH",
        invocation_dir.join(format!(
            "gah-gitlab-publication-validation-{}-{invocation_id}.json",
            std::process::id()
        )),
    );
    cmd
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

fn write_config(tmp: &TempDir, repo: &std::path::Path) -> std::path::PathBuf {
    let cfg = tmp.path().join("gah-config.toml");
    fs::write(
        &cfg,
        format!(
            r#"
[defaults]
artifact_root = "{root}/artifacts"
worktree_base = "{root}/worktrees"
llm_base_url = "http://localhost:4000"
llm_model_local = "local/test"
llm_model_cloud = "cloud/test"
validation_commands = ["true"]

[profiles.real]
display_name = "Real Repo"
repo_id = "real"
provider = "gitlab"
repo = "owner/real"
local_path = "{repo}"
artifact_root = "{root}/artifacts/real"
default_target_branch = "main"
provider_api_base = "https://gitlab.example.com/api/v4"
provider_project_id = "42"

[profiles.real.routing]
improve_backend = "codex"
"#,
            root = tmp.path().display(),
            repo = repo.display()
        ),
    )
    .unwrap();
    cfg
}

fn make_executable(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn invalid_gitlab_mr_response_fails_publication_closed() {
    let tmp = support::test_tempdir();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    let gitlab_root = tmp.path().join("gitlab-root");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&home).unwrap();
    init_git_repo(&repo);

    let origin = gitlab_root.join("owner/real.git");
    fs::create_dir_all(origin.parent().unwrap()).unwrap();
    ProcessCommand::new("git")
        .args(["init", "--bare", origin.to_str().unwrap()])
        .output()
        .unwrap();
    fs::write(
        home.join(".gitconfig"),
        format!(
            "[url \"file://{}/\"]\n\tinsteadOf = https://oauth2@gitlab.example.com/\n",
            gitlab_root.display()
        ),
    )
    .unwrap();
    ProcessCommand::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://oauth2@gitlab.example.com/owner/real.git",
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

    let cfg = write_config(&tmp, &repo);
    let ledger_path = tmp.path().join("ledger.jsonl");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_executable(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf 'agent edit\n' >> README.md\nexit 0\n",
    );
    make_executable(
        &fake_bin,
        "glab",
        "#!/bin/sh\nprintf '%s\\n' '{\"message\":\"404 Project Not Found\"}'\nexit 0\n",
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
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
        .env("PATH", path)
        .env("HOME", &home)
        .env("GITLAB_PAT", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid merge request payload"));

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["push_attempted"], true);
    assert_eq!(entry["push_succeeded"], true);
    assert_eq!(entry["mr_attempted"], true);
    assert_eq!(entry["mr_created"], false);
    assert_eq!(entry["failure_class"], "environment_error");
    assert_eq!(entry["failure_stage"], "mr_create");
    assert!(entry["mr_url"].is_null());
    assert!(entry["error_summary"]
        .as_str()
        .unwrap()
        .contains("invalid merge request payload"));
}
