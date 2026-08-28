use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Manager,
};

struct WorkerState {
    process: Mutex<Option<Child>>,
    profile: Mutex<String>,
    central_url: Mutex<String>,
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Parse KEY=VALUE lines from a shell env file, skipping comments and blanks.
fn load_env_file(path: &PathBuf) -> HashMap<String, String> {
    let Ok(f) = std::fs::File::open(path) else {
        return HashMap::new();
    };
    BufReader::new(f)
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| !l.trim_start().starts_with('#') && l.contains('='))
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Read registry_central_url from ~/.config/gah/config.toml.
/// Simple line scan — avoids pulling in a TOML dep.
fn read_central_url() -> String {
    let path = home_dir().join(".config/gah/config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return "http://100.118.97.79".to_string();
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("registry_central_url") {
            if let Some(val) = trimmed.split('=').nth(1) {
                return val.trim().trim_matches('"').to_string();
            }
        }
    }
    "http://100.118.97.79".to_string()
}

fn read_profile() -> String {
    // First profile found in config.toml [profiles.*] section.
    let path = home_dir().join(".config/gah/config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return "gah".to_string();
    };
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("[profiles.") && t.ends_with(']') {
            let name = t.trim_start_matches("[profiles.").trim_end_matches(']');
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    "gah".to_string()
}

fn is_worker_running(state: &WorkerState) -> bool {
    let mut guard = state.process.lock().unwrap();
    guard
        .as_mut()
        .map(|c| c.try_wait().map_or(true, |r| r.is_none()))
        .unwrap_or(false)
}

fn start_worker(state: &WorkerState) -> Result<(), String> {
    let mut guard = state.process.lock().unwrap();
    if guard
        .as_mut()
        .map(|c| c.try_wait().map_or(true, |r| r.is_none()))
        .unwrap_or(false)
    {
        return Err("already running".into());
    }

    let env_path = home_dir().join(".config/gah/gah-loop.env");
    let env_vars = load_env_file(&env_path);
    let profile = state.profile.lock().unwrap().clone();

    let gah = home_dir().join(".cargo/bin/gah");
    let mut cmd = Command::new(gah);
    cmd.args(["loop", "--profile", &profile])
        .envs(&env_vars)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = cmd.spawn().map_err(|e| e.to_string())?;
    *guard = Some(child);
    Ok(())
}

fn stop_worker(state: &WorkerState) {
    let mut guard = state.process.lock().unwrap();
    if let Some(child) = guard.as_mut() {
        let _ = child.kill();
    }
    *guard = None;
}

fn build_tray_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    running: bool,
    central_url: &str,
) -> tauri::Result<Menu<R>> {
    let status_label = if running { "● Worker running" } else { "○ Worker stopped" };
    let status = MenuItem::with_id(app, "status", status_label, false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Dashboard", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let start = MenuItem::with_id(
        app,
        "start",
        "Start Worker",
        !running,
        None::<&str>,
    )?;
    let stop = MenuItem::with_id(app, "stop", "Stop Worker", running, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let url_item = MenuItem::with_id(app, "url", &format!("Central: {central_url}"), false, None::<&str>)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    Menu::with_items(app, &[&status, &open, &sep1, &start, &stop, &sep2, &url_item, &sep3, &quit])
}

fn rebuild_tray<R: tauri::Runtime>(app: &tauri::AppHandle<R>, state: &Arc<WorkerState>) {
    let running = is_worker_running(state);
    let central_url = state.central_url.lock().unwrap().clone();
    if let Ok(menu) = build_tray_menu(app, running, &central_url) {
        if let Some(tray) = app.tray_by_id("gah-tray") {
            let _ = tray.set_menu(Some(menu));
            let title = if running { "GAH ●" } else { "GAH" };
            let _ = tray.set_title(Some(title));
        }
    }
}

fn main() {
    let central_url = read_central_url();
    let profile = read_profile();

    let worker_state = Arc::new(WorkerState {
        process: Mutex::new(None),
        profile: Mutex::new(profile),
        central_url: Mutex::new(central_url.clone()),
    });

    tauri::Builder::default()
        .setup(move |app| {
            // Keep app alive with no windows open
            let _ = app.handle().set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let state = worker_state.clone();
            let url = central_url.clone();

            let menu = build_tray_menu(&handle, false, &url)?;

            // The tray is built ONLY here. Declaring `app.trayIcon` in
            // tauri.conf.json as well creates a SECOND native menu-bar icon
            // with the same id (Tauri's manager map keeps one entry, the
            // other native icon leaks) — the "two GAH icons" bug.
            TrayIconBuilder::with_id("gah-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .icon_as_template(true)
                .title("GAH")
                .tooltip("GAH Worker")
                .show_menu_on_left_click(false)
                .menu(&menu)
                .on_menu_event({
                    let state = state.clone();
                    move |app, event| match event.id().as_ref() {
                        "open" => {
                            if let Some(win) = app.get_webview_window("main") {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                        "start" => {
                            let _ = start_worker(&state);
                            rebuild_tray(app, &state);
                        }
                        "stop" => {
                            stop_worker(&state);
                            rebuild_tray(app, &state);
                        }
                        "quit" => {
                            stop_worker(&state);
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(win) = tray.app_handle().get_webview_window("main") {
                            let visible = win.is_visible().unwrap_or(false);
                            if visible {
                                let _ = win.hide();
                            } else {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running GAH worker");
}
