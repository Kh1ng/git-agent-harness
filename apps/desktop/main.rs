#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Manager,
};

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct DesktopSettings {
    central_url: String,
    wsl_distribution: String,
}

struct WorkerState(Mutex<Option<Child>>);

fn config_dir() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".config/gah")
}

fn read_settings() -> DesktopSettings {
    let dir = config_dir();
    if let Ok(text) = std::fs::read_to_string(dir.join("desktop.json")) {
        if let Ok(settings) = serde_json::from_str(&text) {
            return settings;
        }
    }
    let text = std::fs::read_to_string(dir.join("config.toml")).unwrap_or_default();
    let central_url = text
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "registry_central_url")
                .then(|| value.trim().trim_matches('"').to_owned())
        })
        .unwrap_or_default();
    DesktopSettings {
        central_url,
        ..Default::default()
    }
}

fn central_url(value: &str) -> Result<tauri::Url, String> {
    let url: tauri::Url = value
        .trim()
        .parse()
        .map_err(|_| "Enter a full http:// or https:// address.")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use an HTTP or HTTPS address without embedded credentials.".into());
    }
    Ok(url)
}

// The remotely hosted dashboard must never invoke local process controls.
fn local_only(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("This command is only available in the local connection window.".into());
    }
    Ok(())
}

#[tauri::command]
fn desktop_settings(window: tauri::WebviewWindow) -> Result<DesktopSettings, String> {
    local_only(&window)?;
    Ok(read_settings())
}

fn open_dashboard(app: &tauri::AppHandle, url: tauri::Url) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("dashboard") {
        window.navigate(url).map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        return window.set_focus().map_err(|e| e.to_string());
    }
    tauri::WebviewWindowBuilder::new(app, "dashboard", tauri::WebviewUrl::External(url))
        .title("GAH Dashboard")
        .inner_size(1400.0, 900.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .visible(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn connect_dashboard(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    settings: DesktopSettings,
) -> Result<(), String> {
    local_only(&window)?;
    let url = central_url(&settings.central_url)?;
    if settings.wsl_distribution.starts_with('-')
        || settings.wsl_distribution.chars().any(char::is_control)
    {
        return Err("Invalid WSL distribution name.".into());
    }
    let settings = DesktopSettings {
        central_url: url.to_string(),
        ..settings
    };
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("desktop.json"),
        serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    open_dashboard(&app, url)
}

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cmd.stdin(Stdio::null());
    cmd
}

#[cfg(windows)]
fn wsl_command(settings: &DesktopSettings) -> Command {
    let mut cmd = command("wsl.exe");
    if !settings.wsl_distribution.is_empty() {
        cmd.args(["--distribution", &settings.wsl_distribution]);
    }
    cmd
}

#[derive(serde::Serialize)]
struct ToolStatus {
    name: String,
    environment: String,
    installed: bool,
}

#[derive(serde::Serialize)]
struct WorkerStatus {
    running: bool,
    tools: Vec<ToolStatus>,
    note: String,
}

#[tauri::command]
async fn worker_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, WorkerState>,
) -> Result<WorkerStatus, String> {
    local_only(&window)?;
    let names = ["git", "gh", "glab", "claude", "codex", "opencode", "vibe"];
    let mut tools = Vec::new();
    for name in names {
        let installed = command(if cfg!(windows) { "where.exe" } else { "which" })
            .arg(name)
            .output()
            .is_ok_and(|out| out.status.success());
        tools.push(ToolStatus {
            name: name.into(),
            environment: if cfg!(windows) { "Windows" } else { "Local" }.into(),
            installed,
        });
    }
    #[cfg(windows)]
    {
        let settings = read_settings();
        for name in names {
            let installed = wsl_command(&settings)
                .args([
                    "--exec",
                    "bash",
                    "-lc",
                    &format!("command -v {name} >/dev/null"),
                ])
                .output()
                .is_ok_and(|out| out.status.success());
            tools.push(ToolStatus {
                name: name.into(),
                environment: "WSL".into(),
                installed,
            });
        }
        let running = wsl_command(&settings)
            .args([
                "--exec",
                "systemctl",
                "--user",
                "is-active",
                "--quiet",
                "gah-worker.service",
            ])
            .output()
            .is_ok_and(|out| out.status.success());
        let _ = state;
        Ok(WorkerStatus { running, tools, note: "The headless worker uses the selected WSL distribution. Windows tools are listed separately; installation does not verify login or worker compatibility. Quitting this app leaves the WSL service running.".into() })
    }
    #[cfg(not(windows))]
    {
        let mut process = state.0.lock().map_err(|e| e.to_string())?;
        let running = process
            .as_mut()
            .is_some_and(|child| child.try_wait().is_ok_and(|status| status.is_none()));
        Ok(WorkerStatus { running, tools, note: "Tool installation does not verify login. Use the dashboard readiness checks for the repository and backend you will run.".into() })
    }
}

#[tauri::command]
async fn set_worker_running(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, WorkerState>,
    running: bool,
) -> Result<(), String> {
    local_only(&window)?;
    #[cfg(windows)]
    {
        let _ = state;
        let out = wsl_command(&read_settings())
            .args([
                "--exec",
                "systemctl",
                "--user",
                if running { "start" } else { "stop" },
                "gah-worker.service",
            ])
            .output()
            .map_err(|e| {
                format!("Cannot launch WSL: {e}. Install the worker from Settings → Add a Node.")
            })?;
        if !out.status.success() {
            return Err(format!("WSL worker service could not be changed. Install it from Settings → Add a Node. {}", String::from_utf8_lossy(&out.stderr)));
        }
    }
    #[cfg(not(windows))]
    {
        let mut process = state.0.lock().map_err(|e| e.to_string())?;
        if !running {
            if let Some(child) = process.as_mut() {
                child.kill().map_err(|e| e.to_string())?;
                child.wait().map_err(|e| e.to_string())?;
            }
            *process = None;
        } else if !process
            .as_mut()
            .is_some_and(|child| child.try_wait().is_ok_and(|status| status.is_none()))
        {
            let dir = config_dir();
            let config = std::fs::read_to_string(dir.join("config.toml"))
                .map_err(|e| format!("Configure a worker profile first: {e}"))?;
            let profile = config
                .lines()
                .find_map(|line| line.trim().strip_prefix("[profiles.")?.strip_suffix(']'))
                .ok_or("Configure a worker profile first.")?;
            let env: HashMap<_, _> = std::fs::read_to_string(dir.join("gah-loop.env"))
                .unwrap_or_default()
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .filter_map(|line| {
                    line.split_once('=')
                        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                })
                .collect();
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("desktop-worker.log"))
                .map_err(|e| e.to_string())?;
            let gah = dir
                .parent()
                .and_then(|p| p.parent())
                .ok_or("Cannot find home directory")?
                .join(".cargo/bin/gah");
            *process = Some(
                command(gah.to_str().ok_or("Invalid worker path")?)
                    .args(["loop", "--profile", profile])
                    .envs(env)
                    .stderr(log.try_clone().map_err(|e| e.to_string())?)
                    .stdout(log)
                    .spawn()
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    Ok(())
}

fn show_connection(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .manage(WorkerState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            desktop_settings,
            connect_dashboard,
            worker_status,
            set_worker_running
        ])
        .setup(|app| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("GAH — Connection & Worker")
            .inner_size(760.0, 780.0)
            .min_inner_size(560.0, 500.0)
            // Keep the command-capable window local, including after a navigation attempt.
            .on_navigation(|url| {
                url.scheme() == "tauri"
                    || url.origin().ascii_serialization() == "http://tauri.localhost"
                    || url.origin().ascii_serialization() == "https://tauri.localhost"
                    || (cfg!(debug_assertions)
                        && url.origin().ascii_serialization() == "http://localhost:1420")
            })
            .visible(true)
            .center()
            .build()?;
            let connection =
                MenuItem::with_id(app, "connection", "Connection & Worker", true, None::<&str>)?;
            let dashboard =
                MenuItem::with_id(app, "dashboard", "Open Dashboard", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit GAH", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&connection, &dashboard, &quit])?;
            TrayIconBuilder::with_id("gah-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .icon_as_template(true)
                .tooltip("GAH")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "connection" => show_connection(app),
                    "dashboard" => {
                        if central_url(&read_settings().central_url)
                            .and_then(|url| open_dashboard(app, url))
                            .is_err()
                        {
                            show_connection(app);
                        }
                    }
                    "quit" => {
                        #[cfg(not(windows))]
                        if let Ok(mut process) = app.state::<WorkerState>().0.lock() {
                            if let Some(child) = process.as_mut() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_connection(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error running GAH desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_addresses_require_http_without_credentials() {
        for value in [
            "http://192.168.1.8:3773",
            "https://gah.example.test",
            "http://[::1]:3773",
        ] {
            assert!(central_url(value).is_ok(), "{value}");
        }
        for value in [
            "",
            "localhost",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:secret@gah.test",
        ] {
            assert!(central_url(value).is_err(), "{value}");
        }
    }
}
