#![cfg(target_os = "linux")]

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use support::test_tempdir;
use support::ProcessGroupGuard;

fn process_exists(pid: i32) -> bool {
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn run_git(repo: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_until_gone(pid: i32, description: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while process_exists(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_exists(pid),
        "{description} {pid} survived parent death"
    );
}

#[test]
fn recurring_loop_exits_and_releases_ownership_when_launcher_dies() {
    let tmp = test_tempdir();
    let tmp_root = support::test_temp_root();
    let repo = tmp.path().join("repo");
    let other_repo = tmp.path().join("other-repo");
    let remote = tmp.path().join("remote.git");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    fs::create_dir_all(&other_repo).unwrap();
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(repo.join("docs/MANAGER_MEMORY.md"), "# test\n").unwrap();
    fs::write(
        repo.join("docs/tickets/TICKET-001-parent-death.md"),
        "# TICKET-001: Parent death worker\n\nGoal: remain active until the launcher exits.\n\nRecommended backend: codex\n",
    )
    .unwrap();
    Command::new("git")
        .args(["init", "--bare", "-q", remote.to_str().unwrap()])
        .status()
        .unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "test@example.com"]);
    run_git(&repo, &["config", "user.name", "Test"]);
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-q", "-m", "initial"]);
    run_git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    run_git(&repo, &["push", "-q", "-u", "origin", "main"]);
    run_git(&other_repo, &["init", "-q", "-b", "main"]);

    let gh = bin_dir.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\ncase \"$1 $2\" in\n  \"pr list\"|\"api --method\") echo '[]' ;;\n  *) exit 0 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();

    let backend_pid_path = tmp.path().join("backend.pid");
    let codex = bin_dir.join("codex");
    fs::write(
        &codex,
        format!(
            "#!/bin/sh\necho $$ > '{}'\ntrap 'exit 0' TERM INT\nwhile :; do sleep 1; done\n",
            backend_pid_path.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

    let config = tmp.path().join("config.toml");
    fs::write(
        &config,
        format!(
            r#"[defaults]
artifact_root = "{root}/artifacts"
worktree_base = "{root}/worktrees"

[profiles.test]
display_name = "Parent death test"
repo_id = "owner/test"
provider = "github"
repo = "owner/test"
local_path = "{repo}"
artifact_root = "{root}/artifacts/test"
default_target_branch = "main"
validation_commands = ["true"]
codex_path = "{codex}"

[profiles.test.routing]
default_backend = "codex"

[profiles.other]
display_name = "Unaffected profile"
repo_id = "owner/other"
provider = "github"
repo = "owner/other"
local_path = "{other_repo}"
artifact_root = "{root}/artifacts/other"
default_target_branch = "main"
validation_commands = ["true"]
"#,
            root = tmp.path().display(),
            repo = repo.display(),
            codex = codex.display(),
            other_repo = other_repo.display(),
        ),
    )
    .unwrap();

    let pid_path = tmp.path().join("loop.pid");
    let log_path = support::test_temp_root().join(format!(
        "recurring-loop-parent-death-{}.log",
        std::process::id()
    ));
    let gah = env!("CARGO_BIN_EXE_gah");
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap());
    let launcher = r#"
"$GAH_BIN" loop --profile test --config-path "$GAH_CONFIG" >"$GAH_LOG" 2>&1 &
child=$!
echo "$child" >"$GAH_PID"
deadline=3000
while [ ! -s "$GAH_BACKEND_PID" ]; do
  kill -0 "$child" 2>/dev/null || exit 20
  deadline=$((deadline - 1))
  [ "$deadline" -gt 0 ] || exit 21
  sleep 0.01
done
exit 0
"#;
    let state_home = tmp.path().join("state");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");
    let claims_path = tmp.path().join("work-claims.json");
    let mut unaffected = ProcessGroupGuard::new(
        Command::new(gah)
            .args([
                "loop",
                "--profile",
                "other",
                "--config-path",
                config.to_str().unwrap(),
            ])
            .env("PATH", &path)
            .env("XDG_STATE_HOME", &state_home)
            .env("GAH_CLAIM_STATE_PATH", &claims_path)
            .env("GAH_LEDGER_PATH", &ledger_path)
            .env("GAH_EVENTS_PATH", &events_path)
            .env("TMPDIR", &tmp_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    thread::sleep(Duration::from_millis(200));
    assert!(unaffected.try_wait().unwrap().is_none());

    eprintln!("launcher log: {}", log_path.display());
    let status = Command::new("/bin/sh")
        .args(["-c", launcher])
        .env("GAH_BIN", gah)
        .env("GAH_CONFIG", &config)
        .env("GAH_LOG", &log_path)
        .env("GAH_PID", &pid_path)
        .env("GAH_BACKEND_PID", &backend_pid_path)
        .env("PATH", &path)
        .env("XDG_STATE_HOME", &state_home)
        .env("GAH_CLAIM_STATE_PATH", &claims_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .env("TMPDIR", &tmp_root)
        .status()
        .unwrap();
    if !status.success() {
        if let Ok(log_contents) = fs::read_to_string(&log_path) {
            eprintln!("launcher output:\n{log_contents}");
        }
        assert!(status.success(), "launcher failed: {status}");
    }

    let pid = fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    let backend_pid = fs::read_to_string(&backend_pid_path)
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    wait_until_gone(pid, "recurring loop");
    wait_until_gone(backend_pid, "backend descendant");
    assert!(
        unaffected.try_wait().unwrap().is_none(),
        "parent death for one profile terminated an unrelated profile"
    );

    let log = fs::read_to_string(log_path).unwrap();
    assert!(
        log.contains("shutdown requested"),
        "loop did not exit gracefully: {log}"
    );
    let claims = Command::new(gah)
        .args([
            "claims",
            "list",
            "--json",
            "--profile",
            "test",
            "--config-path",
            config.to_str().unwrap(),
        ])
        .env("GAH_CLAIM_STATE_PATH", &claims_path)
        .env("TMPDIR", &tmp_root)
        .output()
        .unwrap();
    assert!(claims.status.success());
    let claims_json: serde_json::Value = serde_json::from_slice(&claims.stdout).unwrap();
    assert_eq!(claims_json.as_array().map(Vec::len), Some(0));
    let once = Command::new(gah)
        .args([
            "loop",
            "--profile",
            "test",
            "--config-path",
            config.to_str().unwrap(),
            "--once",
        ])
        .env("PATH", path)
        .env("XDG_STATE_HOME", state_home)
        .env("GAH_LEDGER_PATH", ledger_path)
        .env("GAH_EVENTS_PATH", events_path)
        .env("GAH_CLAIM_STATE_PATH", claims_path)
        .env("TMPDIR", &tmp_root)
        .output()
        .unwrap();
    assert!(
        once.status.success(),
        "profile ownership was not released: {}",
        String::from_utf8_lossy(&once.stderr)
    );

    unsafe {
        libc::kill(unaffected.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while unaffected.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if unaffected.try_wait().unwrap().is_none() {
        unaffected.kill().unwrap();
        unaffected.wait().unwrap();
    }
}
