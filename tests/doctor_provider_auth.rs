//! Issue #538: `gah doctor` provider-auth check must accept an authenticated
//! provider CLI session (glab/gh host session) for the exact configured host
//! and project, not just an explicit token env var. These integration tests
//! use fake `gh`/`glab` commands and contain no real credentials.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command as ProcessCommand;
use tempfile::TempDir;

fn bin() -> Command {
    static COMMAND_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let invocation_id = COMMAND_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut cmd = Command::cargo_bin("gah").unwrap();
    cmd.env(
        "XDG_STATE_HOME",
        std::env::temp_dir().join(format!(
            "gah-docauth-test-state-{}-{invocation_id}",
            std::process::id()
        )),
    );
    cmd.env(
        "GAH_AVAILABILITY_PATH",
        "/nonexistent-availability-path.json",
    );
    cmd.env(
        "GAH_VALIDATION_CHECK_PATH",
        std::env::temp_dir().join(format!(
            "gah-docauth-test-validation-{}-{invocation_id}.json",
            std::process::id()
        )),
    );
    cmd
}

fn init_git_repo(path: &Path) {
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

fn write_real_repo_config(tmp: &TempDir, repo: &Path, provider: &str) -> std::path::PathBuf {
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

[profiles.real]
display_name          = "Real Repo"
repo_id               = "real"
provider              = "{provider}"
repo                  = "owner/real"
local_path            = "{repo}"
artifact_root         = "{root}/artifacts/real"
default_target_branch = "main"
{extra}
"#,
            root = tmp.path().display(),
            provider = provider,
            repo = repo.display(),
            extra = extra,
        ),
    )
    .unwrap();
    cfg
}

fn make_fake_bin_with_body(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

fn prepend_path(dir: &Path) -> String {
    let old = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), old)
}

/// Runs `gah doctor` for `provider` with a fake provider CLI (`cli`) whose
/// `api` preflight behaves per `cli_body`. No token env var is exported, so the
/// only path to a passing provider-auth check is a successful provider CLI
/// session.
fn run_doctor_provider_auth(
    tmp: &TempDir,
    provider: &str,
    cli: &str,
    cli_body: &str,
) -> assert_cmd::assert::Assert {
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(tmp, &repo, provider);

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(&fake_bin, cli, cli_body);

    let mut cmd = bin();
    cmd.args([
        "doctor",
        "--profile",
        "real",
        "--config-path",
        cfg.to_str().unwrap(),
    ])
    .env("PATH", prepend_path(&fake_bin));
    if provider == "github" {
        cmd.env_remove("GITHUB_TOKEN").env_remove("GH_TOKEN");
    } else {
        cmd.env_remove("GITLAB_PAT").env_remove("GITLAB_PAT2");
    }
    cmd.assert()
}

/// A GitHub profile authenticated only through a `gh` host session (no token
/// env var) passes the provider-auth doctor check.
#[test]
fn doctor_passes_github_via_gh_cli_session() {
    let tmp = tempfile::tempdir().unwrap();
    run_doctor_provider_auth(
        &tmp,
        "github",
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"api\" ]; then exit 0; fi\nexit 0\n",
    )
    .success()
    .stdout(predicate::str::contains("[PASS]").and(predicate::str::contains("provider auth")));
}

/// A GitLab profile authenticated only through a `glab` host session (no PAT
/// env var) passes the provider-auth doctor check.
#[test]
fn doctor_passes_gitlab_via_glab_cli_session() {
    let tmp = tempfile::tempdir().unwrap();
    run_doctor_provider_auth(
        &tmp,
        "gitlab",
        "glab",
        "#!/bin/sh\nif [ \"$1\" = \"api\" ]; then exit 0; fi\nexit 0\n",
    )
    .success()
    .stdout(predicate::str::contains("[PASS]").and(predicate::str::contains("provider auth")));
}

/// A GitLab profile whose `glab` session is for the wrong host fails the
/// provider-auth check with a classified auth-failure reason.
#[test]
fn doctor_gitlab_wrong_host_fails_auth() {
    let tmp = tempfile::tempdir().unwrap();
    run_doctor_provider_auth(
        &tmp,
        "gitlab",
        "glab",
        "#!/bin/sh\nif [ \"$1\" = \"api\" ]; then echo \"You are not logged into the GitLab host 'gitlab.example.com'. Run 'glab auth login'.\" >&2; exit 1; fi\nexit 0\n",
    )
    .failure()
    .stdout(
        predicate::str::contains("[FAIL]")
            .and(predicate::str::contains("provider auth"))
            .and(predicate::str::contains("authenticate")),
    );
}

/// A GitLab profile with an expired token fails the provider-auth check with a
/// classified auth-failure reason.
#[test]
fn doctor_gitlab_expired_token_fails_auth() {
    let tmp = tempfile::tempdir().unwrap();
    run_doctor_provider_auth(
        &tmp,
        "gitlab",
        "glab",
        "#!/bin/sh\nif [ \"$1\" = \"api\" ]; then echo \"GET https://gitlab.example.com/api/v4/projects/42: 401 Unauthorized\" >&2; exit 1; fi\nexit 0\n",
    )
    .failure()
    .stdout(
        predicate::str::contains("[FAIL]")
            .and(predicate::str::contains("provider auth"))
            .and(predicate::str::contains("authenticate")),
    );
}

/// A GitLab profile whose exact project is inaccessible/not found fails the
/// provider-auth check with a distinct project-unavailable reason.
#[test]
fn doctor_gitlab_inaccessible_project_fails_classified() {
    let tmp = tempfile::tempdir().unwrap();
    run_doctor_provider_auth(
        &tmp,
        "gitlab",
        "glab",
        "#!/bin/sh\nif [ \"$1\" = \"api\" ]; then echo \"GET https://gitlab.example.com/api/v4/projects/42: 404 Project Not Found\" >&2; exit 1; fi\nexit 0\n",
    )
    .failure()
    .stdout(
        predicate::str::contains("[FAIL]")
            .and(predicate::str::contains("provider auth"))
            .and(predicate::str::contains("inaccessible or not found")),
    );
}

/// A GitHub profile whose `gh` preflight hits a network error fails the
/// provider-auth check with a classified network reason.
#[test]
fn doctor_github_network_error_fails_classified() {
    let tmp = tempfile::tempdir().unwrap();
    run_doctor_provider_auth(
        &tmp,
        "github",
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"api\" ]; then echo \"error connecting to github.com\" >&2; echo \"check your internet connection or https://www.githubstatus.com\" >&2; exit 1; fi\nexit 0\n",
    )
    .failure()
    .stdout(
        predicate::str::contains("[FAIL]")
            .and(predicate::str::contains("provider auth"))
            .and(predicate::str::contains("network error")),
    );
}

/// A GitLab profile with neither a token nor an authenticated `glab` session
/// fails the provider-auth check closed.
#[test]
fn doctor_gitlab_no_token_no_cli_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "gitlab");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(&fake_bin, "which", "#!/bin/sh\nexit 1\n");

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", fake_bin)
        .env_remove("GITLAB_PAT")
        .env_remove("GITLAB_PAT2")
        .assert()
        .failure()
        .stdout(
            predicate::str::contains("[FAIL]")
                .and(predicate::str::contains("provider auth"))
                .and(predicate::str::contains("not found on PATH")),
        );
}
