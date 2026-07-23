use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let config = temp.path().join("gah.toml");
    fs::write(
        &config,
        format!(
            r#"
[defaults]
artifact_root = "{root}/artifacts"
worktree_base = "{root}/worktrees"

[profiles.test]
display_name = "GitLab test"
repo_id = "test"
provider = "gitlab"
repo = "group/project"
local_path = "{repo}"
artifact_root = "{root}/artifacts/test"
default_target_branch = "main"
provider_api_base = "https://gitlab.example.com/api/v4"
provider_project_id = "42"
"#,
            root = temp.path().display(),
            repo = repo.display(),
        ),
    )
    .unwrap();
    (temp, config, bin)
}

fn command(config: &Path, bin: &Path) -> Command {
    let mut command = Command::cargo_bin("gah").unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    command
        .args([
            "sync",
            "--profile",
            "test",
            "--json",
            "--config-path",
            config.to_str().unwrap(),
        ])
        .env("PATH", path);
    command
}

#[test]
fn missing_list_pipeline_is_resolved_through_host_scoped_glab_api() {
    let (_temp, config, bin) = fixture();
    write_executable(
        &bin.join("glab"),
        r#"#!/bin/sh
echo "$@" >> "${0%/*}/calls.txt"
if [ "$1 $2" = "api projects/42/merge_requests" ]; then
  printf '%s\n' '[{"title":"[GAH] fix #1","source_branch":"gah/test","web_url":"https://gitlab.example.com/group/project/-/merge_requests/7","labels":["gah-ready-for-human"],"iid":7,"state":"opened","draft":false}]'
elif [ "$1 $2" = "api projects/42/merge_requests/7/pipelines" ]; then
  printf '%s\n' '[{"id":99,"status":"success"}]'
else
  echo "unexpected glab invocation: $@" >&2
  exit 1
fi
"#,
    );

    let output = command(&config, &bin)
        .assert()
        .success()
        .get_output()
        .clone();
    let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows[0]["ci_passed"], true);
    let calls = fs::read_to_string(bin.join("calls.txt")).unwrap();
    assert!(calls.contains("projects/42/merge_requests/7/pipelines"));
    assert!(calls.contains("--hostname gitlab.example.com"));
    assert!(!calls.contains("PRIVATE-TOKEN"));
}

#[test]
fn pipeline_api_auth_failure_is_not_silently_reported_as_pending() {
    let (_temp, config, bin) = fixture();
    write_executable(
        &bin.join("glab"),
        r#"#!/bin/sh
if [ "$1 $2" = "api projects/42/merge_requests" ]; then
  printf '%s\n' '[{"title":"[GAH] fix #1","source_branch":"gah/test","iid":7,"state":"opened"}]'
else
  echo '401 Unauthorized' >&2
  exit 1
fi
"#,
    );

    command(&config, &bin)
        .assert()
        .failure()
        .stderr(predicates::str::contains("401 Unauthorized"));
}
