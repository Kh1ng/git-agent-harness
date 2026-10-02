//! First-run setup from the app (#1284): shows `gah setup --check --json`
//! and hands the work to Terminal, where `gah setup` can ask questions and
//! sudo can ask for a password. The checklist logic lives in gah; the app
//! only displays it.

use serde::Serialize;

use super::{command, installed_gah, local_only};

/// The paste-line install. `gh auth token` lets it fetch while the
/// repository is private; without a GitHub login it fetches anonymously.
const BOOTSTRAP: &str = "t=\"$(gh auth token 2>/dev/null || true)\"; curl -fsSL ${t:+-H \"Authorization: Bearer $t\"} https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh | GITHUB_TOKEN=\"$t\" bash";

#[derive(Serialize)]
pub struct SetupCheck {
    /// Whether gah is installed where the app expects it.
    installed: bool,
    /// `gah setup --check --json` output; null when gah is missing or the
    /// check failed (see `error`).
    report: Option<serde_json::Value>,
    error: Option<String>,
    /// What the Terminal button runs.
    command: String,
    /// Whether this platform can open Terminal for it.
    terminal: bool,
}

fn setup_command() -> String {
    match installed_gah() {
        Ok(gah) => format!("{} setup", shell_quote(&gah.to_string_lossy())),
        Err(_) => BOOTSTRAP.to_string(),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// A string literal for AppleScript's `do script`.
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[tauri::command]
pub async fn setup_check(window: tauri::WebviewWindow, role: Option<String>) -> Result<SetupCheck, String> {
    local_only(&window)?;
    let command_text = setup_command();
    let terminal = cfg!(target_os = "macos");
    let Ok(gah) = installed_gah() else {
        return Ok(SetupCheck { installed: false, report: None, error: None, command: command_text, terminal });
    };
    let role = match role.as_deref() {
        Some("worker") => "worker",
        _ => "central",
    };
    let output = command(&gah.to_string_lossy())
        .args(["setup", "--check", "--json", "--role", role])
        .output()
        .map_err(|error| format!("Cannot run gah: {error}"))?;
    let (report, error) = if output.status.success() {
        match serde_json::from_slice(&output.stdout) {
            Ok(report) => (Some(report), None),
            Err(_) => (None, Some("This gah is too old to report its setup; run gah update.".into())),
        }
    } else {
        (None, Some(String::from_utf8_lossy(&output.stderr).trim().to_string()))
    };
    Ok(SetupCheck { installed: true, report, error, command: command_text, terminal })
}

#[tauri::command]
pub fn open_setup_terminal(window: tauri::WebviewWindow) -> Result<(), String> {
    local_only(&window)?;
    if !cfg!(target_os = "macos") {
        return Err(format!("Open a terminal and run: {}", setup_command()));
    }
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script {}\nend tell",
        applescript_string(&setup_command())
    );
    let status = command("osascript")
        .args(["-e", &script])
        .status()
        .map_err(|error| format!("Cannot open Terminal: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Terminal did not open. Run this in a terminal: {}", setup_command()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_commands_survive_applescript_and_shell_quoting() {
        assert_eq!(applescript_string("a \"b\" \\c"), "\"a \\\"b\\\" \\\\c\"");
        assert_eq!(shell_quote("/Users/o'neil/.cargo/bin/gah"), "'/Users/o'\"'\"'neil/.cargo/bin/gah'");
        assert!(BOOTSTRAP.contains("scripts/bootstrap.sh | GITHUB_TOKEN="));
        assert!(!BOOTSTRAP.contains('\n'), "Terminal runs it as one line");
    }
}
