use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// Parse a KEY=VALUE env file, skipping blank lines and comments.
pub fn load_env_file(path: &str) -> Vec<(String, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            ))
        })
        .collect()
}

#[derive(Debug)]
pub struct RunResult {
    pub exit_code: i32,
    pub duration_secs: f64,
    pub log_path: String,
}

pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// Load LLM config from an OpenHands named profile (~/.openhands/profiles/<name>.json).
pub fn load_oh_profile(name: &str) -> Result<LlmConfig> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let path = PathBuf::from(format!("{}/.openhands/profiles/{}.json", home, name));
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "openhands profile '{}' not found at {}",
            name,
            path.display()
        )
    })?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing openhands profile {}", path.display()))?;
    Ok(LlmConfig {
        base_url: v["base_url"].as_str().unwrap_or("").to_string(),
        api_key: v["api_key"].as_str().unwrap_or("").to_string(),
        model: v["model"].as_str().unwrap_or("").to_string(),
    })
}

/// List available OpenHands profiles by name (without .json extension).
pub fn list_oh_profiles() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let dir = PathBuf::from(format!("{}/.openhands/profiles", home));
    let Ok(entries) = fs::read_dir(&dir) else {
        return vec![];
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".json").map(|s| s.to_string())
        })
        .collect();
    names.sort();
    names
}

/// Run OpenHands in headless mode. LLM config is injected via --override-with-envs.
/// extra_args come from profile.openhands_args in config (e.g. plugin/skill flags).
pub fn run_openhands(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    llm: &LlmConfig,
    extra_args: &[String],
    env_vars: &[(String, String)],
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    fs::write(session_dir.join("task.md"), task)?;

    let log_file = fs::File::create(&log_path).context("creating log file")?;
    let log_err = log_file.try_clone()?;

    let start = Instant::now();
    let mut cmd = Command::new("openhands");
    cmd.args([
        "--headless",
        "--json",
        "-t",
        task,
        "--exit-without-confirmation",
        "--always-approve",
        "--override-with-envs",
    ])
    .args(extra_args)
    .env("OPENHANDS_SUPPRESS_BANNER", "1")
    .env("LLM_BASE_URL", &llm.base_url)
    .env("LLM_API_KEY", &llm.api_key)
    .env("LLM_MODEL", &llm.model)
    .current_dir(worktree)
    .stdout(Stdio::from(log_file))
    .stderr(Stdio::from(log_err));
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    let status = cmd
        .status()
        .context("launching openhands; is it installed and on PATH?")?;

    Ok(RunResult {
        exit_code: status.code().unwrap_or(-1),
        duration_secs: start.elapsed().as_secs_f64(),
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

/// Run Codex non-interactively via `codex exec`.
/// extra_args come from profile.codex_args (e.g. `-c model=gpt-4o`).
pub fn run_codex(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    extra_args: &[String],
    env_vars: &[(String, String)],
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    fs::write(session_dir.join("task.md"), task)?;

    let log_file = fs::File::create(&log_path).context("creating log file")?;
    let log_err = log_file.try_clone()?;

    let start = Instant::now();
    let mut cmd = Command::new("codex");
    cmd.args(["exec", task])
        .args(extra_args)
        .current_dir(worktree)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    let status = cmd
        .status()
        .context("launching codex; is it installed and on PATH?")?;

    Ok(RunResult {
        exit_code: status.code().unwrap_or(-1),
        duration_secs: start.elapsed().as_secs_f64(),
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

/// Run Claude CLI non-interactively via `claude -p`.
/// extra_args come from profile.claude_args (e.g. `--allowedTools Edit,Write,Bash`).
pub fn run_claude(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    extra_args: &[String],
    env_vars: &[(String, String)],
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    fs::write(session_dir.join("task.md"), task)?;

    let log_file = fs::File::create(&log_path).context("creating log file")?;
    let log_err = log_file.try_clone()?;

    let start = Instant::now();
    let mut cmd = Command::new("claude");
    cmd.args(["-p", task])
        .args(extra_args)
        .current_dir(worktree)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    let status = cmd
        .status()
        .context("launching claude; is it installed and on PATH?")?;

    Ok(RunResult {
        exit_code: status.code().unwrap_or(-1),
        duration_secs: start.elapsed().as_secs_f64(),
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

pub fn backend_available(name: &str) -> bool {
    let cmd = match name {
        "openhands" | "cloud-coder" | "auto" => "openhands",
        "codex" => "codex",
        "claude" => "claude",
        _ => return false,
    };
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn make_fake_bin(dir: &Path, name: &str, body: &str) {
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
    fn make_recording_bin(dir: &Path, name: &str, record_dir: &Path, exit_code: i32) {
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

    fn recorded_argv(record_dir: &Path) -> Vec<String> {
        fs::read_to_string(record_dir.join("argv.txt"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn recorded_env(record_dir: &Path) -> String {
        fs::read_to_string(record_dir.join("env.txt")).unwrap()
    }

    struct Fixture {
        _tmp: TempDir,
        bin_dir: PathBuf,
        record_dir: PathBuf,
        session_dir: PathBuf,
        worktree: PathBuf,
    }

    fn fixture() -> Fixture {
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

    fn test_llm() -> LlmConfig {
        LlmConfig {
            base_url: "http://llm.test".into(),
            api_key: "test-key".into(),
            model: "test-model".into(),
        }
    }

    // ── run_openhands ────────────────────────────────────────────────────

    #[test]
    fn run_openhands_success_writes_stdout_and_stderr_to_log() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_openhands(
            &f.worktree,
            "my task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("stdout-marker-openhands"),
            "log missing stdout: {log}"
        );
        assert!(
            log.contains("stderr-marker-openhands"),
            "log missing stderr: {log}"
        );
        let task = fs::read_to_string(f.session_dir.join("task.md")).unwrap();
        assert_eq!(task, "my task");
    }

    #[test]
    fn run_openhands_nonzero_exit_preserved() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 3);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result =
            run_openhands(&f.worktree, "task", &f.session_dir, &test_llm(), &[], &envs).unwrap();

        assert_eq!(result.exit_code, 3);
    }

    #[test]
    fn run_openhands_core_argv_and_extra_args_present() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_openhands(
            &f.worktree,
            "the task text",
            &f.session_dir,
            &test_llm(),
            &["--extra-flag".to_string()],
            &envs,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--headless".to_string()));
        assert!(argv.contains(&"-t".to_string()));
        assert!(argv.contains(&"the task text".to_string()));
        assert!(argv.contains(&"--override-with-envs".to_string()));
        assert!(argv.contains(&"--extra-flag".to_string()));
    }

    #[test]
    fn run_openhands_propagates_llm_config_via_env() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let llm = LlmConfig {
            base_url: "http://distinct-base.test".into(),
            api_key: "distinct-api-key".into(),
            model: "distinct-model-name".into(),
        };

        run_openhands(&f.worktree, "task", &f.session_dir, &llm, &[], &envs).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("LLM_BASE_URL=http://distinct-base.test"));
        assert!(env.contains("LLM_API_KEY=distinct-api-key"));
        assert!(env.contains("LLM_MODEL=distinct-model-name"));
    }

    #[test]
    fn run_openhands_propagates_env_file_vars() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "env-file-value".to_string()),
        ];

        run_openhands(&f.worktree, "task", &f.session_dir, &test_llm(), &[], &envs).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=env-file-value"));
    }

    #[test]
    fn run_openhands_missing_binary_produces_useful_error() {
        let f = fixture(); // bin_dir stays empty — no openhands on PATH
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_openhands(&f.worktree, "task", &f.session_dir, &test_llm(), &[], &envs)
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("launching openhands; is it installed"));
    }

    // ── run_codex ────────────────────────────────────────────────────────

    #[test]
    fn run_codex_success_writes_stdout_and_stderr_to_log() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_codex(&f.worktree, "codex task", &f.session_dir, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-codex"));
        assert!(log.contains("stderr-marker-codex"));
    }

    #[test]
    fn run_codex_nonzero_exit_preserved() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 7);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_codex(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 7);
    }

    #[test]
    fn run_codex_core_argv_and_extra_args_present() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_codex(
            &f.worktree,
            "the codex task",
            &f.session_dir,
            &["-c".to_string(), "model=gpt".to_string()],
            &envs,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert!(argv.contains(&"the codex task".to_string()));
        assert!(argv.contains(&"-c".to_string()));
        assert!(argv.contains(&"model=gpt".to_string()));
    }

    #[test]
    fn run_codex_propagates_env_file_vars() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "codex-env-value".to_string()),
        ];

        run_codex(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=codex-env-value"));
    }

    #[test]
    fn run_codex_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_codex(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap_err();

        assert!(err.to_string().contains("launching codex; is it installed"));
    }

    // ── run_claude ───────────────────────────────────────────────────────

    #[test]
    fn run_claude_success_writes_stdout_and_stderr_to_log() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_claude(&f.worktree, "claude task", &f.session_dir, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-claude"));
        assert!(log.contains("stderr-marker-claude"));
    }

    #[test]
    fn run_claude_nonzero_exit_preserved() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 1);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn run_claude_core_argv_and_extra_args_present() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_claude(
            &f.worktree,
            "the claude task",
            &f.session_dir,
            &["--allowedTools".to_string(), "Edit,Bash".to_string()],
            &envs,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"the claude task".to_string()));
        assert!(argv.contains(&"--allowedTools".to_string()));
        assert!(argv.contains(&"Edit,Bash".to_string()));
    }

    #[test]
    fn run_claude_propagates_env_file_vars() {
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "claude-env-value".to_string()),
        ];

        run_claude(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=claude-env-value"));
    }

    #[test]
    fn run_claude_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap_err();

        assert!(err
            .to_string()
            .contains("launching claude; is it installed"));
    }

    // ── backend_available ────────────────────────────────────────────────
    // Not part of the spec's priority list, but it is a one-line pure-ish
    // wrapper around `which` that every routing decision depends on, and it
    // was previously completely untested.

    #[test]
    fn backend_available_false_for_unknown_backend_name() {
        assert!(!backend_available("not-a-real-backend"));
    }
}
