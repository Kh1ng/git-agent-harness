mod support;

use std::fs;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use support::test_tempdir;

fn spawn_bin() -> ProcessCommand {
    let mut cmd = ProcessCommand::new(
        std::env::var("CARGO_BIN_EXE_gah").unwrap_or_else(|_| "target/debug/gah".into()),
    );
    cmd.env(
        "XDG_STATE_HOME",
        std::env::temp_dir().join(format!("gah-controller-refill-{}", std::process::id())),
    );
    cmd.env(
        "GAH_AVAILABILITY_PATH",
        "/nonexistent-availability-path.json",
    );
    cmd.env(
        "GAH_VALIDATION_CHECK_PATH",
        std::env::temp_dir().join(format!(
            "gah-controller-refill-validation-{}.json",
            std::process::id(),
        )),
    );
    cmd.env("TMPDIR", support::test_temp_root());
    cmd
}

fn make_fake_bin_with_body(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
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

fn configure_git_url_instead_of(home: &std::path::Path, from: &str, to: &str) {
    fs::write(
        home.join(".gitconfig"),
        format!("[url \"{}\"]\n\tinsteadOf = {}\n", to, from),
    )
    .unwrap();
}

fn write_real_repo_config_with_extra(
    tmp: &tempfile::TempDir,
    repo: &std::path::Path,
    provider: &str,
    extra_profile: &str,
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
        ),
    )
    .unwrap();
    cfg
}

fn setup_fix_dispatch_repo(
    tmp: &tempfile::TempDir,
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

    let cfg = write_real_repo_config_with_extra(
        tmp,
        &repo,
        "github",
        &format!(
            "{}\n[profiles.real.routing]\nimprove_backend = \"codex\"\n",
            extra_profile
        ),
    );
    (repo, home, cfg)
}

#[test]
fn parallel_loop_refills_immediately_after_a_fast_completion() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");

    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    for (ticket_id, title) in [
        ("401", "Slow slot"),
        ("402", "Fast slot"),
        ("403", "Refill slot"),
    ] {
        fs::write(
            repo.join(format!("docs/tickets/TICKET-{ticket_id}-refill.md")),
            format!(
                "# TICKET-{ticket_id}: {title}\n\nGoal: exercise parallel refill scheduling.\nRecommended backend: codex\n"
            ),
        )
        .unwrap();
    }

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let slow_release = tmp.path().join("slow-release");
    let call_count_file = tmp.path().join("codex-call-count");
    let active_count_file = tmp.path().join("codex-active-count");
    let active_lock_dir = tmp.path().join("codex-active-count.lock");
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\n\
             call_count_file='{call_count_file}'\n\
             active_count_file='{active_count_file}'\n\
             active_lock_dir='{active_lock_dir}'\n\
             slow_release='{slow_release}'\n\
             acquire_active_lock() {{ while ! mkdir \"$active_lock_dir\" 2>/dev/null; do sleep 0.01; done; }}\n\
             release_active_lock() {{ rmdir \"$active_lock_dir\"; }}\n\
             inc_active() {{\n\
             \x20\x20acquire_active_lock\n\
             \x20\x20count=$( [ -f \"$active_count_file\" ] && cat \"$active_count_file\" || echo 0 )\n\
             \x20\x20count=$((count + 1))\n\
             \x20\x20echo \"$count\" > \"$active_count_file\"\n\
             \x20\x20release_active_lock\n\
             \x20\x20if [ \"$count\" -gt 2 ]; then echo 'active slot cap exceeded' >&2; exit 97; fi\n\
             }}\n\
             dec_active() {{\n\
             \x20\x20acquire_active_lock\n\
             \x20\x20count=$( [ -f \"$active_count_file\" ] && cat \"$active_count_file\" || echo 0 )\n\
             \x20\x20if [ \"$count\" -gt 0 ]; then count=$((count - 1)); fi\n\
             \x20\x20echo \"$count\" > \"$active_count_file\"\n\
             \x20\x20release_active_lock\n\
             }}\n\
             trap dec_active EXIT INT TERM\n\
             n=$( [ -f \"$call_count_file\" ] && cat \"$call_count_file\" || echo 0 )\n\
             n=$((n + 1))\n\
             echo \"$n\" > \"$call_count_file\"\n\
             inc_active\n\
             printf 'agent edit %s\\n' \"$n\" > \"refill-$n.txt\"\n\
             case \"$n\" in\n\
             \x20\x201) while [ ! -f \"$slow_release\" ]; do sleep 0.05; done ;;\n\
             \x20\x202) : ;;\n\
             \x20\x203) : ;;\n\
             \x20\x20*) : ;;\n\
             esac\n\
             printf 'codex call %s\\n' \"$n\"\n\
             exit 0\n",
            call_count_file = call_count_file.display(),
            active_count_file = active_count_file.display(),
            active_lock_dir = active_lock_dir.display(),
            slow_release = slow_release.display(),
        ),
    );

    let ledger_path = tmp.path().join("ledger.jsonl");
    let events_path = tmp.path().join("events.jsonl");
    let mut child = spawn_bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
            "--parallel",
            "2",
        ])
        .env(
            "PATH",
            format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("GAH_EVENTS_PATH", &events_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while fs::read_to_string(&call_count_file)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
        < 3
    {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("controller exited before the third slot started: {status:?}");
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for the third slot to start");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !slow_release.exists(),
        "third action should start before the slow worker is released"
    );
    fs::write(&slow_release, "release\n").unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn parallel_worker_error_stops_refill_after_running_sibling_finishes() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    for (ticket_id, title) in [
        ("411", "Slow sibling"),
        ("412", "Terminal failure"),
        ("413", "Must not refill"),
    ] {
        fs::write(
            repo.join(format!("docs/tickets/TICKET-{ticket_id}-bounded.md")),
            format!(
                "# TICKET-{ticket_id}: {title}\n\nGoal: exercise bounded parallel failure handling.\nRecommended backend: codex\n"
            ),
        )
        .unwrap();
    }

    let fake_bin = tmp.path().join("bin");
    let calls = tmp.path().join("calls");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; exit 0; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; exit 0; fi\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nlock='{calls}.lock'\nwhile ! mkdir \"$lock\" 2>/dev/null; do sleep 0.01; done\nn=$(cat '{calls}.count' 2>/dev/null || echo 0)\nn=$((n + 1))\necho \"$n\" > '{calls}.count'\nif [ \"$n\" -eq 2 ]; then pwd > '{calls}.failure-pwd'; fi\nrmdir \"$lock\"\nif [ \"$n\" -eq 1 ]; then printf 'slow-start\\n' >> '{calls}'; sleep 5; printf 'slow edit\\n' > slow.txt; printf 'slow-done\\n' >> '{calls}'; exit 0; fi\nif [ -f '{calls}.failure-pwd' ] && [ \"$PWD\" = \"$(cat '{calls}.failure-pwd')\" ]; then printf 'failed\\n' >> '{calls}'; exit 9; fi\nprintf 'refilled\\n' >> '{calls}'\nprintf 'unexpected\\n' > third.txt\nexit 0\n",
            calls = calls.display(),
        ),
    );

    let output = spawn_bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
            "--parallel",
            "2",
        ])
        .env(
            "PATH",
            format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let calls = fs::read_to_string(calls).unwrap();
    assert!(calls.contains("slow-start"));
    assert!(calls.contains("slow-done"));
    assert!(calls.contains("failed"));
    assert!(!calls.contains("refilled"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("parallel action(s) failed"));
}

#[test]
fn parallel_loop_does_not_refill_after_shutdown() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "validation_commands = [\"true\"]\n");
    fs::create_dir_all(repo.join("docs/tickets")).unwrap();
    for id in 501..=503 {
        fs::write(
            repo.join(format!("docs/tickets/TICKET-{id}-shutdown.md")),
            format!("# TICKET-{id}: Shutdown refill guard\n\nGoal: prove shutdown stops refill.\nRecommended backend: codex\n"),
        )
        .unwrap();
    }

    let fake_bin = tmp.path().join("bin");
    let calls = tmp.path().join("codex-calls");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; fi\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then echo '[]'; fi\nexit 0\n",
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\nprintf 'call\\n' >> '{}'\nwhile :; do sleep 0.05; done\n",
            calls.display()
        ),
    );

    let child = spawn_bin()
        .args([
            "loop",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--once",
            "--parallel",
            "2",
        ])
        .env(
            "PATH",
            format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while fs::read_to_string(&calls)
        .map(|text| text.lines().count())
        .unwrap_or(0)
        < 2
    {
        assert!(
            Instant::now() < deadline,
            "two initial workers did not start"
        );
        thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(calls).unwrap().lines().count(), 2);
}
