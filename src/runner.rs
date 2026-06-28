use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

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
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    fs::write(session_dir.join("task.md"), task)?;

    let log_file = fs::File::create(&log_path).context("creating log file")?;
    let log_err = log_file.try_clone()?;

    let start = Instant::now();
    let status = Command::new("openhands")
        .args([
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
        .stderr(Stdio::from(log_err))
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
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    fs::write(session_dir.join("task.md"), task)?;

    let log_file = fs::File::create(&log_path).context("creating log file")?;
    let log_err = log_file.try_clone()?;

    let start = Instant::now();
    let status = Command::new("codex")
        .args(["exec", task])
        .args(extra_args)
        .current_dir(worktree)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
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
) -> Result<RunResult> {
    let log_path = session_dir.join("backend-output.log");
    fs::write(session_dir.join("task.md"), task)?;

    let log_file = fs::File::create(&log_path).context("creating log file")?;
    let log_err = log_file.try_clone()?;

    let start = Instant::now();
    let status = Command::new("claude")
        .args(["-p", task])
        .args(extra_args)
        .current_dir(worktree)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
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
