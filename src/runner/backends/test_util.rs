//! Shared test fixtures for the backend adapter tests. These were previously
//! defined inline in the old `runner.rs` test module; the adapters that own
//! them moved into `runner/backends/*`, so the fixtures now live in one place
//! that every adapter test module can reuse.

use crate::config::*;
use crate::runner::LlmConfig;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub(crate) fn make_fake_bin(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

/// A fake backend binary that records every argv it received (one per
/// line) to `<record_dir>/argv.txt`, records its full environment to
/// `<record_dir>/env.txt`, writes a marker to stdout and stderr, and
/// exits with `exit_code`. This is the seam the spec asks for: real
/// backend CLIs never run in tests, but the *contract* GAH relies on
/// (which flags, which env vars, what happens on nonzero exit) does.
pub(crate) fn make_recording_bin(dir: &Path, name: &str, record_dir: &Path, exit_code: i32) {
    // Use absolute paths for the environment dump so the fake PATH does
    // not interfere with the recorder itself.
    let body = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{argv}'\n/usr/bin/env | /usr/bin/sort > '{env}'\necho 'stdout-marker-{name}'\necho 'stderr-marker-{name}' >&2\nexit {exit_code}\n",
            argv = record_dir.join("argv.txt").display(),
            env = record_dir.join("env.txt").display(),
            name = name,
            exit_code = exit_code,
        );
    make_fake_bin(dir, name, &body);
}

pub(crate) fn recorded_argv(record_dir: &Path) -> Vec<String> {
    fs::read_to_string(record_dir.join("argv.txt"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

pub(crate) fn recorded_env(record_dir: &Path) -> String {
    fs::read_to_string(record_dir.join("env.txt")).unwrap()
}

pub(crate) struct Fixture {
    pub(crate) _tmp: TempDir,
    pub(crate) bin_dir: PathBuf,
    pub(crate) record_dir: PathBuf,
    pub(crate) session_dir: PathBuf,
    pub(crate) worktree: PathBuf,
}

pub(crate) fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    let record_dir = tmp.path().join("record");
    let session_dir = tmp.path().join("session");
    let worktree = tmp.path().join("worktree");
    for d in [&bin_dir, &record_dir, &session_dir, &worktree] {
        fs::create_dir_all(d).unwrap();
    }
    Fixture {
        _tmp: tmp,
        bin_dir,
        record_dir,
        session_dir,
        worktree,
    }
}

pub(crate) fn initialize_git_worktree(worktree: &Path) {
    let run_git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(worktree)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    };
    run_git(&["init", "-q"]);
    run_git(&["config", "user.email", "gah-test@example.invalid"]);
    run_git(&["config", "user.name", "GAH test"]);
    fs::write(worktree.join("progress.txt"), "initial\n").unwrap();
    run_git(&["add", "progress.txt"]);
    run_git(&["commit", "-qm", "initial fixture"]);
}

pub(crate) fn test_llm() -> LlmConfig {
    LlmConfig {
        base_url: "http://llm.test".into(),
        api_key: "test-key".into(),
        model: "test-model".into(),
    }
}

pub(crate) fn test_profile() -> Profile {
    Profile {
        delivery_mode: crate::config::DeliveryMode::default(),
        manager_wake_autonomy: crate::config::WakeAutonomy::default(),
        prune_older_than_days: None,
        display_name: "Repo".into(),
        repo_id: "repo".into(),
        provider: "github".into(),
        repo: "owner/repo".into(),
        local_path: "/tmp/repo".into(),
        artifact_root: "/tmp/artifacts".into(),
        default_target_branch: "main".into(),
        provider_api_base: None,
        provider_project_id: None,
        oh_profile: None,
        openhands_args: vec![],
        codex_args: vec![],
        codex_path: None,
        claude_args: vec![],
        claude_path: None,
        agy_path: None,
        vibe_args: vec![],
        vibe_path: None,
        opencode_args: vec![],
        opencode_path: None,
        agy_second_home: None,
        agy_print_timeout_seconds: std::collections::HashMap::new(),
        agy_idle_timeout_seconds: None,
        opencode_idle_timeout_seconds: None,
        opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
        max_concurrent_per_model: std::collections::HashMap::new(),
        openhands_idle_timeout_seconds: None,
        vibe_idle_timeout_seconds: None,
        codex_idle_timeout_seconds: None,
        claude_idle_timeout_seconds: None,
        max_parallel_workers: None,
        max_open_managed_mrs: None,
        policy_path: None,
        env_file: None,
        env_file_prod: None,
        validation_commands: vec![],
        auto_fix_commands: vec![],
        test_file_patterns: vec![],
        known_baseline_failure_markers: vec![],
        model_improve: None,
        model_pm: None,
        model_review: None,
        review_timeout_seconds: None,
        review_hard_timeout_seconds: None,
        validation_timeout_seconds: None,
        notify_command: None,
        routing: RoutingPolicy::default(),
        pacing: Default::default(),
        publishing: Default::default(),
    }
}
