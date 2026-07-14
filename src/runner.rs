use crate::config::Profile;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use std::time::Instant;

pub(crate) mod output;
pub(crate) mod process;

pub(crate) use crate::runner::process::copy_stream_to_file;
pub use crate::runner::process::install_shutdown_handler;
pub(crate) use crate::runner::process::kill_process_group;
pub(crate) use crate::runner::process::prepare_process_group;
pub use crate::runner::process::shutdown_requested;
pub(crate) use crate::runner::process::spawn_with_idle_watch;
pub(crate) use crate::runner::process::spawn_with_worktree_progress_watch;
pub(crate) use crate::runner::process::write_redacted_task;

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
    /// Authoritative final assistant text selected by the backend adapter.
    /// Raw stdout/stderr is never promoted into this field.
    pub final_summary: Option<String>,
    /// TICKET-066/#155: for AGY backends, the bytes appended to AGY's
    /// `cli.log` during this specific run, scoped to the pre-run byte
    /// offset (so concurrent appends from other AGY instances/log sources
    /// are excluded). `None` for non-AGY backends and for runs where the
    /// cli.log could not be read. This delta — not a fresh read of the
    /// whole log — is what usage/quota parsing consumes, so a single
    /// attempt's usage is never polluted by prior runs.
    pub agy_cli_log_delta: Option<String>,
    /// A run-scoped tail of a backend-owned diagnostic log. Unlike
    /// `log_path`, this is not CLI stdout/stderr: some backends (notably
    /// OpenCode) write provider failures only to their own internal log.
    /// The tail begins at the pre-run byte offset, so old failures and other
    /// runs cannot poison routing for this attempt.
    pub internal_log_delta: Option<String>,
    /// Source path for `internal_log_delta`, retained for availability
    /// diagnostics. Missing/unreadable logs leave both fields `None`.
    pub internal_log_path: Option<String>,
    /// Backend-owned structured usage artifact. Claude uses its transcript
    /// JSONL; Vibe uses its session `meta.json`. `None` when the backend did
    /// not produce a discoverable artifact.
    pub transcript_path: Option<String>,
    /// AGY CLI version for this run (e.g. "1.0.16"), captured via
    /// `agy --version`. `None` for non-AGY backends and for runs where version
    /// detection fails. Used for log-path resolution and upstream log-format
    /// drift detection (TICKET-242).
    pub agy_version: Option<String>,
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
///
/// Issue #87: kills the process once its log has gone quiet for
/// `idle_timeout_seconds` -- OpenHands has no timeout of its own, and a
/// hung/looping run previously blocked `gah dispatch` (and anything built
/// on it, like `gah loop --once`) indefinitely. Mirrors
/// `run_opencode_with_executable`'s idle-watch loop exactly.
pub fn run_openhands(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    llm: &LlmConfig,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

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
    .current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching openhands; is it installed and on PATH?",
    )?;

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary: fs::read_to_string(&log_path)
            .ok()
            .and_then(|text| output::extract_openhands_jsonl_summary(&text)),
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: None,
        agy_version: None,
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
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_codex_with_executable(
        Path::new("codex"),
        worktree,
        task,
        session_dir,
        model,
        extra_args,
        env_vars,
        idle_timeout_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_codex_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

    let mut cmd = Command::new(executable);
    // Issue #152: --json produces structured JSONL output for programmatic
    // usage extraction in parse_codex_exec_json (usage.rs).
    cmd.arg("exec")
        .arg("--json")
        .arg(task)
        .args(filtered_codex_args(extra_args))
        .args(codex_model_args(model))
        .current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching codex; is it installed and on PATH?",
    )?;

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary: fs::read_to_string(&log_path)
            .ok()
            .and_then(|text| output::extract_codex_jsonl_summary(&text)),
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: None,
        agy_version: None,
    })
}

fn codex_model_args(model: Option<&str>) -> Vec<String> {
    model
        .map(|model| vec!["-m".to_string(), model.to_string()])
        .unwrap_or_default()
}

fn filtered_codex_args(extra_args: &[String]) -> Vec<String> {
    filtered_backend_args("codex", extra_args)
}

pub fn extract_model_from_backend_args(backend: &str, args: &[String]) -> Option<String> {
    match backend {
        "codex" => {
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
        "opencode" | "claude" => {
            let mut i = 0;
            while i < args.len() {
                let arg = &args[i];
                if arg == "--model" {
                    if i + 1 < args.len() {
                        return Some(args[i + 1].clone());
                    }
                    break;
                }
                if let Some(val) = arg.strip_prefix("--model=") {
                    return Some(val.to_string());
                }
                i += 1;
            }
            None
        }
        _ => None,
    }
}

pub fn filtered_backend_args(backend: &str, extra_args: &[String]) -> Vec<String> {
    let mut filtered = Vec::with_capacity(extra_args.len());
    let mut i = 0;
    while i < extra_args.len() {
        let arg = &extra_args[i];
        match backend {
            "codex" => {
                if matches!(arg.as_str(), "-m" | "--model") {
                    i += 2;
                    continue;
                }
                if arg.starts_with("-m=") || arg.starts_with("--model=") {
                    i += 1;
                    continue;
                }
            }
            "opencode" | "claude" => {
                if arg == "--model" {
                    i += 2;
                    continue;
                }
                if arg.starts_with("--model=") {
                    i += 1;
                    continue;
                }
            }
            _ => {}
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
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_claude_with_executable(
        Path::new("claude"),
        worktree,
        task,
        session_dir,
        None,
        extra_args,
        env_vars,
        idle_timeout_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_claude_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    effective_model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;

    // Issue #153: pin a stable session id so we can locate the exact
    // transcript `.jsonl` Claude Code writes afterwards (the source of real
    // per-attempt token/cost usage, rather than scraping stdout).
    let session_id = uuid::Uuid::new_v4().to_string();
    // Find the HOME this invocation will run under (a per-attempt HOME is
    // injected via env_vars; fall back to the ambient HOME).
    let home = env_vars
        .iter()
        .find_map(|(k, v)| (k == "HOME").then_some(PathBuf::from(v)))
        .or_else(|| env::var("HOME").ok().map(PathBuf::from));

    let mut cmd = Command::new(executable);
    cmd.args([
        "-p",
        task,
        "--output-format",
        "text",
        "--verbose",
        "--session-id",
        &session_id,
    ])
    .current_dir(worktree);
    if let Some(model) = effective_model {
        cmd.args(["--model", model]);
    }
    cmd.args(filtered_backend_args("claude", extra_args));
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching claude; is it installed and on PATH?",
    )?;

    // Locate the transcript for the pinned session id so per-attempt usage
    // parsing can consume it.
    let transcript_path = home
        .as_ref()
        .and_then(|h| crate::claude_monitor::find_claude_transcript(h, worktree, &session_id))
        .map(|p| p.to_string_lossy().into_owned());
    let final_summary = transcript_path
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| output::extract_claude_transcript_summary(&text));

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary,
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path,
        agy_version: None,
    })
}

/// Run Mistral's Vibe CLI non-interactively via `vibe -p`.
/// Used for both worker/fix and review execution.
/// extra_args come from profile.vibe_args (e.g. `--max-turns 40 --max-price 2`).
/// No --model flag exists on this CLI; model selection is config/env-var
/// driven on vibe's own side (VIBE_ACTIVE_MODEL / ~/.vibe/config.toml),
/// so GAH binds the effective route model through `VIBE_ACTIVE_MODEL`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_vibe(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_vibe_with_executable(
        Path::new("vibe"),
        worktree,
        task,
        session_dir,
        None,
        extra_args,
        env_vars,
        idle_timeout_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_vibe_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    effective_model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;
    let started_at = std::time::SystemTime::now();
    // Vibe can retain old session directories for the same worktree. Capture
    // them before launch so a reused or concurrently-updated metadata file is
    // never misreported as this attempt's token consumption.
    let sessions_before = snapshot_vibe_session_metadata_paths(env_vars);

    let mut cmd = Command::new(executable);
    // --trust: automation-only, not persisted to trusted_folders.toml --
    // skips the interactive trust prompt without touching global config.
    // --auto-approve: same automation need as agy's --dangerously-skip-permissions.
    // Vibe's durable session metadata supplies the authoritative token/model
    // totals. Keep the established text output mode for the runner's idle
    // watcher and backend summary handling.
    cmd.args(["-p", task, "--trust", "--auto-approve"])
        .args(extra_args)
        .current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    // Vibe has no per-invocation --model flag. Its documented environment
    // selector must receive the route's effective model so the actual model
    // and the ledger attribution cannot diverge. Set this after env_file
    // variables so the route is authoritative for this attempt.
    if let Some(model) = effective_model {
        cmd.env("VIBE_ACTIVE_MODEL", model);
    }

    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching vibe; is it installed and on PATH?",
    )?;

    let metadata_path =
        find_vibe_session_metadata(env_vars, worktree, started_at, &sessions_before);
    let final_summary = metadata_path
        .as_deref()
        .and_then(|path| Path::new(path).parent())
        .map(|directory| directory.join("messages.jsonl"))
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| output::extract_vibe_messages_summary(&text));

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary,
        agy_cli_log_delta: None,
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: metadata_path,
        agy_version: None,
    })
}

fn vibe_sessions_dir(env_vars: &[(String, String)]) -> Option<PathBuf> {
    let home = env_vars
        .iter()
        .find(|(key, _)| key == "VIBE_HOME")
        .map(|(_, value)| PathBuf::from(value))
        .or_else(|| {
            env_vars
                .iter()
                .find(|(key, _)| key == "HOME")
                .map(|(_, value)| PathBuf::from(value).join(".vibe"))
        })
        .or_else(|| env::var_os("VIBE_HOME").map(PathBuf::from))
        .or_else(|| env::var_os("HOME").map(|value| PathBuf::from(value).join(".vibe")))?;
    Some(home.join("logs/session"))
}

fn snapshot_vibe_session_metadata_paths(env_vars: &[(String, String)]) -> HashSet<PathBuf> {
    let Some(sessions) = vibe_sessions_dir(env_vars) else {
        return HashSet::new();
    };
    fs::read_dir(sessions)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("meta.json"))
        .filter(|path| path.is_file())
        .collect()
}

/// Locate a *new* Vibe session created by this invocation. Merely matching a
/// worktree and recent mtime is insufficient: Vibe can update a pre-existing
/// session whose counters are cumulative. If no new metadata file appears,
/// usage is deliberately left unknown rather than attributed incorrectly.
fn find_vibe_session_metadata(
    env_vars: &[(String, String)],
    worktree: &Path,
    started_at: std::time::SystemTime,
    sessions_before: &HashSet<PathBuf>,
) -> Option<String> {
    let sessions = vibe_sessions_dir(env_vars)?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let worktree_string = worktree.to_string_lossy().into_owned();
    for entry in fs::read_dir(sessions).ok()?.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let metadata_path = dir.join("meta.json");
        if sessions_before.contains(&metadata_path) {
            continue;
        }
        let Ok(modified) = fs::metadata(&metadata_path).and_then(|metadata| metadata.modified())
        else {
            continue;
        };
        if modified < started_at {
            continue;
        }
        let Ok(metadata) = fs::read_to_string(&metadata_path) else {
            continue;
        };
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&metadata) else {
            continue;
        };
        let cwd = root
            .get("environment")
            .and_then(|environment| environment.get("working_directory"))
            .and_then(|value| value.as_str());
        if cwd != Some(worktree_string.as_str()) {
            continue;
        }
        if best
            .as_ref()
            .map(|(time, _)| modified > *time)
            .unwrap_or(true)
        {
            best = Some((modified, metadata_path));
        }
    }
    best.map(|(_, path)| path.to_string_lossy().into_owned())
}

/// Run OpenCode CLI non-interactively via `opencode run --model <model> --dir <path> --auto `<prompt>`.
/// Worker/fix backend and review backend support.
/// extra_args come from profile.opencode_args (e.g. `--format json`).
/// Unlike vibe, opencode DOES take --model, so we pass effective_model through.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_opencode(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    run_opencode_with_executable(
        Path::new("opencode"),
        worktree,
        task,
        session_dir,
        model,
        extra_args,
        env_vars,
        idle_timeout_seconds,
    )
}

/// Issue #170: a live dispatch hung for 3+ hours with zero output and no
/// supervision at all -- opencode had no timeout of any kind (the previous
/// implementation used a plain blocking `cmd.status()`). Now uses the same
/// idle-detection approach as `run_agy_with_executable`: kill only when the
/// log has genuinely gone quiet for `idle_timeout_seconds`, never on a flat
/// wall-clock budget. OpenCode's own narration is not trusted as progress:
/// only a durable worktree change resets this backend's window.
#[allow(clippy::too_many_arguments)]
pub fn run_opencode_with_executable(
    executable: &Path,
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    model: Option<&str>,
    extra_args: &[String],
    env_vars: &[(String, String)],
    idle_timeout_seconds: u64,
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    write_redacted_task(session_dir, task)?;
    let started_at = std::time::SystemTime::now();
    // OpenCode emits provider-side failures (including the observed Hy3
    // rate-limit response) to this internal log, not reliably to stdout or
    // stderr. Snapshot its byte length before launch so only this attempt's
    // appended evidence can influence availability/routing.
    let opencode_log = opencode_log_path(env_vars);
    let opencode_log_pre_offset = opencode_log
        .as_ref()
        .and_then(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
        .unwrap_or(0);

    let mut cmd = Command::new(executable);
    // opencode run --model <model> --dir <path> --auto "<prompt>"
    cmd.arg("run").arg("--dir").arg(worktree).arg("--auto");

    // Add model if specified
    if let Some(model) = model {
        cmd.arg("--model").arg(model);
    }

    // Add task as the last argument (prompt)
    cmd.arg(task);

    // Add extra args from profile
    cmd.args(filtered_backend_args("opencode", extra_args));

    cmd.current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let (exit_code, duration_secs) = spawn_with_worktree_progress_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        "launching opencode; is it installed and on PATH?",
    )?;
    let (transcript_path, final_summary) =
        snapshot_opencode_session(env_vars, worktree, started_at, session_dir)
            .map(|snapshot| (Some(snapshot.path), snapshot.final_summary))
            .unwrap_or((None, None));

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        final_summary,
        agy_cli_log_delta: None,
        internal_log_delta: log_delta(&opencode_log, opencode_log_pre_offset),
        internal_log_path: opencode_log.map(|path| path.to_string_lossy().into_owned()),
        transcript_path,
        agy_version: None,
    })
}

/// Locate OpenCode's process-wide diagnostic log using the same data-home
/// resolution as its SQLite session store. The configured per-run HOME/XDG
/// environment wins over the parent process, which keeps isolated backend
/// instances from reading each other's diagnostics.
fn opencode_log_path(env_vars: &[(String, String)]) -> Option<PathBuf> {
    let value_for = |name: &str| {
        env_vars
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| env::var(name).ok())
    };
    value_for("XDG_DATA_HOME")
        .map(|path| PathBuf::from(path).join("opencode/log/opencode.log"))
        .or_else(|| {
            value_for("HOME")
                .map(|home| PathBuf::from(home).join(".local/share/opencode/log/opencode.log"))
        })
}

/// Persist the exact OpenCode session created by this invocation as a small
/// JSON artifact. Querying by worktree and start time prevents concurrent
/// workers from attributing each other's SQLite rows.
struct OpenCodeSessionSnapshot {
    path: String,
    final_summary: Option<String>,
}

fn snapshot_opencode_session(
    env_vars: &[(String, String)],
    worktree: &Path,
    started_at: std::time::SystemTime,
    session_dir: &Path,
) -> Option<OpenCodeSessionSnapshot> {
    let value_for = |name: &str| {
        env_vars
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| env::var(name).ok())
    };
    let database = value_for("XDG_DATA_HOME")
        .map(|path| PathBuf::from(path).join("opencode/opencode.db"))
        .or_else(|| {
            value_for("HOME")
                .map(|home| PathBuf::from(home).join(".local/share/opencode/opencode.db"))
        })?;
    let started_at_ms = started_at
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let mut statement = connection
        .prepare(
            "SELECT id, model, tokens_input, tokens_output, tokens_reasoning, \
             tokens_cache_read, tokens_cache_write, time_updated \
             FROM session WHERE directory = ?1 AND time_created >= ?2 \
             ORDER BY time_updated DESC LIMIT 1",
        )
        .ok()?;
    let snapshot = statement
        .query_row(
            rusqlite::params![worktree.to_string_lossy(), started_at_ms],
            |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "model": serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(1)?)
                        .unwrap_or(serde_json::Value::Null),
                    "tokens_input": row.get::<_, i64>(2)? as u64,
                    "tokens_output": row.get::<_, i64>(3)? as u64,
                    "tokens_reasoning": row.get::<_, i64>(4)? as u64,
                    "tokens_cache_read": row.get::<_, i64>(5)? as u64,
                    "tokens_cache_write": row.get::<_, i64>(6)? as u64,
                    "time_updated": row.get::<_, i64>(7)?,
                }))
            },
        )
        .ok()?;
    let session_id = snapshot.get("id")?.as_str()?;
    let final_summary = connection
        .prepare(
            "SELECT message.data, part.data FROM message \
             JOIN part ON part.message_id = message.id \
             WHERE message.session_id = ?1 \
             ORDER BY message.time_created DESC, part.time_created DESC",
        )
        .ok()
        .and_then(|mut statement| {
            let rows = statement
                .query_map(rusqlite::params![session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .ok()?;
            for row in rows.flatten() {
                let Ok(message) = serde_json::from_str::<serde_json::Value>(&row.0) else {
                    continue;
                };
                let Ok(part) = serde_json::from_str::<serde_json::Value>(&row.1) else {
                    continue;
                };
                if message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
                    && message.get("finish").and_then(serde_json::Value::as_str) == Some("stop")
                    && part.get("type").and_then(serde_json::Value::as_str) == Some("text")
                {
                    if let Some(text) = part
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                    {
                        return Some(text.to_string());
                    }
                }
            }
            None
        });
    let path = session_dir.join("opencode-session.json");
    fs::write(&path, serde_json::to_vec(&snapshot).ok()?).ok()?;
    Some(OpenCodeSessionSnapshot {
        path: path.to_string_lossy().into_owned(),
        final_summary,
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

/// Detect AGY CLI version by running `agy --version`. Returns `None` on any
/// failure (missing binary, non-zero exit, unparseable output). Cheap and used
/// for log-path selection and upstream log-format drift detection (TICKET-242).
fn detect_agy_version(
    executable: &Path,
    worktree: &Path,
    env_vars: &[(String, String)],
) -> Option<String> {
    let mut cmd = Command::new(executable);
    cmd.arg("--version").current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Accept "antigravity-cli version 1.0.16" or a bare "1.0.16" (an optional
    // leading `v`/`V` is tolerated). AGY versions are `MAJOR.MINOR.PATCH`, so
    // take the last whitespace-separated token that looks like a dotted-numeric
    // version.
    version_str
        .split_whitespace()
        .rev()
        .find(|tok| {
            let t = tok.strip_prefix(['v', 'V']).unwrap_or(tok);
            t.split('.').count() >= 2 && t.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .map(|s| s.strip_prefix(['v', 'V']).unwrap_or(s).to_string())
}

/// AGY cli.log layout, keyed by the first upstream version that introduced it
/// (TICKET-242). A future upstream log relocation is a new table row here, not
/// a code archaeology session in `agy_cli_log_path`.
///
/// Each entry is `(first_version, candidate_paths)`: `first_version` is the
/// semver-style lower bound (inclusive) at which the layout appeared, and
/// `candidate_paths` are resolved relative to `~/.gemini/antigravity-cli` (a
/// file is used directly; a directory is scanned for the newest `cli-*` file).
const AGY_LOG_PATHS: &[(&str, &[&str])] = &[
    // Earliest releases: a single `cli.log` file.
    ("0.0.0", &["cli.log"]),
    // v1.0.0+: logs are rotated under `log/` as `cli-*.log`.
    ("1.0.0", &["log"]),
];

/// Compare two dotted-numeric version strings (e.g. "1.0.16" vs "1.0.0").
/// Returns the [`std::cmp::Ordering`] of `a` relative to `b`. Non-numeric
/// components compare as `0`, which is fine for the `MAJOR.MINOR.PATCH`
/// versions AGY emits.
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let ap: Vec<u64> = a.split('.').filter_map(|p| p.parse().ok()).collect();
    let bp: Vec<u64> = b.split('.').filter_map(|p| p.parse().ok()).collect();
    let max = ap.len().max(bp.len());
    for i in 0..max {
        let av = *ap.get(i).unwrap_or(&0);
        let bv = *bp.get(i).unwrap_or(&0);
        match av.cmp(&bv) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Candidate cli.log locations (relative to the AGY root) for the detected
/// version. When the version is unknown we consider every layout ever seen, so
/// resolution still works against an unrecognized/old CLI.
fn agy_log_candidates(version: Option<&str>) -> Vec<&'static str> {
    // AGY_LOG_PATHS is defined oldest-first, but candidates must be tried
    // newest-first: a version at or above multiple thresholds (the common
    // case, since thresholds are cumulative lower bounds) would otherwise
    // resolve the OLDEST matching layout first. A stale `cli.log` left over
    // from a pre-1.0.0 install, sitting alongside a freshly-populated `log/`
    // directory the upgraded CLI is actually writing to, would then win --
    // silently reading a dead log file for exactly the upgrade-transition
    // window this table exists to handle correctly.
    match version {
        Some(v) => AGY_LOG_PATHS
            .iter()
            .rev()
            .filter(|(first, _)| version_cmp(v, first) != std::cmp::Ordering::Less)
            .flat_map(|(_, cands)| cands.iter().copied())
            .collect(),
        None => AGY_LOG_PATHS
            .iter()
            .rev()
            .flat_map(|(_, cands)| cands.iter().copied())
            .collect(),
    }
}

/// Resolve AGY's cli.log path from the HOME that the run actually uses
/// (the per-call `env_vars` win over process `HOME`, matching how the run's
/// effective HOME is resolved elsewhere). `version` selects the candidate
/// layout(s) from `AGY_LOG_PATHS` (keyed by version range); when `None` every
/// known layout is tried. Returns `None` only when no HOME is discoverable or
/// no candidate log exists -- in which case there is no cli.log to delta
/// against.
fn agy_cli_log_path(
    env_vars: &[(String, String)],
    _executable: &Path,
    version: Option<&str>,
) -> Option<PathBuf> {
    let home = env_vars
        .iter()
        .find(|(k, _)| k == "HOME")
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var("HOME").ok())?;
    let root = PathBuf::from(home).join(".gemini/antigravity-cli");
    for rel in agy_log_candidates(version) {
        let cand = root.join(rel);
        if cand.is_file() {
            return Some(cand);
        }
        if cand.is_dir() {
            if let Some(newest) = newest_cli_in(&cand) {
                return Some(newest);
            }
        }
    }
    None
}

/// Newest `cli-*` file inside `dir`, used for rotated AGY log layouts.
fn newest_cli_in(dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() || !path.file_name()?.to_string_lossy().starts_with("cli-") {
            continue;
        }
        let modified = fs::metadata(&path).ok()?.modified().ok()?;
        if newest
            .as_ref()
            .map(|(time, _)| modified > *time)
            .unwrap_or(true)
        {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

/// Returns `true` when `delta` is non-empty yet contains none of the known AGY
/// cli.log signatures. That is the TICKET-242 drift canary: an AGY run failed
/// with empty output, the current-run log delta matched zero known signatures,
/// and the delta is non-empty -- so upstream has silently changed its log
/// format/path and classification has degraded.
fn is_agy_log_format_unrecognized(delta: &str) -> bool {
    if delta.trim().is_empty() {
        return false;
    }
    const KNOWN_SIGNATURES: &[&str] = &[
        "RESOURCE_EXHAUSTED",
        "Individual quota reached",
        "Quota exceeded",
        "quota has been reached",
        "quota reached",
        "not logged into Antigravity",
        "not logged in",
        "AGY not authenticated",
        "Resets in ",
        "resets at ",
        "Your quota resets at",
    ];
    !KNOWN_SIGNATURES.iter().any(|sig| delta.contains(sig))
}

/// Read only bytes appended to a backend-owned log after `pre_offset`.
/// Returns `None` on missing/unreadable logs and treats a truncated/unchanged
/// log as no delta. Lossy decoding deliberately preserves diagnostic text
/// even if a backend left a partial UTF-8 write while exiting.
fn log_delta(log: &Option<PathBuf>, pre_offset: u64) -> Option<String> {
    let path = log.as_ref()?;
    let bytes = fs::read(path).ok()?;
    if (pre_offset as usize) >= bytes.len() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[pre_offset as usize..]).into_owned())
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
    write_redacted_task(session_dir, task)?;

    // TICKET-242: detect the AGY CLI version up front. It is cheap, good
    // attribution, and drives log-path resolution plus upstream log-format
    // drift detection below.
    let agy_version = detect_agy_version(executable, worktree, env_vars);

    let mut cmd = Command::new(executable);
    cmd.arg("--print");
    cmd.arg(task);
    cmd.args(["--model", llm.model.as_str()]);
    if let Some(secs) = print_timeout_seconds {
        cmd.args(["--print-timeout", &format!("{secs}s")]);
    }

    // #155: scope AGY's cli.log to just the bytes this run appends. Capture
    // the pre-run byte offset up front so that, after the run, we read the
    // new tail only (not the whole log, which may contain unrelated
    // history or concurrent appends from other AGY instances). `None` when
    // the cli.log isn't readable yet is fine -- the delta stays `None` and
    // usage/quota parsing simply has nothing to draw from for this attempt.
    // The path is resolved via the version-keyed `AGY_LOG_PATHS` table.
    let agy_cli_log = agy_cli_log_path(env_vars, executable, agy_version.as_deref());
    let agy_cli_log_pre_offset = agy_cli_log
        .as_ref()
        .and_then(|p| fs::metadata(p).ok().map(|m| m.len()))
        .unwrap_or(0);
    cmd.arg("--dangerously-skip-permissions")
        .current_dir(worktree);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    // GAH-side supervision: kill only when the log has genuinely gone quiet
    // for idle_timeout_seconds, not on a flat wall-clock budget. A model
    // that's slow but still producing output (still working) is never
    // killed for being slow; --print-timeout above stays as an outer
    // safety backstop for a truly hung process.
    let (exit_code, duration_secs) = spawn_with_idle_watch(
        cmd,
        &log_path,
        worktree,
        idle_timeout_seconds,
        &format!(
            "launching {}; is it installed and on PATH?",
            executable.display()
        ),
    )?;

    // Read captured stdout to detect silent failures. (A kill-for-idle
    // already leaves exit_code at -1, so the exit_code == 0 guard below
    // naturally skips this diagnosis for that case.)
    let output = fs::read_to_string(&log_path).unwrap_or_default();
    let trimmed = output.trim();

    // AGY sometimes exits 0 with empty output when quota is exhausted or
    // auth has expired.  Treat empty output at exit 0 as a failure and
    // try to classify the real cause from AGY's own log.
    if trimmed.is_empty() && exit_code == 0 {
        // TICKET-242 drift canary: if the run-scoped log delta is non-empty
        // but matches zero known signatures, upstream has silently changed its
        // log format/path. Emit a distinct note so the degradation is visible
        // the day it happens, instead of being silently classified unknown.
        let agy_cli_log_delta = log_delta(&agy_cli_log, agy_cli_log_pre_offset);
        if is_agy_log_format_unrecognized(agy_cli_log_delta.as_deref().unwrap_or("")) {
            let drift_msg = format!(
                "[agy_log_format_unrecognized] AGY produced no output with an unrecognized cli.log delta (agy_version={}). Upstream log format/path may have changed; quota/auth classification is degraded.",
                agy_version.as_deref().unwrap_or("unknown"),
            );
            if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&log_path) {
                let _ = writeln!(file, "{}", drift_msg);
            }
        }

        let err_msg = agy_empty_output_diagnosis(env_vars, executable);

        if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&log_path) {
            let _ = writeln!(file, "{}", err_msg);
        }

        return Ok(RunResult {
            exit_code: -1,
            duration_secs,
            log_path: log_path.to_string_lossy().into_owned(),
            final_summary: None,
            agy_cli_log_delta,
            internal_log_delta: None,
            internal_log_path: None,
            transcript_path: None,
            agy_version: agy_version.clone(),
        });
    }

    Ok(RunResult {
        exit_code,
        duration_secs,
        log_path: log_path.to_string_lossy().into_owned(),
        // AGY --print currently mixes stdout and stderr in the diagnostic
        // log and exposes no run-scoped structured conversation artifact.
        // Fail closed until its adapter can identify one authoritatively.
        final_summary: None,
        agy_cli_log_delta: log_delta(&agy_cli_log, agy_cli_log_pre_offset),
        internal_log_delta: None,
        internal_log_path: None,
        transcript_path: None,
        agy_version,
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
    let _ = write_redacted_task(session_dir, prompt);

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
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
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
        "vibe" => {
            cmd.arg("-p").arg(prompt);
            cmd.arg("--output").arg("text");
            cmd.arg("--trust");
            cmd.arg("--auto-approve");
            // Vibe does not accept --model flag; model selection is via
            // environment/config. We record the effective model separately
            // in ledger metadata but do not pass it to Vibe CLI.
        }
        "opencode" => {
            cmd.arg("run");
            if let Some(model) = effective_model {
                cmd.args(["--model", model]);
            }
            cmd.arg(prompt);
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
    prepare_process_group(&mut cmd);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    if backend == "vibe" {
        if let Some(model) = effective_model {
            cmd.env("VIBE_ACTIVE_MODEL", model);
        }
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
                if shutdown_requested() {
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::SignalTermination(libc::SIGTERM);
                }
                if start.elapsed() >= timeout {
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    break ReviewProcessOutcome::Timeout;
                }
                thread::sleep(poll_interval);
            }
            Err(err) => {
                kill_process_group(&mut child);
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
        "opencode" => Some("opencode"),
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

    fn initialize_git_worktree(worktree: &Path) {
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

    fn test_llm() -> LlmConfig {
        LlmConfig {
            base_url: "http://llm.test".into(),
            api_key: "test-key".into(),
            model: "test-model".into(),
        }
    }

    fn test_profile() -> Profile {
        Profile {
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
            publishing: Default::default(),
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
            300,
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
    fn backend_output_and_task_artifacts_are_redacted_before_persisting() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "openhands",
            "#!/bin/sh\necho 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz'\necho 'glpat-abcdefghijklmnopqrstuv' >&2\n",
        );
        let envs = vec![("PATH".to_string(), f.bin_dir.to_string_lossy().into_owned())];

        let result = run_openhands(
            &f.worktree,
            "task includes ghp_abcdefghijklmnopqrstuvwxyz",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let log = fs::read_to_string(result.log_path).unwrap();
        let task = fs::read_to_string(f.session_dir.join("task.md")).unwrap();
        for raw in [
            "abcdefghijklmnopqrstuvwxyz",
            "glpat-abcdefghijklmnopqrstuv",
            "ghp_abcdefghijklmnopqrstuvwxyz",
        ] {
            assert!(!log.contains(raw), "raw secret persisted in log: {raw}");
            assert!(!task.contains(raw), "raw secret persisted in task: {raw}");
        }
        assert!(log.contains("[REDACTED:TOKEN]"));
        assert!(task.contains("[REDACTED:GITHUB_TOKEN]"));
    }

    #[test]
    fn run_openhands_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "openhands", &f.record_dir, 3);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_openhands(
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap();

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
            300,
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

        run_openhands(&f.worktree, "task", &f.session_dir, &llm, &[], &envs, 300).unwrap();

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

        run_openhands(
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=env-file-value"));
    }

    #[test]
    fn run_openhands_missing_binary_produces_useful_error() {
        let f = fixture(); // bin_dir stays empty — no openhands on PATH
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_openhands(
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            300,
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("launching openhands; is it installed"));
    }

    #[test]
    fn run_openhands_kills_process_after_idle_timeout_with_no_new_output() {
        // Issue #87: a live openhands dispatch ran for 2h20m+ with no
        // supervision at all -- run_openhands previously used a plain
        // blocking `cmd.status()`. This pins the fix: it must be killed
        // once output has genuinely stopped, not allowed to run forever.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "openhands",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_openhands(
            &f.worktree,
            "task",
            &f.session_dir,
            &test_llm(),
            &[],
            &envs,
            1, // idle timeout: 1s of silence is stalled
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }

    // ── run_codex ────────────────────────────────────────────────────────

    #[test]
    fn run_codex_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "codex", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_codex(
            &f.worktree,
            "codex task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap();

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

        let result = run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap();

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
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--json");
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

        run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=codex-env-value"));
    }

    #[test]
    fn run_codex_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err =
            run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap_err();

        assert!(err.to_string().contains("launching codex; is it installed"));
    }

    #[test]
    fn run_codex_kills_process_after_idle_timeout_with_no_new_output() {
        // codex used a plain blocking cmd.status() with zero supervision,
        // same class of bug as issues #87/#170. Pins the shared
        // spawn_with_idle_watch fix for this backend.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "codex",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_codex(&f.worktree, "task", &f.session_dir, None, &[], &envs, 1).unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new backend output or worktree progress"),
            "got log: {log}"
        );
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
    fn test_extract_model_from_backend_args() {
        // codex: -m or --model
        assert_eq!(
            extract_model_from_backend_args(
                "codex",
                &[
                    "--some-flag".to_string(),
                    "-m".to_string(),
                    "gpt-5.4-mini".to_string()
                ]
            ),
            Some("gpt-5.4-mini".to_string())
        );
        assert_eq!(
            extract_model_from_backend_args("codex", &["--model=gpt-5.4".to_string()]),
            Some("gpt-5.4".to_string())
        );
        assert_eq!(
            extract_model_from_backend_args("codex", &["-m=gpt-5.4-mini".to_string()]),
            Some("gpt-5.4-mini".to_string())
        );

        // opencode and claude: --model only (not -m)
        assert_eq!(
            extract_model_from_backend_args(
                "opencode",
                &[
                    "--some-flag".to_string(),
                    "-m".to_string(),
                    "gpt-5.4-mini".to_string()
                ]
            ),
            None
        );
        assert_eq!(
            extract_model_from_backend_args(
                "opencode",
                &["--model".to_string(), "gpt-5.4-mini".to_string()]
            ),
            Some("gpt-5.4-mini".to_string())
        );
        assert_eq!(
            extract_model_from_backend_args("claude", &["--model=gpt-5.4".to_string()]),
            Some("gpt-5.4".to_string())
        );
    }

    #[test]
    fn test_filtered_backend_args() {
        // codex: filters -m and --model
        let args = vec![
            "-m".to_string(),
            "stale-model".to_string(),
            "--model=another-stale".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];
        let filtered = filtered_backend_args("codex", &args);
        assert_eq!(filtered, vec!["--format".to_string(), "json".to_string()]);

        // opencode: filters --model only, leaves -m intact
        let args = vec![
            "-m".to_string(),
            "stale-model".to_string(),
            "--model=another-stale".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];
        let filtered = filtered_backend_args("opencode", &args);
        assert_eq!(
            filtered,
            vec![
                "-m".to_string(),
                "stale-model".to_string(),
                "--format".to_string(),
                "json".to_string()
            ]
        );
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
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--json");
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

        let result =
            run_claude(&f.worktree, "claude task", &f.session_dir, &[], &envs, 300).unwrap();

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

        let result = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap();

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
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"the claude task".to_string()));
        assert!(argv.contains(&"--allowedTools".to_string()));
        assert!(argv.contains(&"Edit,Bash".to_string()));
    }

    #[test]
    fn run_claude_binds_the_effective_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_claude_with_executable(
            &f.bin_dir.join("claude"),
            &f.worktree,
            "the claude task",
            &f.session_dir,
            Some("haiku"),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
    }

    #[test]
    fn run_claude_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_claude_with_executable(
            &f.bin_dir.join("claude"),
            &f.worktree,
            "the claude task",
            &f.session_dir,
            Some("haiku"),
            &[
                "--allowedTools".to_string(),
                "Edit".to_string(),
                "--model".to_string(),
                "opus".to_string(),
                "--model=sonnet".to_string(),
            ],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
        assert!(argv.contains(&"--allowedTools".to_string()));
        assert!(!argv.contains(&"opus".to_string()));
        assert!(!argv.contains(&"sonnet".to_string()));
        assert!(!argv.contains(&"--model=sonnet".to_string()));
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

        run_claude(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=claude-env-value"));
    }

    #[test]
    fn run_claude_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap_err();

        assert!(err
            .to_string()
            .contains("launching claude; is it installed"));
    }

    #[test]
    fn run_claude_kills_process_after_idle_timeout_with_no_new_output() {
        // claude used a plain blocking cmd.status() with zero supervision,
        // same class of bug as issues #87/#170. Pins the shared
        // spawn_with_idle_watch fix for this backend.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "claude",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_claude(&f.worktree, "task", &f.session_dir, &[], &envs, 1).unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }

    // ── run_vibe ─────────────────────────────────────────────────────────

    #[test]
    fn run_vibe_success_writes_stdout_and_stderr_to_log() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_vibe(&f.worktree, "vibe task", &f.session_dir, &[], &envs, 300).unwrap();

        assert_eq!(result.exit_code, 0);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("stdout-marker-vibe"));
        assert!(log.contains("stderr-marker-vibe"));
    }

    #[test]
    fn vibe_usage_metadata_ignores_preexisting_cumulative_session() {
        let f = fixture();
        let vibe_home = f._tmp.path().join("vibe-home");
        let sessions = vibe_home.join("logs/session");
        let old = sessions.join("old-session");
        fs::create_dir_all(&old).unwrap();
        fs::write(
            old.join("meta.json"),
            format!(
                r#"{{"environment":{{"working_directory":"{}"}},"stats":{{"session_total_llm_tokens":999999}}}}"#,
                f.worktree.display()
            ),
        )
        .unwrap();
        let envs = vec![("VIBE_HOME".to_string(), vibe_home.display().to_string())];
        let sessions_before = snapshot_vibe_session_metadata_paths(&envs);
        let started_at = std::time::SystemTime::now() - Duration::from_secs(1);

        let current = sessions.join("this-attempt");
        fs::create_dir_all(&current).unwrap();
        fs::write(
            current.join("meta.json"),
            format!(
                r#"{{"environment":{{"working_directory":"{}"}},"stats":{{"session_total_llm_tokens":1200}}}}"#,
                f.worktree.display()
            ),
        )
        .unwrap();

        let selected =
            find_vibe_session_metadata(&envs, &f.worktree, started_at, &sessions_before).unwrap();
        assert_eq!(PathBuf::from(selected), current.join("meta.json"));
        let usage = crate::usage::parse_vibe_session_metadata(
            &fs::read_to_string(current.join("meta.json")).unwrap(),
        );
        assert_eq!(usage.total_tokens, Some(1200));

        let after = snapshot_vibe_session_metadata_paths(&envs);
        assert!(find_vibe_session_metadata(&envs, &f.worktree, started_at, &after).is_none());
    }

    #[test]
    fn run_vibe_nonzero_exit_preserved() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 1);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let result = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap();

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
            300,
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
    fn run_vibe_binds_effective_model_through_environment() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("VIBE_ACTIVE_MODEL".to_string(), "wrong-default".to_string()),
        ];

        run_vibe_with_executable(
            &f.bin_dir.join("vibe"),
            &f.worktree,
            "the vibe task",
            &f.session_dir,
            Some("devstral-small"),
            &[],
            &envs,
            300,
        )
        .unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("VIBE_ACTIVE_MODEL=devstral-small"));
        assert!(!env.contains("VIBE_ACTIVE_MODEL=wrong-default"));
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

        run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=vibe-env-value"));
    }

    #[test]
    fn run_vibe_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 300).unwrap_err();

        assert!(err.to_string().contains("launching vibe; is it installed"));
    }

    #[test]
    fn run_vibe_kills_process_after_idle_timeout_with_no_new_output() {
        // Live-observed (issue #154 dispatch, TICKET-154): a vibe attempt
        // hung for 15+ minutes with zero output and was only stopped by an
        // external watchdog script, not by gah itself -- same class of bug
        // as issues #87/#170. Pins the shared spawn_with_idle_watch fix.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "vibe",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 1).unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new backend output or worktree progress"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_vibe_allows_silent_backend_that_keeps_changing_worktree() {
        // Subscription CLIs can make tool calls without printing progress.
        // Repeated tracked-file edits must keep an otherwise silent process
        // alive, while the adjacent test proves a truly idle process dies.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        initialize_git_worktree(&f.worktree);
        make_fake_bin(
            &f.bin_dir,
            "vibe",
            "#!/bin/sh\nsleep 1\nprintf 'first\\n' > progress.txt\nsleep 1\nprintf 'second\\n' > progress.txt\nsleep 1\nexit 0\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_vibe(&f.worktree, "task", &f.session_dir, &[], &envs, 2).unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(
            result.duration_secs >= 3.0,
            "ran only {}s",
            result.duration_secs
        );
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(!log.contains("GAH: killed"), "got log: {log}");
        assert_eq!(
            fs::read_to_string(f.worktree.join("progress.txt")).unwrap(),
            "second\n"
        );
    }

    // ── run_opencode ─────────────────────────────────────────────────────

    #[test]
    fn run_opencode_core_argv_includes_run_dir_auto_and_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_opencode(
            &f.worktree,
            "the opencode task",
            &f.session_dir,
            Some("provider/test-model"),
            &["--format".to_string(), "json".to_string()],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "run");
        assert!(argv.contains(&"--dir".to_string()));
        assert!(argv.contains(&f.worktree.to_string_lossy().to_string()));
        assert!(argv.contains(&"--auto".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"provider/test-model".to_string()));
        assert!(argv.contains(&"--format".to_string()));
        assert!(argv.contains(&"json".to_string()));
        assert!(argv.contains(&"the opencode task".to_string()));
    }

    #[test]
    fn run_opencode_route_model_overrides_stale_profile_model_flags() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_opencode(
            &f.worktree,
            "the opencode task",
            &f.session_dir,
            Some("provider/test-model"),
            &[
                "--model".to_string(),
                "stale-model".to_string(),
                "--model=another-stale".to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "run");
        assert!(argv.contains(&"--dir".to_string()));
        assert!(argv.contains(&"--auto".to_string()));
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"provider/test-model".to_string()));
        assert!(argv.contains(&"--format".to_string()));
        assert!(argv.contains(&"json".to_string()));
        assert!(!argv.contains(&"stale-model".to_string()));
        assert!(!argv.contains(&"another-stale".to_string()));
        assert!(!argv.contains(&"--model=another-stale".to_string()));
    }

    #[test]
    fn run_opencode_without_model_still_includes_run_dir_auto() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        run_opencode(
            &f.worktree,
            "the opencode task",
            &f.session_dir,
            None,
            &[],
            &envs,
            300,
        )
        .unwrap();

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "run");
        assert!(argv.contains(&"--dir".to_string()));
        assert!(argv.contains(&f.worktree.to_string_lossy().to_string()));
        assert!(argv.contains(&"--auto".to_string()));
        assert!(argv.contains(&"the opencode task".to_string()));
        // No --model flag should be present when model is None
        assert!(!argv.contains(&"--model".to_string()));
    }

    #[test]
    fn run_opencode_propagates_env_file_vars() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "opencode", &f.record_dir, 0);
        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            (
                "FROM_ENV_FILE".to_string(),
                "opencode-env-value".to_string(),
            ),
        ];

        run_opencode(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap();

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=opencode-env-value"));
    }

    #[test]
    fn snapshot_opencode_session_scopes_metadata_to_worktree_and_start_time() {
        let f = fixture();
        let data_dir = f._tmp.path().join(".local/share/opencode");
        fs::create_dir_all(&data_dir).unwrap();
        let database = data_dir.join("opencode.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    directory TEXT NOT NULL,
                    model TEXT NOT NULL,
                    tokens_input INTEGER NOT NULL,
                    tokens_output INTEGER NOT NULL,
                    tokens_reasoning INTEGER NOT NULL,
                    tokens_cache_read INTEGER NOT NULL,
                    tokens_cache_write INTEGER NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL
                );
                CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
                CREATE TABLE part (
                    id TEXT PRIMARY KEY,
                    message_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    data TEXT NOT NULL
                );",
            )
            .unwrap();
        let started_at = std::time::SystemTime::now();
        let started_at_ms = started_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        connection
            .execute(
                "INSERT INTO session VALUES (?1, ?2, ?3, 775, 140, 20, 15360, 0, ?4, ?5)",
                rusqlite::params![
                    "session-current",
                    f.worktree.to_string_lossy(),
                    r#"{"id":"hy3-free","providerID":"opencode"}"#,
                    started_at_ms + 1,
                    started_at_ms + 2,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO message VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    "message-final",
                    "session-current",
                    started_at_ms + 3,
                    r#"{"role":"assistant","finish":"stop"}"#,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    "part-final",
                    "message-final",
                    "session-current",
                    started_at_ms + 4,
                    r#"{"type":"text","text":"Implemented the scoped fix."}"#,
                ],
            )
            .unwrap();
        let envs = vec![("HOME".to_string(), f._tmp.path().display().to_string())];

        let snapshot = snapshot_opencode_session(&envs, &f.worktree, started_at, &f.session_dir)
            .expect("current worktree session should be captured");
        let usage = crate::usage::parse_opencode_session_metadata(
            &fs::read_to_string(snapshot.path).unwrap(),
        );
        assert_eq!(usage.actual_model.as_deref(), Some("hy3-free"));
        assert_eq!(usage.input_tokens, Some(775));
        assert_eq!(usage.total_tokens, Some(16295));
        assert_eq!(
            snapshot.final_summary.as_deref(),
            Some("Implemented the scoped fix.")
        );
    }

    #[test]
    fn run_opencode_missing_binary_produces_useful_error() {
        let f = fixture();
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];

        let err =
            run_opencode(&f.worktree, "task", &f.session_dir, None, &[], &envs, 300).unwrap_err();

        assert!(err
            .to_string()
            .contains("launching opencode; is it installed"));
    }

    #[test]
    fn run_opencode_kills_process_after_idle_timeout_with_no_new_output() {
        // Issue #170: a live opencode dispatch hung for 3+ hours with zero
        // output and no supervision at all -- opencode previously used a
        // plain blocking `cmd.status()`. This pins the fix: it must be
        // killed once output has genuinely stopped, not allowed to run
        // forever.
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "opencode",
            "#!/bin/sh\necho 'step1'\nsleep 5\necho 'step2 should never appear'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_opencode_with_executable(
            &f.bin_dir.join("opencode"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            1, // idle timeout: 1s of silence is stalled
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("step1"));
        assert!(!log.contains("step2"));
        assert!(
            log.contains("killed after 1s with no new worktree progress"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_opencode_kills_chatty_backend_without_worktree_progress() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "opencode",
            "#!/bin/sh\nwhile true; do echo 'tool chatter with no successful edit'; sleep 1; done\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_opencode_with_executable(
            &f.bin_dir.join("opencode"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            1,
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("tool chatter with no successful edit"));
        assert!(
            log.contains("killed after 1s with no new worktree progress"),
            "got log: {log}"
        );
    }

    #[test]
    fn run_opencode_allows_real_worktree_progress_despite_chatty_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "opencode",
            "#!/bin/sh\necho 'starting edit'; sleep 1; printf 'first\\n' > progress.txt; echo 'editing'; sleep 1; printf 'second\\n' > progress.txt; echo 'done'\n",
        );
        let envs = vec![(
            "PATH".to_string(),
            format!(
                "{}:{}",
                f.bin_dir.to_str().unwrap(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )];

        let result = run_opencode_with_executable(
            &f.bin_dir.join("opencode"),
            &f.worktree,
            "task",
            &f.session_dir,
            None,
            &[],
            &envs,
            3,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            fs::read_to_string(f.worktree.join("progress.txt")).unwrap(),
            "second\n"
        );
    }

    #[test]
    fn run_opencode_captures_only_its_internal_log_delta() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        let data_home = f._tmp.path().join("xdg-data");
        let internal_log = data_home.join("opencode/log/opencode.log");
        fs::create_dir_all(internal_log.parent().unwrap()).unwrap();
        fs::write(
            &internal_log,
            "old run: AI_APICallError: Rate limit exceeded. Please try again later.\n",
        )
        .unwrap();
        make_fake_bin(
            &f.bin_dir,
            "opencode",
            "#!/bin/sh\nprintf '%s\\n' 'timestamp=now level=ERROR message=\"AI_APICallError: Rate limit exceeded. Please try again later.\"' >> \"$XDG_DATA_HOME/opencode/log/opencode.log\"\nexit 0\n",
        );
        let envs = vec![
            (
                "PATH".to_string(),
                format!(
                    "{}:{}",
                    f.bin_dir.to_str().unwrap(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            ),
            ("XDG_DATA_HOME".to_string(), data_home.display().to_string()),
        ];

        let result = run_opencode_with_executable(
            &f.bin_dir.join("opencode"),
            &f.worktree,
            "task",
            &f.session_dir,
            Some("opencode/hy3-free"),
            &[],
            &envs,
            5,
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.internal_log_path.as_deref(),
            Some(internal_log.to_str().unwrap())
        );
        let delta = result.internal_log_delta.as_deref().unwrap();
        assert!(delta.contains("Rate limit exceeded"));
        assert!(!delta.contains("old run"), "delta was {delta:?}");
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
            log.contains("killed after 1s with no new backend output or worktree progress"),
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

    #[test]
    fn run_review_backend_supports_vibe_without_model_flag() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "vibe",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("mistral-medium-3.5"),
            &[("FROM_ENV_FILE".into(), "vibe-review-env".into())],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        assert!(result.stdout.contains("stdout-marker-vibe"));
        assert!(result.stderr.contains("stderr-marker-vibe"));

        let argv = recorded_argv(&f.record_dir);
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"task".to_string()));
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"text".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        // Vibe does NOT accept --model flag
        assert!(!argv.contains(&"--model".to_string()));
        assert!(!argv.contains(&"mistral-medium-3.5".to_string()));

        let env = recorded_env(&f.record_dir);
        assert!(env.contains("FROM_ENV_FILE=vibe-review-env"));
        assert!(env.contains("VIBE_ACTIVE_MODEL=mistral-medium-3.5"));
    }

    #[test]
    fn run_review_backend_binds_claude_model() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "claude", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "claude",
            &f.worktree,
            "task",
            &f.session_dir,
            Some("haiku"),
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);
        let argv = recorded_argv(&f.record_dir);
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"haiku".to_string()));
    }

    #[test]
    fn run_review_backend_vibe_command_construction() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_recording_bin(&f.bin_dir, "vibe", &f.record_dir, 0);
        let profile = test_profile();
        let _guard = PathGuard::set(f.bin_dir.display().to_string());

        let result = run_review_backend(
            &profile,
            "vibe",
            &f.worktree,
            "Review this code: int main() { return 0; }",
            &f.session_dir,
            None,
            &[],
        );

        assert_eq!(result.outcome, ReviewProcessOutcome::Success);

        let argv = recorded_argv(&f.record_dir);
        // Verify correct command construction: vibe -p <prompt> --output text --trust --auto-approve
        assert_eq!(argv[0], "-p");
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"text".to_string()));
        assert!(argv.contains(&"--trust".to_string()));
        assert!(argv.contains(&"--auto-approve".to_string()));
        // Should NOT contain the invalid "review" subcommand
        assert!(!argv.contains(&"review".to_string()));
        // Should NOT contain --model flag
        assert!(!argv.contains(&"--model".to_string()));
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

    // ── TICKET-242: AGY version + log-format drift detection ───────────────

    #[test]
    fn detect_agy_version_parses_version_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = TempDir::new().unwrap();
        make_fake_bin(
            tmp.path(),
            "agy",
            "#!/bin/sh\necho 'antigravity-cli version 1.0.16'\n",
        );
        assert_eq!(
            detect_agy_version(&tmp.path().join("agy"), tmp.path(), &[]).as_deref(),
            Some("1.0.16")
        );
    }

    #[test]
    fn detect_agy_version_handles_bare_version_token() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = TempDir::new().unwrap();
        make_fake_bin(tmp.path(), "agy", "#!/bin/sh\necho 'v2.3.4'\n");
        assert_eq!(
            detect_agy_version(&tmp.path().join("agy"), tmp.path(), &[]).as_deref(),
            Some("2.3.4")
        );
    }

    #[test]
    fn detect_agy_version_returns_none_on_failure() {
        let tmp = TempDir::new().unwrap();
        make_fake_bin(
            tmp.path(),
            "agy",
            "#!/bin/sh\necho 'not a version'\nexit 1\n",
        );
        assert_eq!(
            detect_agy_version(&tmp.path().join("agy"), tmp.path(), &[]),
            None
        );
    }

    #[test]
    fn detect_agy_version_runs_inside_the_dispatch_worktree() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        let observed_cwd = tmp.path().join("observed-cwd");
        make_fake_bin(
            tmp.path(),
            "agy",
            &format!(
                "#!/bin/sh\npwd > '{}'\necho 'antigravity-cli version 1.0.16'\n",
                observed_cwd.display()
            ),
        );

        assert_eq!(
            detect_agy_version(&tmp.path().join("agy"), &worktree, &[]).as_deref(),
            Some("1.0.16")
        );
        assert_eq!(
            fs::read_to_string(observed_cwd).unwrap().trim(),
            worktree.display().to_string()
        );
    }

    #[test]
    fn is_agy_log_format_unrecognized_classifies_correctly() {
        // Empty delta is never "unrecognized".
        assert!(!is_agy_log_format_unrecognized(""));
        // A non-empty, unrecognized shape trips the canary.
        assert!(is_agy_log_format_unrecognized(
            "[NEW FORMAT] upstream reshaped this line; no known signature"
        ));
        // Every known signature keeps classification on the recognized path.
        assert!(!is_agy_log_format_unrecognized(
            "RESOURCE_EXHAUSTED: quota exhausted"
        ));
        assert!(!is_agy_log_format_unrecognized(
            "not logged into Antigravity"
        ));
        assert!(!is_agy_log_format_unrecognized("Resets in 15m30s"));
    }

    #[test]
    fn agy_log_path_table_resolves_by_version() {
        // Unknown version falls back to every known layout's candidate.
        let cands = agy_log_candidates(None);
        assert!(cands.contains(&"cli.log"));
        assert!(cands.contains(&"log"));

        // A pre-1.0.0 version predates the rotated `log/` layout.
        let cands_old = agy_log_candidates(Some("0.9.0"));
        assert!(cands_old.contains(&"cli.log"));
        assert!(!cands_old.contains(&"log"));

        // A current version includes both layouts.
        let cands_new = agy_log_candidates(Some("1.0.16"));
        assert!(cands_new.contains(&"cli.log"));
        assert!(cands_new.contains(&"log"));

        // Candidates must be tried NEWEST-first: a version satisfying
        // multiple thresholds must not resolve the oldest matching layout
        // ahead of a newer one that's also a valid match.
        let cli_log_pos = cands_new.iter().position(|c| *c == "cli.log").unwrap();
        let log_dir_pos = cands_new.iter().position(|c| *c == "log").unwrap();
        assert!(
            log_dir_pos < cli_log_pos,
            "newer `log/` layout must be tried before the older flat `cli.log`, got {cands_new:?}"
        );
    }

    #[test]
    fn agy_cli_log_path_prefers_rotated_log_over_stale_flat_file() {
        // A stale `cli.log` left over from a pre-upgrade install, coexisting
        // with a freshly-populated `log/` directory the upgraded CLI is
        // actually writing to -- the resolved path must be the one inside
        // `log/`, not the dead flat file.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join(".gemini/antigravity-cli");
        fs::create_dir_all(root.join("log")).unwrap();
        fs::write(root.join("cli.log"), "stale pre-upgrade content").unwrap();
        fs::write(root.join("log/cli-1.log"), "current rotated content").unwrap();

        let envs = vec![("HOME".to_string(), tmp.path().to_str().unwrap().to_string())];
        let resolved = agy_cli_log_path(&envs, Path::new("agy"), Some("1.0.16")).unwrap();
        assert_eq!(resolved, root.join("log/cli-1.log"));
    }

    #[test]
    fn run_agy_captures_cli_version_in_result() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "agy",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'antigravity-cli version 1.0.16'; else echo 'stdout-marker-agy'; fi\n",
        );
        let envs = vec![("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string())];
        let result = run_agy(
            &f.worktree,
            "test task",
            &f.session_dir,
            &test_llm(),
            &envs,
            "agy",
        )
        .unwrap();
        assert_eq!(result.agy_version.as_deref(), Some("1.0.16"));
    }

    #[test]
    fn run_agy_empty_output_with_unrecognized_log_emits_drift_note() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();

        // Fake AGY that reports a future version via `--version`, and on
        // `--print` exits 0 with no stdout but appends an *unrecognized* line
        // to its cli.log -- exactly the silent upstream log-format change
        // TICKET-242 defends against.
        let home = f._tmp.path().join("home");
        let cli_log = home.join(".gemini/antigravity-cli/cli.log");
        fs::create_dir_all(cli_log.parent().unwrap()).unwrap();
        fs::write(&cli_log, "").unwrap();
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'antigravity-cli version 2.0.0'; elif [ \"$1\" = \"--print\" ]; then printf '[NEW FORMAT] upstream reshaped this line; no known signature\\n' >> '{}'; exit 0; else exit 1; fi\n",
            cli_log.display(),
        );
        make_fake_bin(&f.bin_dir, "agy", &body);

        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("HOME".to_string(), home.to_str().unwrap().to_string()),
        ];

        let result = run_agy(
            &f.worktree,
            "test task",
            &f.session_dir,
            &test_llm(),
            &envs,
            "agy",
        )
        .unwrap();

        assert_eq!(result.exit_code, -1, "empty output must be a failure");
        assert_eq!(result.agy_version.as_deref(), Some("2.0.0"));

        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("agy_log_format_unrecognized"),
            "drift canary note must be present; log was:\n{log}"
        );
    }

    #[test]
    fn run_agy_empty_output_with_recognized_log_emits_no_drift_note() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();

        // Fake AGY that appends a *recognized* quota signature to its cli.log.
        let home = f._tmp.path().join("home");
        let cli_log = home.join(".gemini/antigravity-cli/cli.log");
        fs::create_dir_all(cli_log.parent().unwrap()).unwrap();
        fs::write(&cli_log, "").unwrap();
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'antigravity-cli version 1.0.16'; elif [ \"$1\" = \"--print\" ]; then printf 'RESOURCE_EXHAUSTED: quota exhausted\\n' >> '{}'; exit 0; else exit 1; fi\n",
            cli_log.display(),
        );
        make_fake_bin(&f.bin_dir, "agy", &body);

        let envs = vec![
            ("PATH".to_string(), f.bin_dir.to_str().unwrap().to_string()),
            ("HOME".to_string(), home.to_str().unwrap().to_string()),
        ];

        let result = run_agy(
            &f.worktree,
            "test task",
            &f.session_dir,
            &test_llm(),
            &envs,
            "agy",
        )
        .unwrap();

        assert_eq!(result.exit_code, -1);
        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(
            !log.contains("agy_log_format_unrecognized"),
            "recognized signature must not trip the drift canary; log was:\n{log}"
        );
        assert!(
            log.contains("AGY quota exhausted"),
            "the recognized quota signature is still classified normally; log was:\n{log}"
        );
    }
}
