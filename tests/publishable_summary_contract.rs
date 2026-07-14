use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command as ProcessCommand;

fn git(args: &[&str], directory: &Path, home: &Path) {
    let output = ProcessCommand::new("git")
        .args(args)
        .current_dir(directory)
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn structured_final_summary_is_the_only_backend_text_published() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let repo = root.join("repo");
    let home = root.join("home");
    let artifacts = root.join("artifacts");
    let worktrees = root.join("worktrees");
    let bin = root.join("bin");
    let github_root = root.join("github-root");
    let origin = github_root.join("owner/real.git");
    for directory in [&repo, &home, &artifacts, &worktrees, &bin] {
        fs::create_dir_all(directory).unwrap();
    }
    fs::create_dir_all(origin.parent().unwrap()).unwrap();

    git(&["init", "-b", "main"], &repo, &home);
    git(&["config", "user.email", "test@example.com"], &repo, &home);
    git(&["config", "user.name", "Test User"], &repo, &home);
    fs::write(repo.join("README.md"), "initial\n").unwrap();
    git(&["add", "README.md"], &repo, &home);
    git(&["commit", "-m", "initial"], &repo, &home);
    git(&["init", "--bare", origin.to_str().unwrap()], root, &home);
    fs::write(
        home.join(".gitconfig"),
        format!(
            "[url \"file://{}/\"]\n\tinsteadOf = https://github.com/\n",
            github_root.display()
        ),
    )
    .unwrap();
    git(
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/real.git",
        ],
        &repo,
        &home,
    );
    git(&["push", "-u", "origin", "main"], &repo, &home);

    let config = root.join("config.toml");
    fs::write(
        &config,
        format!(
            r#"[defaults]
artifact_root = "{}"
worktree_base = "{}"

[profiles.real]
display_name = "Test Repo"
repo_id = "real"
provider = "github"
repo = "owner/real"
local_path = "{}"
artifact_root = "{}"
default_target_branch = "main"
validation_commands = ["true"]

[profiles.real.routing]
improve_backend = "codex"

[profiles.real.publishing]
allow_pull_request_creation = true
allow_commit_message_generation = true
allow_issue_comments = true
"#,
            artifacts.display(),
            worktrees.display(),
            repo.display(),
            artifacts.display(),
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("codex"),
        r#"#!/bin/sh
printf 'agent edit\n' >> README.md
cat <<'EOF'
{"type":"turn.started"}
{"type":"item.completed","item":{"type":"command_execution","aggregated_output":"SECRET_TOOL_TRANSCRIPT src/noise.rs test output"}}
{"type":"item.completed","item":{"type":"agent_message","text":"Implemented the safe publication boundary."}}
{"type":"turn.completed","usage":{"input_tokens":999}}
EOF
"#,
    );
    let gh_log = root.join("gh.log");
    write_executable(
        &bin.join("gh"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nif [ \"$1\" = pr ] && [ \"$2\" = create ]; then printf 'https://github.com/owner/real/pull/1\\n'; fi\n",
            gh_log.display()
        ),
    );

    let ledger = artifacts.join("ledger.jsonl");
    Command::cargo_bin("gah")
        .unwrap()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            config.to_str().unwrap(),
            "--target",
            "#371 structured summaries",
        ])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "test-token")
        .env("GAH_LEDGER_PATH", &ledger)
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .assert()
        .success();

    let entry: Value = fs::read_to_string(&ledger)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .find(|entry: &Value| entry["mode"] != "claim")
        .unwrap();
    let branch = entry["branch"].as_str().unwrap();
    let commit = ProcessCommand::new("git")
        .args(["log", "-1", "--format=%B", branch])
        .current_dir(&origin)
        .output()
        .unwrap();
    let commit = String::from_utf8_lossy(&commit.stdout);
    let provider_arguments = fs::read_to_string(&gh_log).unwrap();

    for published in [&*commit, provider_arguments.as_str()] {
        assert!(published.contains("Implemented the safe publication boundary."));
        assert!(!published.contains("SECRET_TOOL_TRANSCRIPT"));
        assert!(!published.contains("input_tokens"));
    }
}
