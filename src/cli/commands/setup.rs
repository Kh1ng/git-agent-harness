//! Install the bundled memory hook and merge opt-in tool configuration.
use crate::cli::args::SetupCommands;
use anyhow::{bail, Context, Result};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

pub fn run(command: SetupCommands) -> Result<()> {
    let SetupCommands::MemoryHooks {
        tool,
        home_dir,
        python,
        gateway_url,
    } = command;
    let home = home_dir
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .context("Cannot find your home directory; pass --home-dir")?;
    let hermes_python = home.join(".hermes/hermes-agent/venv/bin/python");
    let python = python.unwrap_or_else(|| {
        if tool.iter().any(|t| t == "hermes") && hermes_python.is_file() {
            hermes_python
        } else {
            PathBuf::from("python3")
        }
    });
    let input = serde_json::to_vec(&serde_json::json!({
        "home": home, "tools": tool, "gateway_url": gateway_url,
        "hook_source": include_str!("../../../scripts/gah-memory-hook.py"),
    }))?;
    let mut child = Command::new(&python)
        .args([
            "-c",
            include_str!("../../../scripts/install-memory-hooks.py"),
        ])
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "Cannot start {}. Memory hooks require Python 3.10+; select it with --python",
                python.display()
            )
        })?;
    let written = child
        .stdin
        .take()
        .context("memory-hook setup input unavailable")?
        .write_all(&input);
    let status = child.wait().context("waiting for memory-hook setup")?;
    written.context("sending memory-hook setup input")?;
    if !status.success() {
        bail!("Memory-hook setup failed; see the diagnostic above");
    }
    Ok(())
}
