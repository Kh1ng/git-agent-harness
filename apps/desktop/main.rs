#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
    presence: Presence,
}

/// Presence preferences always leave a way to return to the local controls.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Presence {
    dock: bool,
    launch_window: bool,
    tray: bool,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            dock: false,
            launch_window: true,
            tray: true,
        }
    }
}

impl Presence {
    fn can_hide(self) -> bool {
        self.tray || (cfg!(target_os = "macos") && self.dock)
    }

    // Without a persistent icon, opening the app must open a window.
    fn recoverable(mut self) -> Self {
        self.launch_window |= !self.can_hide();
        self
    }
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
        if let Ok(mut settings) = serde_json::from_str::<DesktopSettings>(&text) {
            settings.presence = settings.presence.recoverable();
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

fn write_settings(settings: &DesktopSettings) -> Result<(), String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("desktop.json"),
        serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Change native presence without navigating or hiding the operator's current window.
fn apply_presence(app: &tauri::AppHandle, presence: Presence) -> Result<(), String> {
    // Enable the replacement icon before removing the previous recovery surface.
    if presence.tray {
        if let Some(tray) = app.tray_by_id("gah-tray") {
            tray.set_visible(true).map_err(|e| e.to_string())?;
        }
    }
    #[cfg(target_os = "macos")]
    app.set_activation_policy(if presence.dock {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    })
    .map_err(|e| e.to_string())?;
    if !presence.tray {
        if let Some(tray) = app.tray_by_id("gah-tray") {
            tray.set_visible(false).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn save_presence(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    presence: Presence,
) -> Result<Presence, String> {
    local_only(&window)?;
    let mut settings = read_settings();
    settings.presence = presence.recoverable();
    write_settings(&settings)?;
    apply_presence(&app, settings.presence)
        .map_err(|e| format!("Preferences saved, but could not apply them. Restart GAH: {e}"))?;
    Ok(settings.presence)
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

// Only the bundled Settings document may operate this computer. The dashboard
// shares its window, but has no native command capability.
fn is_local_settings(url: &tauri::Url) -> bool {
    let origin = url.origin().ascii_serialization();
    let local = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || origin == "http://tauri.localhost"
        || origin == "https://tauri.localhost"
        || (cfg!(debug_assertions) && origin == "http://localhost:1420");
    local
        && matches!(url.path(), "/" | "/index.html")
        && url.username().is_empty()
        && url.password().is_none()
}

fn local_only(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "dashboard"
        || !is_local_settings(&window.url().map_err(|e| e.to_string())?)
    {
        return Err("This command is only available in this computer’s Settings page.".into());
    }
    Ok(())
}

#[tauri::command]
fn desktop_settings(window: tauri::WebviewWindow) -> Result<DesktopSettings, String> {
    local_only(&window)?;
    Ok(read_settings())
}

fn open_dashboard(app: &tauri::AppHandle, url: tauri::Url) -> Result<(), String> {
    let window = app
        .get_webview_window("dashboard")
        .ok_or("App window is unavailable")?;
    window.navigate(url).map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

#[tauri::command]
fn open_central_settings(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    local_only(&window)?;
    let mut url = central_url(&read_settings().central_url)?;
    url.set_query(Some("page=settings"));
    url.set_fragment(None);
    open_dashboard(&app, url)
}

// This navigation can only open local UI, never run a worker command.
fn can_open_settings(from: &tauri::Url, configured: &str) -> bool {
    central_url(configured).is_ok_and(|central| from.origin() == central.origin())
        && from
            .query_pairs()
            .find(|(key, _)| key == "page")
            .is_some_and(|(_, value)| value == "settings")
}

fn notification_payload(url: &tauri::Url) -> Option<(String, String)> {
    if url.scheme() != "gah" || url.host_str() != Some("notify") || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    let values: HashMap<_, _> = url.query_pairs().into_owned().collect();
    let id = values.get("id")?;
    let title = values.get("title")?;
    let body = values.get("body")?;
    if id.is_empty()
        || id.len() > 128
        || title.is_empty()
        || title.len() > 120
        || body.is_empty()
        || body.len() > 500
        || id
            .chars()
            .chain(title.chars())
            .chain(body.chars())
            .any(char::is_control)
    {
        return None;
    }
    Some((title.clone(), body.clone()))
}

fn can_bridge_notification(from: &tauri::Url, configured: &str) -> bool {
    central_url(configured).is_ok_and(|central| from.origin() == central.origin())
}

#[cfg(target_os = "macos")]
fn show_native_notification(title: &str, body: &str) {
    let escape = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape(body),
        escape(title)
    );
    let _ = command("osascript").args(["-e", &script]).status();
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
        wsl_distribution: settings.wsl_distribution,
        ..read_settings()
    };
    write_settings(&settings)?;
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

// Use the worker's installed environment for every WSL tool; before installation,
// report tools available to the login shell. Only the exit status leaves WSL.
#[cfg(any(windows, test))]
const WSL_TOOL_PROBE: &str = r#"
worker_env="${2:-$HOME/.local/share/gah/worker/worker.env}"
if [ -e "$worker_env" ]; then
    source "$worker_env" >/dev/null 2>&1 || exit 1
fi
command -v "$1" >/dev/null 2>&1
"#;

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
                    WSL_TOOL_PROBE,
                    "gah-tool-probe",
                    name,
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

fn show_settings(app: &tauri::AppHandle) {
    // Match WebviewUrl::App with the default (HTTP) protocol on Windows.
    // Do not cache window.url() during construction: WebView2 can still report about:blank.
    let url = if tauri::is_dev() {
        app.config().build.dev_url.clone()
    } else {
        Some(
            if cfg!(windows) {
                "http://tauri.localhost/"
            } else {
                "tauri://localhost/"
            }
            .parse()
            .unwrap(),
        )
    };
    if let Some(url) = url {
        let _ = open_dashboard(app, url);
    }
}

fn show_app(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("dashboard") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

// Every exit path, including the native application menu, reaps an owned worker.
#[cfg(not(windows))]
fn stop_owned_worker(app: &tauri::AppHandle) {
    if let Ok(mut process) = app.state::<WorkerState>().0.lock() {
        if let Some(child) = process.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn menu_event(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "connection" => show_settings(app),
        "dashboard" => {
            if central_url(&read_settings().central_url)
                .and_then(|url| open_dashboard(app, url))
                .is_err()
            {
                show_settings(app);
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

fn main() {
    tauri::Builder::default()
        .manage(WorkerState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            desktop_settings,
            save_presence,
            connect_dashboard,
            open_central_settings,
            worker_status,
            set_worker_running
        ])
        .setup(|app| {
            let settings = read_settings();
            let navigation_app = app.handle().clone();
            tauri::WebviewWindowBuilder::new(
                app,
                "dashboard",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("GAH")
            .inner_size(1400.0, 900.0)
            .min_inner_size(900.0, 600.0)
            // An inert UI marker; remote pages still have no IPC permissions.
            .initialization_script(
                if cfg!(target_os = "macos") {
                    "if (window === window.top) { window.__GAH_DESKTOP_SETTINGS__ = true; window.__GAH_DESKTOP_NATIVE_NOTIFICATIONS__ = true; }"
                } else {
                    "if (window === window.top) window.__GAH_DESKTOP_SETTINGS__ = true;"
                },
            )
            .on_navigation(move |url| {
                if url.as_str() == "gah://settings" || url.as_str() == "gah://settings/" {
                    let allowed = navigation_app
                        .get_webview_window("dashboard")
                        .and_then(|window| window.url().ok())
                        .is_some_and(|from| can_open_settings(&from, &read_settings().central_url));
                    if allowed {
                        let app = navigation_app.clone();
                        let _ = navigation_app.run_on_main_thread(move || show_settings(&app));
                    }
                    return false;
                }
                if let Some((title, body)) = notification_payload(url) {
                    let allowed = navigation_app
                        .get_webview_window("dashboard")
                        .and_then(|window| window.url().ok())
                        .is_some_and(|from| can_bridge_notification(&from, &read_settings().central_url));
                    #[cfg(target_os = "macos")]
                    if allowed {
                        show_native_notification(&title, &body);
                    }
                    #[cfg(not(target_os = "macos"))]
                    let _ = (allowed, title, body);
                    return false;
                }
                is_local_settings(url) || central_url(url.as_str()).is_ok()
            })
            .visible(settings.presence.launch_window)
            .center()
            .build()?;
            let connection = MenuItem::with_id(app, "connection", "Settings", true, None::<&str>)?;
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
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_app(tray.app_handle());
                    }
                })
                .build(app)?;
            #[cfg(target_os = "macos")]
            {
                let native_menu = Menu::default(app.handle())?;
                native_menu.append(&tauri::menu::Submenu::with_items(
                    app,
                    "GAH Controls",
                    true,
                    &[&connection, &dashboard],
                )?)?;
                app.set_menu(native_menu)?;
            }
            // A native Settings menu is also the offline recovery path when the
            // user has disabled the tray icon (including window-only Windows).
            #[cfg(not(target_os = "macos"))]
            app.set_menu(menu.clone())?;
            apply_presence(app.handle(), settings.presence)?;
            if settings.presence.launch_window && !settings.central_url.is_empty() {
                if let Ok(url) = central_url(&settings.central_url) {
                    let _ = open_dashboard(app.handle(), url);
                }
            }
            Ok(())
        })
        .on_menu_event(menu_event)
        .on_window_event(|window, event| {
            if window.label() == "dashboard" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if read_settings().presence.can_hide() {
                        let _ = window.hide();
                    } else {
                        window.app_handle().exit(0);
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error building GAH desktop")
        .run(|_app, _event| {
            #[cfg(not(windows))]
            if let tauri::RunEvent::Exit = _event {
                stop_owned_worker(_app);
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = _event
            {
                show_app(_app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn wsl_readiness_uses_worker_path_without_printing_credentials() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "gah-tool-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let tool = dir.join("gah-readiness-regression-tool");
        std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        let env = dir.join("worker.env");
        let probe = |name: &str| {
            let output = Command::new("bash")
                .args(["-lc", WSL_TOOL_PROBE, "gah-tool-probe", name])
                .arg(&env)
                .output()
                .unwrap();
            assert!(output.stdout.is_empty() && output.stderr.is_empty());
            output.status.success()
        };
        assert!(!probe("gah-readiness-regression-tool"));
        assert!(probe("git")); // A machine without a worker still reports login-shell tools.
        std::fs::write(
            &env,
            format!(
                "export COORDINATOR_TOKEN='private-test-token'\necho \"$COORDINATOR_TOKEN\"\nexport PATH='{}':\"$PATH\"\n",
                dir.to_string_lossy().replace('\'', "'\\''")
            ),
        )
        .unwrap();
        assert!(probe("gah-readiness-regression-tool"));
        assert!(!probe("gah-readiness-missing-tool"));
        std::fs::write(&env, "return 1\n").unwrap();
        assert!(!probe("git")); // Do not fall back when an installed environment is broken.
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn presence_defaults_migrate_and_every_combination_is_recoverable() {
        let legacy: DesktopSettings = serde_json::from_str(
            r#"{"central_url":"http://localhost:3773","wsl_distribution":"Ubuntu"}"#,
        )
        .unwrap();
        assert_eq!(legacy.presence, Presence::default());
        assert!(!legacy.presence.dock);
        assert!(legacy.presence.tray && legacy.presence.launch_window);
        for dock in [false, true] {
            for tray in [false, true] {
                for launch_window in [false, true] {
                    let requested = Presence {
                        dock,
                        tray,
                        launch_window,
                    };
                    let actual = requested.recoverable();
                    assert_eq!((actual.dock, actual.tray), (dock, tray));
                    assert_eq!(actual.launch_window, launch_window || !actual.can_hide());
                    assert_eq!(actual.recoverable(), actual);
                    let saved = serde_json::to_string(&actual).unwrap();
                    assert_eq!(serde_json::from_str::<Presence>(&saved).unwrap(), actual);
                }
            }
        }
    }

    #[test]
    fn local_controls_and_settings_navigation_have_separate_trust_boundaries() {
        for value in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "https://tauri.localhost/index.html",
        ] {
            assert!(is_local_settings(&value.parse().unwrap()), "{value}");
        }
        for value in [
            "https://gah.example/index.html",
            "tauri://attacker/index.html",
            "https://tauri.localhost/other",
            "file:///index.html",
            "https://tauri.localhost.evil/index.html",
        ] {
            assert!(!is_local_settings(&value.parse().unwrap()), "{value}");
        }
        let central = "https://gah.example";
        assert!(can_open_settings(
            &"https://gah.example/?page=settings".parse().unwrap(),
            central
        ));
        for value in [
            "https://evil.example/?page=settings",
            "http://gah.example/?page=settings",
            "https://gah.example:444/?page=settings",
            "https://gah.example/?page=chat",
            "https://gah.example/?page=chat&page=settings",
        ] {
            assert!(
                !can_open_settings(&value.parse().unwrap(), central),
                "{value}"
            );
        }
    }

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

    #[test]
    fn notification_bridge_accepts_only_bounded_payloads_from_the_configured_origin() {
        let from: tauri::Url = "https://gah.example.test/?page=activity".parse().unwrap();
        assert!(can_bridge_notification(&from, "https://gah.example.test"));
        assert!(!can_bridge_notification(
            &from,
            "https://other.example.test"
        ));
        let payload: tauri::Url =
            "gah://notify?id=event-1&title=Work%20finished&body=%23941%20passed"
                .parse()
                .unwrap();
        assert_eq!(
            notification_payload(&payload),
            Some(("Work finished".into(), "#941 passed".into()))
        );
        assert!(
            notification_payload(&"gah://notify?title=Missing&id=event-1".parse().unwrap())
                .is_none()
        );
        let oversized: tauri::Url = format!("gah://notify?id=x&title=Ok&body={}", "x".repeat(501))
            .parse()
            .unwrap();
        assert!(notification_payload(&oversized).is_none());
    }
}
