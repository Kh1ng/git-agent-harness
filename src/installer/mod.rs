//! `gah installer`: the file work the shell installers need, so they need
//! nothing beyond bash, curl, git, and the `gah` they install. Hidden from
//! help: these are the scripts' tools, not the operator's.
//!
//! - `files`: atomic private writes, environment files, shell quoting.
//! - `plist`: LaunchAgent property lists.
//! - `launchd`: the macOS agents `macos-launchd.sh install` loads.
//! - `wsl`: the Windows worker's files inside WSL.
//! - `import_docs`: seed a project's docs into shared memory at install.

pub mod files;
pub mod import_docs;
pub mod launchd;
pub mod plist;
pub mod wsl;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum Command {
    /// Set KEY in an environment file to the value on stdin (mode 0600).
    EnvSet {
        #[arg(long)]
        file: PathBuf,
        key: String,
    },
    /// Succeed only when the environment file sets KEY to a non-empty value.
    EnvHas {
        #[arg(long)]
        file: PathBuf,
        key: String,
    },
    /// Print a value from JSON on stdin or in --file. Exits 1 when absent.
    Json {
        #[arg(long)]
        file: Option<PathBuf>,
        /// JSON pointer, e.g. /Self/TailscaleIPs. Empty for the whole document.
        #[arg(long, default_value = "")]
        pointer: String,
        /// For an array: print this field of each element.
        #[arg(long)]
        each: Option<String>,
        /// For an array: print each element.
        #[arg(long)]
        lines: bool,
        /// Separator for --each and --lines output (default: newline).
        #[arg(long)]
        join: Option<String>,
    },
    /// Print an EnvironmentVariables value from a LaunchAgent plist.
    PlistGet {
        #[arg(long)]
        file: PathBuf,
        key: String,
    },
    /// Write the macOS LaunchAgents and their settings.
    Launchd(Box<launchd::Args>),
    /// Write the WSL worker's environment, scripts, and service.
    WslWorker(wsl::Args),
    /// Seed a project's memory and handoff docs into the memory gateway.
    ImportDocs(import_docs::Args),
}

fn home() -> PathBuf {
    crate::setup::host::home()
}

fn read_input(file: Option<&Path>) -> Result<String> {
    match file {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
        }
        None => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            Ok(text)
        }
    }
}

fn scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// The text `gah installer json` prints, or `None` when the value is absent.
pub fn json_output(
    document: &serde_json::Value,
    pointer: &str,
    each: Option<&str>,
    lines: bool,
    join: Option<&str>,
) -> Option<String> {
    let value = document.pointer(pointer)?;
    if each.is_none() && !lines {
        return scalar(value);
    }
    let items: Vec<String> = value
        .as_array()?
        .iter()
        .filter_map(|item| match each {
            Some(field) => item.get(field).and_then(scalar),
            None => scalar(item),
        })
        .collect();
    Some(items.join(join.unwrap_or("\n")))
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::EnvSet { file, key } => {
            let mut value = String::new();
            std::io::stdin().read_to_string(&mut value)?;
            files::env_set(&file, &key, &value)
        }
        Command::EnvHas { file, key } => {
            if !files::env_has(&file, &key) {
                bail!("{} does not set {key}", file.display());
            }
            Ok(())
        }
        Command::Json {
            file,
            pointer,
            each,
            lines,
            join,
        } => {
            let document: serde_json::Value = serde_json::from_str(&read_input(file.as_deref())?)
                .context("the input is not JSON")?;
            match json_output(&document, &pointer, each.as_deref(), lines, join.as_deref()) {
                Some(text) => {
                    println!("{text}");
                    Ok(())
                }
                None => std::process::exit(1),
            }
        }
        Command::PlistGet { file, key } => {
            let xml = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            match plist::environment_value(&xml, &key) {
                Some(value) => {
                    println!("{value}");
                    Ok(())
                }
                None => std::process::exit(1),
            }
        }
        Command::Launchd(args) => launchd::install(&args, &home()),
        Command::WslWorker(args) => wsl::install(&args, &home()),
        Command::ImportDocs(args) => import_docs::run(&args, &home()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_reads_scalars_arrays_and_fields() {
        let health =
            json!({"status": "degraded", "error": {"message": "unreachable"}, "port": 3774});
        assert_eq!(
            json_output(&health, "/status", None, false, None).as_deref(),
            Some("degraded")
        );
        assert_eq!(
            json_output(&health, "/error/message", None, false, None).as_deref(),
            Some("unreachable")
        );
        assert_eq!(
            json_output(&health, "/port", None, false, None).as_deref(),
            Some("3774")
        );
        assert_eq!(json_output(&health, "/state", None, false, None), None);
        let tailscale = json!({"Self": {"TailscaleIPs": ["100.64.0.42", "fd7a:115c:a1e0::1"]}});
        assert_eq!(
            json_output(&tailscale, "/Self/TailscaleIPs", None, true, None).as_deref(),
            Some("100.64.0.42\nfd7a:115c:a1e0::1")
        );
        let profiles = json!([{"name": "alpha"}, {"name": "beta"}, {"other": 1}]);
        assert_eq!(
            json_output(&profiles, "", Some("name"), false, Some(",")).as_deref(),
            Some("alpha,beta")
        );
    }
}
