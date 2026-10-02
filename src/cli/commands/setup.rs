//! Install the bundled memory hook and merge opt-in tool configuration.
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

/// Machine-local, explicitly requested integrations.
#[derive(Subcommand)]
pub enum SetupCommands {
    /// Install shared memory hooks without replacing existing tool hooks.
    MemoryHooks {
        /// Tools to configure. Repeat this flag or use comma-separated names.
        #[arg(long, required = true, value_delimiter = ',', value_parser = ["claude", "codex", "hermes"])]
        tool: Vec<String>,
        /// Target home directory (defaults to the current user's home).
        #[arg(long)]
        home_dir: Option<PathBuf>,
        /// Python 3.10+ interpreter; Hermes setup uses its installed Python by default.
        #[arg(long)]
        python: Option<PathBuf>,
        /// Memory gateway address, without credentials. Omit to retain existing setup.
        #[arg(long)]
        gateway_url: Option<String>,
    },
}

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
    let gateway_url = gateway_url
        .or_else(|| std::env::var("TDAI_GATEWAY_URL").ok())
        .or_else(|| {
            let contents = std::fs::read(home.join(".config/gah/memory-hooks.json")).ok()?;
            let settings: serde_json::Value = serde_json::from_slice(&contents).ok()?;
            settings.get("gateway_url")?.as_str().map(str::to_owned)
        });
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
        "gateway_api_key": crate::memory_gateway::gateway_api_key(&home)?,
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
    if let Some(url) = gateway_url {
        use crate::memory_gateway::{CurlMemoryGatewayTransport, MemoryGatewayTransport};
        let key = crate::memory_gateway::gateway_api_key(&home)?;
        let (status, _) = CurlMemoryGatewayTransport.post(
            &format!("{}/recall", url.trim_end_matches('/')),
            r#"{"query":"gah setup auth check","session_key":"gah:setup-check"}"#,
            key.as_deref(),
            10,
        )?;
        if status != 200 {
            bail!(
                "Memory gateway check failed (HTTP {status}); fix TDAI_GATEWAY_API_KEY in {}",
                home.join(".config/gah/tdai-gateway.env").display()
            );
        }
        println!("Memory gateway authenticated recall check passed.");
    }
    Ok(())
}
