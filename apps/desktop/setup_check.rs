//! First-run setup from the app (#1284): shows `gah setup --check --json`
//! and hands the work to a terminal (Terminal.app on macOS, the first
//! emulator found on Linux), where `gah setup` can ask questions and
//! sudo can ask for a password. The checklist logic lives in gah; the app
//! only displays it.

use serde::Serialize;

use super::{command, installed_gah, local_only, read_settings, write_settings};

/// The paste-line install. `gh auth token` lets it fetch while the
/// repository is private; without a GitHub login it fetches anonymously.
const BOOTSTRAP: &str = "t=\"$(gh auth token 2>/dev/null || true)\"; curl -fsSL ${t:+-H \"Authorization: Bearer $t\"} https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh | GITHUB_TOKEN=\"$t\" bash";

/// Standalone: a central node only this computer can reach, every setup
/// default accepted. bootstrap.sh turns GAH_NODE_ROLE/GAH_YES into
/// `gah setup` flags; install-linux.sh binds the dashboard to GAH_SERVER_HOST
/// on first install instead of the tailnet address.
const STANDALONE_ENV: &str = "GAH_NODE_ROLE=central GAH_YES=1 GAH_SERVER_HOST=127.0.0.1";

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

fn setup_command(standalone: bool) -> String {
    match (installed_gah(), standalone) {
        (Ok(gah), false) => format!("{} setup", shell_quote(&gah.to_string_lossy())),
        (Ok(gah), true) => format!(
            "{STANDALONE_ENV} {} setup --role central --yes",
            shell_quote(&gah.to_string_lossy())
        ),
        (Err(_), false) => BOOTSTRAP.to_string(),
        (Err(_), true) => BOOTSTRAP.replacen(" bash", &format!(" {STANDALONE_ENV} bash"), 1),
    }
}

/// The dashboard a standalone install serves on this computer.
fn standalone_url() -> String {
    // ponytail: gah-server.service hardcodes PORT=3773; launchd uses the desktop's port.
    let port = if cfg!(target_os = "macos") { read_settings().server_port } else { 3773 };
    format!("http://127.0.0.1:{port}")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// A string literal for AppleScript's `do script`.
#[cfg(any(target_os = "macos", test))]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[tauri::command]
pub async fn setup_check(window: tauri::WebviewWindow, role: Option<String>) -> Result<SetupCheck, String> {
    local_only(&window)?;
    let command_text = setup_command(false);
    let terminal = cfg!(unix);
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
/// Returns the dashboard address saved for a standalone setup, else "".
pub fn open_setup_terminal(window: tauri::WebviewWindow, standalone: Option<bool>) -> Result<String, String> {
    local_only(&window)?;
    let standalone = standalone.unwrap_or(false);
    let line = setup_command(standalone);
    if !open_terminal(&line) {
        return Err(format!("No terminal opened. Run this in a terminal: {line}"));
    }
    if !standalone {
        return Ok(String::new());
    }
    let mut settings = read_settings();
    settings.central_url = standalone_url();
    write_settings(&settings)?;
    Ok(settings.central_url)
}

#[cfg(target_os = "macos")]
fn open_terminal(line: &str) -> bool {
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script {}\nend tell",
        applescript_string(line)
    );
    command("osascript")
        .args(["-e", &script])
        .status()
        .is_ok_and(|status| status.success())
}

/// Terminal emulators and the arguments that precede `bash -lc SCRIPT`,
/// tried in order; a missing one fails to spawn and the next is tried.
#[cfg(all(unix, not(target_os = "macos")))]
const LINUX_TERMINALS: &[(&str, &[&str])] = &[
    ("x-terminal-emulator", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("ptyxis", &["--"]),
    ("konsole", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("foot", &[]),
    ("wezterm", &["start", "--"]),
    ("xterm", &["-e"]),
];

#[cfg(all(unix, not(target_os = "macos")))]
fn open_terminal(line: &str) -> bool {
    // Keep the window open so the user can read how setup ended.
    let script = format!("{line}; printf '\\nPress Enter to close. '; read -r _");
    let changes = std::env::var("APPDIR")
        .map(|appdir| {
            let vars = std::env::vars_os()
                .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)));
            host_env(vars, &appdir)
        })
        .unwrap_or_default();
    LINUX_TERMINALS.iter().any(|(program, prefix)| {
        let mut terminal = command(program);
        terminal.args(*prefix).args(["bash", "-lc", script.as_str()]);
        for (key, value) in &changes {
            match value {
                Some(value) => terminal.env(key, value),
                None => terminal.env_remove(key),
            };
        }
        terminal.spawn().is_ok()
    })
}

#[cfg(windows)]
fn open_terminal(_line: &str) -> bool {
    false
}

/// The AppImage runtime points GTK, GIO, XDG, and library paths into its
/// mount; a host terminal that inherits them can fail to start. Returns the
/// variables to rewrite (mount entries dropped) or remove (nothing left).
#[cfg(any(all(unix, not(target_os = "macos")), test))]
fn host_env(
    vars: impl IntoIterator<Item = (String, String)>,
    appdir: &str,
) -> Vec<(String, Option<String>)> {
    vars.into_iter()
        .filter(|(_, value)| !appdir.is_empty() && value.contains(appdir))
        .map(|(key, value)| {
            let kept: Vec<&str> = value
                .split(':')
                .filter(|part| !part.is_empty() && !part.contains(appdir))
                .collect();
            (key, (!kept.is_empty()).then(|| kept.join(":")))
        })
        .collect()
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
        let standalone = BOOTSTRAP.replacen(" bash", &format!(" {STANDALONE_ENV} bash"), 1);
        assert!(standalone.ends_with(&format!("GITHUB_TOKEN=\"$t\" {STANDALONE_ENV} bash")));
    }

    #[test]
    fn host_terminals_lose_only_appimage_paths() {
        let vars = [
            ("PATH", "/tmp/.mount_gah/usr/bin:/usr/bin:/bin"),
            ("GDK_PIXBUF_MODULE_FILE", "/tmp/.mount_gah/usr/lib/loaders.cache"),
            ("HOME", "/home/k"),
            ("DISPLAY", ":0"),
        ]
        .map(|(key, value)| (key.to_string(), value.to_string()));
        assert_eq!(
            host_env(vars.clone(), "/tmp/.mount_gah"),
            vec![
                ("PATH".to_string(), Some("/usr/bin:/bin".to_string())),
                ("GDK_PIXBUF_MODULE_FILE".to_string(), None),
            ]
        );
        assert!(host_env(vars, "").is_empty());
    }
}
