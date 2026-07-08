use crate::config::Profile;
use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableResolution {
    Found(PathBuf),
    MissingExplicitPath(PathBuf),
    MissingFromPath(String),
    UnknownBackend(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewProcessOutcome {
    Success,
    ExecutableUnavailable,
    SpawnFailure,
    NonZeroExit(i32),
    SignalTermination(i32),
    Timeout,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub struct ReviewRunResult {
    pub outcome: ReviewProcessOutcome,
    pub duration_secs: f64,
    pub stdout: String,
    pub stderr: String,
}

fn copy_stream_to_file<R: Read + Send + 'static>(
    mut reader: R,
    path: PathBuf,
    progress_tx: Option<mpsc::Sender<()>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let mut buf = [0_u8; 8192];
        while let Ok(read) = reader.read(&mut buf) {
            if read == 0 {
                break;
            }
            if file.write_all(&buf[..read]).is_err() {
                break;
            }
            let _ = file.flush();
            if let Some(tx) = &progress_tx {
                let _ = tx.send(());
            }
        }
    })
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
#[allow(dead_code)]
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
/// extra_args come from profile.codex_args, but stale model flags are
/// stripped so the resolved route controls the launched model.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_codex(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
) -> Result<RunResult> {
    run_codex_with_executable(
        Path::new("codex"),
        worktree,
        task,
        session_dir,
        model,
        extra_args,
        env_vars,
    )
}

pub fn run_codex_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    fs::write(session_dir.join("task.md"), task)?;

    let log_file = fs::File::create(&log_path).context("creating log file")?;
    let log_err = log_file.try_clone()?;

    let start = Instant::now();
    let mut cmd = Command::new(executable);
    cmd.arg("exec")
        .arg(task)
        .args(filtered_codex_args(extra_args))
        .args(codex_model_args(model))
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

fn codex_model_args(model: Option<&str>) -> Vec<String> {
    model
        .map(|model| vec!["-m".to_string(), model.to_string()])
        .unwrap_or_default()
}

fn filtered_codex_args(extra_args: &[String]) -> Vec<String> {
    let mut filtered = Vec::with_capacity(extra_args.len());
    let mut i = 0;
    while i < extra_args.len() {
        let arg = &extra_args[i];
        if matches!(arg.as_str(), "-m" | "--model") {
            i += 2;
            continue;
        }
        if arg.starts_with("-m=") || arg.starts_with("--model=") {
            i += 1;
            continue;
        }
        filtered.push(arg.clone());
        i += 1;
    }
    filtered
}

pub fn extract_model_from_args(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if matches!(arg.as_str(), "-m" | "--model") {
            if i + 1 < args.len() {
                return Some(args[i + 1].clone());
            }
            break;
        }
        if let Some(val) = arg.strip_prefix("-m=") {
            return Some(val.to_string());
        }
        if let Some(val) = arg.strip_prefix("--model=") {
            return Some(val.to_string());
        }
        i += 1;
    }
    None
}

/// Run Claude CLI non-interactively via `claude -p`.
/// extra_args come from profile.claude_args (e.g. `--allowedTools Edit,Write,Bash`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_claude(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    extra_args: &[String],
    env_vars: &[(String, String)],
) -> Result<RunResult> {
    run_claude_with_executable(
        Path::new("claude"),
        worktree,
        task,
        session_dir,
        extra_args,
        env_vars,
    )
}

pub fn run_claude_with_executable(
    executable: &Path,
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
    let mut cmd = Command::new(executable);
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

/// Run Mistral's Vibe CLI non-interactively via `vibe -p`.
/// Worker/fix backend only -- not wired into review (see runner::run_review_backend).
/// extra_args come from profile.vibe_args (e.g. `--max-turns 40 --max-price 2`).
/// No --model flag exists on this CLI; model selection is config/env-var
/// driven on vibe's own side (VIBE_ACTIVE_MODEL / ~/.vibe/config.toml),
/// not a per-invocation argument GAH can pass.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_vibe(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    extra_args: &[String],
    env_vars: &[(String, String)],
) -> Result<RunResult> {
    run_vibe_with_executable(
        Path::new("vibe"),
        worktree,
        task,
        session_dir,
        extra_args,
        env_vars,
    )
}

pub fn run_vibe_with_executable(
    executable: &Path,
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
    let mut cmd = Command::new(executable);
    // --trust: automation-only, not persisted to trusted_folders.toml --
    // skips the interactive trust prompt without touching global config.
    // --auto-approve: same automation need as agy's --dangerously-skip-permissions.
    cmd.args(["-p", task, "--trust", "--auto-approve"])
        .args(extra_args)
        .current_dir(worktree)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    let status = cmd
        .status()
        .context("launching vibe; is it installed and on PATH?")?;

    Ok(RunResult {
        exit_code: status.code().unwrap_or(-1),
        duration_secs: start.elapsed().as_secs_f64(),
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

/// Run Antigravity CLI non-interactively via `agy --print`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_agy(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    llm: &LlmConfig,
    env_vars: &[(String, String)],
    executable_name: &str,
) -> Result<RunResult> {
    run_agy_with_executable(
        Path::new(executable_name),
        worktree,
        task,
        session_dir,
        llm,
        env_vars,
        None,
        120,
    )
}

/// AGY sometimes exits 0 with empty stdout on a provider-side failure
/// (quota exhaustion, expired auth) instead of a non-zero exit -- shared
/// by the worker path (run_agy_with_executable) and the review path
/// (run_review_backend), both of which treat that as a failure needing a
/// real diagnosis, not a silently-empty "success".
fn agy_empty_output_diagnosis(env_vars: &[(String, String)], executable: &Path) -> String {
    let agy_home = env_vars
        .iter()
        .find(|(k, _)| k == "HOME")
        .map(|(_, v)| v.as_str())
        .map(|h| h.to_string())
        .or_else(|| std::env::var("HOME").ok());
    let Some(home) = agy_home else {
        return "AGY produced no output (exit=0) and HOME is unset — cannot inspect cli.log."
            .to_string();
    };
    let agy_log = PathBuf::from(home).join(".gemini/antigravity-cli/cli.log");
    let Ok(contents) = fs::read_to_string(&agy_log) else {
        return format!(
            "AGY produced no output (exit=0).  Check {} for details.",
            agy_log.display(),
        );
    };
    if contents.contains("RESOURCE_EXHAUSTED") || contents.contains("429") {
        format!(
            "AGY quota exhausted (exit=0 empty output).  See {}.  Resets ~{}.",
            agy_log.display(),
            extract_reset_time(&contents).unwrap_or_else(|| "unknown".into()),
        )
    } else if contents.contains("not logged into Antigravity") || contents.contains("not logged in")
    {
        format!(
            "AGY not authenticated.  Run `{}` interactively to log in.",
            executable.display(),
        )
    } else {
        format!(
            "AGY produced no output (exit=0).  Check {} for details.",
            agy_log.display(),
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_agy_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    llm: &LlmConfig,
    env_vars: &[(String, String)],
    print_timeout_seconds: Option<u64>,
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    fs::write(session_dir.join("task.md"), task)?;

    let start = Instant::now();
    let mut cmd = Command::new(executable);
    cmd.arg("--print");
    cmd.arg(task);
    cmd.args(["--model", llm.model.as_str()]);
    if let Some(secs) = print_timeout_seconds {
        cmd.args(["--print-timeout", &format!("{secs}s")]);
    }
    cmd.arg("--dangerously-skip-permissions")
        .current_dir(worktree)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().context(format!(
        "launching {}; is it installed and on PATH?",
        executable.display()
    ))?;
    let (progress_tx, progress_rx) = mpsc::channel();
    let stdout_thread = child
        .stdout
        .take()
        .map(|stdout| copy_stream_to_file(stdout, log_path.clone(), Some(progress_tx.clone())));
    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| copy_stream_to_file(stderr, log_path.clone(), Some(progress_tx)));

    // GAH-side supervision: kill only when the log has genuinely gone quiet
    // for idle_timeout_seconds, not on a flat wall-clock budget. A model
    // that's slow but still producing output (still working) is never
    // killed for being slow; --print-timeout above stays as an outer
    // safety backstop for a truly hung process.
    let idle_timeout = Duration::from_secs(idle_timeout_seconds);
    let startup_grace = idle_timeout + idle_timeout;
    let poll_interval = Duration::from_millis(500);
    let mut last_seen_len = fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
    let mut last_progress_at = Instant::now();
    let mut saw_progress = false;
    let mut killed_for_idle = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                while progress_rx.try_recv().is_ok() {
                    last_seen_len = fs::metadata(&log_path)
                        .map(|m| m.len())
                        .unwrap_or(last_seen_len);
                    last_progress_at = Instant::now();
                    saw_progress = true;
                }
                let current_len = fs::metadata(&log_path)
                    .map(|m| m.len())
                    .unwrap_or(last_seen_len);
                if current_len != last_seen_len {
                    last_seen_len = current_len;
                    last_progress_at = Instant::now();
                    saw_progress = true;
                }
                let stalled = if saw_progress {
                    last_progress_at.elapsed() >= idle_timeout
                } else {
                    start.elapsed() >= startup_grace
                };
                if stalled {
                    let _ = child.kill();
                    let _ = child.wait();
                    killed_for_idle = true;
                    break -1;
                }
                thread::sleep(poll_interval);
            }
            Err(_) => break -1,
        }
    };
    let duration = start.elapsed();
    if let Some(handle) = stdout_thread {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }

    if killed_for_idle {
        if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&log_path) {
            use std::io::Write;
            let _ = writeln!(
                file,
                "GAH: killed after {idle_timeout_seconds}s with no new output (stalled, not just slow)."
            );
        }
        return Ok(RunResult {
            exit_code: -1,
            duration_secs: duration.as_secs_f64(),
            log_path: log_path.to_string_lossy().into_owned(),
        });
    }

    // Read captured stdout to detect silent failures.
    let output = fs::read_to_string(&log_path).unwrap_or_default();
    let trimmed = output.trim();

    // AGY sometimes exits 0 with empty output when quota is exhausted or
    // auth has expired.  Treat empty output at exit 0 as a failure and
    // try to classify the real cause from AGY's own log.
    if trimmed.is_empty() && exit_code == 0 {
        let err_msg = agy_empty_output_diagnosis(env_vars, executable);

        if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&log_path) {
            use std::io::Write;
            let _ = writeln!(file, "{}", err_msg);
        }

        return Ok(RunResult {
            exit_code: -1,
            duration_secs: duration.as_secs_f64(),
            log_path: log_path.to_string_lossy().into_owned(),
        });
    }

    Ok(RunResult {
        exit_code,
        duration_secs: duration.as_secs_f64(),
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

/// Crude extraction of a reset timestamp from an AGY cli.log line such as
/// "Resets in 16m44s."  Returns `Some(..)` or `None` if no pattern found.
fn extract_reset_time(log: &str) -> Option<String> {
    for line in log.lines().rev() {
        if let Some(pos) = line.find("Resets in ") {
            let rest = &line[pos + 10..];
            if let Some(end) = rest.find(['.', ':']) {
                let reset = rest[..end].trim();
                if !reset.is_empty() {
                    return Some(reset.to_string());
                }
            }
            let end = rest.trim_end_matches('.');
            if !end.is_empty() {
                return Some(end.to_string());
            }
        }
    }
    None
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn backend_available(name: &str) -> bool {
    backend_command_name(name)
        .and_then(resolve_executable_on_path)
        .is_some()
}

pub fn backend_available_for_profile(profile: &Profile, name: &str) -> bool {
    matches!(
        resolve_backend_executable(profile, name),
        ExecutableResolution::Found(_)
    )
}

pub fn require_backend_executable(profile: &Profile, backend: &str) -> Result<PathBuf> {
    match resolve_backend_executable(profile, backend) {
        ExecutableResolution::Found(path) => Ok(path),
        ExecutableResolution::MissingExplicitPath(path) => {
            anyhow::bail!("configured executable '{}' does not exist", path.display())
        }
        ExecutableResolution::MissingFromPath(cmd) => {
            anyhow::bail!("required binary '{}' not found on PATH", cmd)
        }
        ExecutableResolution::UnknownBackend(backend) => {
            anyhow::bail!("unknown backend '{}'", backend)
        }
    }
}

pub fn resolve_backend_executable(profile: &Profile, backend: &str) -> ExecutableResolution {
    let Some(command) = backend_command_name(backend) else {
        return ExecutableResolution::UnknownBackend(backend.to_string());
    };
    if let Some(explicit) = profile.configured_backend_path(backend) {
        let path = PathBuf::from(explicit);
        return if is_executable_path(&path) {
            ExecutableResolution::Found(path)
        } else {
            ExecutableResolution::MissingExplicitPath(path)
        };
    }
    match resolve_executable_on_path(command) {
        Some(path) => ExecutableResolution::Found(path),
        None => ExecutableResolution::MissingFromPath(command.to_string()),
    }
}

pub fn run_review_backend(
    profile: &Profile,
    backend: &str,
    worktree: &Path,
    prompt: &str,
    session_dir: &Path,
    effective_model: Option<&str>,
    env_vars: &[(String, String)],
) -> ReviewRunResult {
    let start = Instant::now();
    let stdout_path = session_dir.join("review-stdout.log");
    let stderr_path = session_dir.join("review-stderr.log");
    let _ = fs::write(session_dir.join("task.md"), prompt);

    let executable = match resolve_backend_executable(profile, backend) {
        ExecutableResolution::Found(path) => path,
        ExecutableResolution::MissingExplicitPath(_) | ExecutableResolution::MissingFromPath(_) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::ExecutableUnavailable,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: String::new(),
            };
        }
        ExecutableResolution::UnknownBackend(_) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: format!("unsupported review backend: {backend}"),
            };
        }
    };

    if let Err(err) = fs::File::create(&stdout_path) {
        return ReviewRunResult {
            outcome: ReviewProcessOutcome::SpawnFailure,
            duration_secs: start.elapsed().as_secs_f64(),
            stdout: String::new(),
            stderr: format!("creating {}: {err}", stdout_path.display()),
        };
    }
    if let Err(err) = fs::File::create(&stderr_path) {
        return ReviewRunResult {
            outcome: ReviewProcessOutcome::SpawnFailure,
            duration_secs: start.elapsed().as_secs_f64(),
            stdout: String::new(),
            stderr: format!("creating {}: {err}", stderr_path.display()),
        };
    }

    let mut cmd = Command::new(&executable);
    match backend {
        "claude" => {
            cmd.args(["-p", prompt]).args(&profile.claude_args);
        }
        "codex" => {
            cmd.arg("exec")
                .arg(prompt)
                .args(filtered_codex_args(&profile.codex_args))
                .args(codex_model_args(effective_model));
        }
        "agy" | "agy-main" | "agy-second" => {
            cmd.arg("--print").arg(prompt);
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
            cmd.arg("--dangerously-skip-permissions");
        }
        _ => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: format!("unsupported review backend: {backend}"),
            };
        }
    }
    cmd.current_dir(worktree)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return ReviewRunResult {
                outcome: ReviewProcessOutcome::SpawnFailure,
                duration_secs: start.elapsed().as_secs_f64(),
                stdout: String::new(),
                stderr: err.to_string(),
            };
        }
    };
    let stdout_thread = child
        .stdout
        .take()
        .map(|stdout| copy_stream_to_file(stdout, stdout_path.clone(), None));
    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| copy_stream_to_file(stderr, stderr_path.clone(), None));

    let timeout = Duration::from_secs(profile.review_timeout_seconds());
    let poll_interval = Duration::from_millis(25);
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    break ReviewProcessOutcome::Success;
                }
                if let Some(code) = status.code() {
                    break ReviewProcessOutcome::NonZeroExit(code);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(signal) = status.signal() {
                        break ReviewProcessOutcome::SignalTermination(signal);
                    }
                }
                break ReviewProcessOutcome::SpawnFailure;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break ReviewProcessOutcome::Timeout;
                }
                thread::sleep(poll_interval);
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let mut stderr = read_text_file(&stderr_path);
                if !stderr.is_empty() {
                    stderr.push('\n');
                }
                stderr.push_str(&err.to_string());
                return ReviewRunResult {
                    outcome: ReviewProcessOutcome::SpawnFailure,
                    duration_secs: start.elapsed().as_secs_f64(),
                    stdout: read_text_file(&stdout_path),
                    stderr,
                };
            }
        }
    };
    if let Some(handle) = stdout_thread {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }

    let mut stdout = read_text_file(&stdout_path);
    // AGY sometimes exits 0 with empty stdout on a provider-side failure
    // (quota exhaustion, expired auth) -- the same silent-success failure
    // mode already handled for the worker path in run_agy_with_executable.
    // Left as Success here, parse_review_verdict would just fail with an
    // opaque "reviewer did not return verdict JSON" and never give the
    // caller (dispatch::review) a chance to recognize the real cause and
    // reroute to the next review_candidates entry -- so diagnose it the
    // same way the worker path does, and put the diagnosis in stdout
    // where mark_backend_unavailable_from_output can actually see it.
    let outcome = if matches!(backend, "agy" | "agy-main" | "agy-second")
        && matches!(outcome, ReviewProcessOutcome::Success)
        && stdout.trim().is_empty()
    {
        stdout = agy_empty_output_diagnosis(env_vars, &executable);
        ReviewProcessOutcome::NonZeroExit(-1)
    } else {
        outcome
    };

    ReviewRunResult {
        outcome,
        duration_secs: start.elapsed().as_secs_f64(),
        stdout,
        stderr: read_text_file(&stderr_path),
    }
}

fn backend_command_name(name: &str) -> Option<&'static str> {
    match name {
        "openhands" | "cloud-coder" | "auto" => Some("openhands"),
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        "agy" => Some("agy"),
        "agy-main" => Some("agy-main"),
        "agy-second" => Some("agy-second"),
        "vibe" => Some("vibe"),
        _ => None,
    }
}

fn resolve_executable_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_path(candidate))
}

fn is_executable_path(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn read_text_file(path: &Path) -> String {
    let mut buf = Vec::new();
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Profile, RoutingPolicy};
    use crate::test_support::PathGuard;
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

    fn test_profile() -> Profile {
        Profile {
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
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
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
            notify_command: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
        }
    }

    // ── run_openhands ────────────────────────────────────────────────────

    #[test]
    fn run_openhands_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
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
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 3);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result =
            run_openhands(&f.worktree, "task", &f.session_dir, &test_llm(), &[], &envs).unwrap();

        assert_eq!(result.exit_code, 3);
    }

    #[test]
    fn run_openhands_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
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
        let _exec_guard = crate::test_support::ExecGuard::new();
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
        let _exec_guard = crate::test_support::ExecGuard::new();
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
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result =
            run_codex(&f.worktree, "codex task", &f.session_dir, None, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-codex"));
        assert!(log.contains("stderr-marker-codex"));
    }

    #[test]
    fn run_codex_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 7);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 7);
    }

    #[test]
    fn run_codex_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_codex(
            &f.worktree,
            "the codex task",
            &f.session_dir,
            None,
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
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "codex-env-value".to_string()),
        ];

        run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=codex-env-value"));
    }

    #[test]
    fn run_codex_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs).unwrap_err();

        assert!(err.to_string().contains("launching codex; is it installed"));
    }

    #[test]
    fn test_extract_model_from_args() {
        assert_eq!(
            extract_model_from_args(&[
                "--some-flag".to_string(),
                "-m".to_string(),
                "gpt-5.4-mini".to_string()
            ]),
            Some("gpt-5.4-mini".to_string())
        );
        assert_eq!(
            extract_model_from_args(&["--model=gpt-5.4".to_string(), "-c".to_string()]),
            Some("gpt-5.4".to_string())
        );
        assert_eq!(
            extract_model_from_args(&["-m=gpt-5.4-mini".to_string()]),
            Some("gpt-5.4-mini".to_string())
        );
        assert_eq!(extract_model_from_args(&["--some-flag".to_string()]), None);
        assert_eq!(extract_model_from_args(&["-m".to_string()]), None);
    }

    #[test]
    fn run_codex_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_codex(
            &f.worktree,
            "task",
            &f.session_dir,
            Some("gpt-5.4"),
            &[
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "-m".to_string(),
                "legacy-mini".to_string(),
                "--model=older".to_string(),
                "--trace".to_string(),
            ],
            &envs,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(argv.contains(&"--trace".to_string()));
        assert!(argv.contains(&"-m".to_string()));
        assert!(argv.contains(&"gpt-5.4".to_string()));
        assert!(!argv.contains(&"legacy-mini".to_string()));
        assert!(!argv.contains(&"--model".to_string()));
        assert!(!argv.contains(&"--model=older".to_string()));
    }

    // ── run_claude ───────────────────────────────────────────────────────

    #[test]
    fn run_claude_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
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
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 1);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn run_claude_core_argv_and_extra_args_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
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
        let _exec_guard = crate::test_support::ExecGuard::new();
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

    // ── run_vibe ─────────────────────────────────────────────────────────

    #[test]
    fn run_vibe_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_vibe(&f.worktree, "vibe task", &f.session_dir, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-vibe"));
        assert!(log.contains("stderr-marker-vibe"));
    }

    #[test]
    fn run_vibe_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 1);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap();

        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn run_vibe_core_argv_always_includes_trust_and_auto_approve() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_vibe(
            &f.worktree,
            "the vibe task",
            &f.session_dir,
            &["--max-turns".to_string(), "40".to_string()],
            &envs,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"the vibe task".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        assert!(argv.contains(&"--max-turns".to_string()));
        assert!(argv.contains(&"40".to_string()));
    }

    #[test]
    fn run_vibe_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("FROM_ENV_FILE".to_string(), "vibe-env-value".to_string()),
        ];

        run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=vibe-env-value"));
    }

    #[test]
    fn run_vibe_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs).unwrap_err();

        assert!(err.to_string().contains("launching vibe; is it installed"));
    }

    // ── run_agy ─────────────────────────────────────────────────────────

    #[test]
    fn run_agy_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_agy(
            &f.worktree,
            "agy task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            "agy",
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-agy"));
        assert!(log.contains("stderr-marker-agy"));
    }

    #[test]
    fn run_agy_with_executable_passes_print_timeout_when_given() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "Gemini 3.5 Flash (Medium)".into(),
            },
            &envs,
            Some(900),
            120,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        let flag_pos = argv.iter().position(|a| a == "--print-timeout").unwrap();
        assert_eq!(argv[flag_pos + 1], "900s");
    }

    #[test]
    fn run_agy_with_executable_omits_print_timeout_when_absent() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "Gemini 3.5 Flash (Medium)".into(),
            },
            &envs,
            None,
            120,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(!argv.iter().any(|a| a == "--print-timeout"));
    }

    #[test]
    fn run_agy_with_executable_kills_process_after_idle_timeout_with_no_new_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Writes one line, then goes silent for far longer than the idle
        // timeout below -- this must be killed, not allowed to keep running.
        make_fake_bin(
            &f.bin_dir,
            "agy",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        // Needs the real `sleep` binary reachable, not just the fake bin_dir.
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &envs,
            None,
            1, // idle timeout: 1s of silence is stalled
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new output"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_agy_with_executable_does_not_kill_while_output_keeps_arriving() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Writes output every ~1s for 3s total -- longer than the 2s idle
        // timeout below, but never actually goes quiet for that long. Must
        // be allowed to finish naturally, not killed for being slow overall.
        make_fake_bin(
            &f.bin_dir,
            "agy",
            "#!/bin/sh\nfor i in 1 2 3; do echo \"step$i\"; sleep 1; done\n",
        );
        // Needs the real `sleep` binary reachable, not just the fake bin_dir.
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &envs,
            None,
            2, // idle timeout: longer than any single gap between writes
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1") && log.contains("step2") && log.contains("step3"));
        assert!(!log.contains("killed after"));
    }

    #[test]
    fn run_agy_core_argv_and_model_present() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_agy(
            &f.worktree,
            "the agy task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            "agy",
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "--print");
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"gpt-5.4".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(argv.contains(&"the agy task".to_string()));
    }

    #[test]
    fn run_agy_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_agy(
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            "agy",
        )
        .unwrap_err();

        assert!(err.to_string().contains("launching agy; is it installed"));
    }

    #[test]
    fn resolve_backend_executable_prefers_explicit_path() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "claude-explicit", "#!/bin/sh\nexit 0\n");
        let mut profile = test_profile();
        profile.claude_path = Some(f.bin_dir.join("claude-explicit").display().to_string());
        let _guard = PathGuard::set("");

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::Found(f.bin_dir.join("claude-explicit"))
        );
    }

    #[test]
    fn resolve_backend_executable_falls_back_to_path_when_unset() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "claude", "#!/bin/sh\nexit 0\n");
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::Found(f.bin_dir.join("claude"))
        );
    }

    #[test]
    fn resolve_backend_executable_invalid_explicit_path_is_unavailable() {
        let mut profile = test_profile();
        profile.claude_path = Some("/definitely/missing/claude".into());

        let resolved = resolve_backend_executable(&profile, "claude");

        assert_eq!(
            resolved,
            ExecutableResolution::MissingExplicitPath(PathBuf::from("/definitely/missing/claude"))
        );
    }

    #[test]
    fn resolve_backend_executable_supports_codex_and_agy_paths() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "codex-explicit", "#!/bin/sh\nexit 0\n");
        make_fake_bin(&f.bin_dir, "agy-explicit", "#!/bin/sh\nexit 0\n");
        let mut profile = test_profile();
        profile.codex_path = Some(f.bin_dir.join("codex-explicit").display().to_string());
        profile.agy_path = Some(f.bin_dir.join("agy-explicit").display().to_string());

        assert_eq!(
            resolve_backend_executable(&profile, "codex"),
            ExecutableResolution::Found(f.bin_dir.join("codex-explicit"))
        );
        assert_eq!(
            resolve_backend_executable(&profile, "agy"),
            ExecutableResolution::Found(f.bin_dir.join("agy-explicit"))
        );
    }

    #[test]
    fn run_review_backend_times_out_and_preserves_partial_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\necho 'partial review'\nsleep 2\necho 'late stderr' >&2\n",
        );
        let mut profile = test_profile();
        profile.review_timeout_seconds = Some(1);
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Timeout);
        assert!(result.stdout.contains("partial review"));
    }

    #[test]
    fn run_review_backend_supports_agy_with_model_and_env() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "agy",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("Claude Sonnet 4.6 (Thinking)"),
            &[("FROM_ENV_FILE".into(), "agy-review-env".into())],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        assert!(result.stdout.contains("stdout-marker-agy"));
        assert!(result.stderr.contains("stderr-marker-agy"));

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "--print");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"Claude Sonnet 4.6 (Thinking)".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=agy-review-env"));
    }

    // ── backend_available ────────────────────────────────────────────────
    // Not part of the spec's priority list, but it is a one-line pure-ish
    // wrapper around `which` that every routing decision depends on, and it
    // was previously completely untested.

    #[test]
    fn backend_available_false_for_unknown_backend_name() {
        assert!(!backend_available("not-a-real-backend"));
    }

    // ── AGY empty-output detection ────────────────────────────────────

    #[test]
    fn extract_reset_time_parses_standard_format() {
        let log = "RESOURCE_EXHAUSTED (code 429): quota. Resets in 16m44s.";
        assert_eq!(extract_reset_time(log).as_deref(), Some("16m44s"));
    }

    #[test]
    fn extract_reset_time_returns_none_when_absent() {
        assert_eq!(extract_reset_time("no reset info here"), None);
    }

    #[test]
    fn agy_empty_output_with_quota_log_detected_as_error() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Fake agy that exits 0 with no stdout/stderr.
        make_fake_bin(&f.bin_dir, "agy", "#!/bin/sh\nexit 0\n");
        // Write a cli.log with quota error text.
        let agy_home = f.record_dir.parent().unwrap();
        let agy_log_dir = agy_home.join(".gemini/antigravity-cli");
        fs::create_dir_all(&agy_log_dir).unwrap();
        fs::write(
            agy_log_dir.join("cli.log"),
            "RESOURCE_EXHAUSTED (code 429): quota. Resets in 10m.\n",
        )
        .unwrap();

        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("HOME".to_string(), agy_home.to_string_lossy().to_string()),
        ];
        let result = run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            None,
            120,
        )
        .unwrap();
        // Empty output with quota error becomes exit_code=-1
        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("quota exhausted"), "got log: {log}");
    }

    #[test]
    fn agy_empty_output_with_auth_log_detected_as_auth_failure() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "agy", "#!/bin/sh\nexit 0\n");
        let agy_home = f.record_dir.parent().unwrap();
        let agy_log_dir = agy_home.join(".gemini/antigravity-cli");
        fs::create_dir_all(&agy_log_dir).unwrap();
        fs::write(
            agy_log_dir.join("cli.log"),
            "error getting token source: You are not logged into Antigravity.\n",
        )
        .unwrap();

        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("HOME".to_string(), agy_home.to_string_lossy().to_string()),
        ];
        let result = run_agy_with_executable(
            &f.bin_dir.join("agy"),
            &f.worktree,
            "task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            None,
            120,
        )
        .unwrap();
        // Empty output with auth error becomes exit_code=-1
        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("not authenticated"), "got log: {log}");
    }

    #[test]
    fn agy_successful_output_not_affected_by_detection() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        // Normal agy that produces stdout content.
        make_recording_bin(&f.bin_dir, "agy", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_agy(
            &f.worktree,
            "normal task",
            &f.session_dir,
            &LlmConfig {
                base_url: "http://llm.test".into(),
                api_key: "test-key".into(),
                model: "gpt-5.4".into(),
            },
            &envs,
            "agy",
        )
        .unwrap();
        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-agy"), "normal output preserved");
    }
}
