use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

pub struct RunResult {
    pub exit_code: i32,
    pub duration_secs: f64,
    pub log_path: String,
}

pub fn run_openhands(
    worktree: &Path,
    task: &str,
    session_dir: &Path,
    cloud: bool,
    llm_base_url: &str,
    llm_api_key: &str,
    llm_model: &str,
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
        ])
        .env("OPENHANDS_SUPPRESS_BANNER", "1")
        .env("LLM_BASE_URL", llm_base_url)
        .env("LLM_API_KEY", llm_api_key)
        .env("LLM_MODEL", llm_model)
        .current_dir(worktree)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .status()
        .context("launching openhands; is it installed?")?;

    Ok(RunResult {
        exit_code: status.code().unwrap_or(-1),
        duration_secs: start.elapsed().as_secs_f64(),
        log_path: log_path.to_string_lossy().into_owned(),
    })
}

pub fn backend_available(name: &str) -> bool {
    let cmd = match name {
        "openhands",
        "codex" => "codex",
        "claude" => "claude",
        _ => return true, // internal backends (ponytail, auto) are always "available"
    };
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
