use tauri::Manager;
use std::process::{Command, Child};
use std::path::PathBuf;
use std::sync::{Mutex, Arc};
use std::time::Duration;

struct ServerState {
    server_process: Mutex<Option<Child>>,
}

fn main() {
    // Initialize server state
    let server_state = Arc::new(ServerState {
        server_process: Mutex::new(None),
    });

    // Get the path to the server executable
    let server_path = get_server_path();

    // Start the server before building the app
    if let Some(server_path) = server_path {
        start_server(&server_state, &server_path);
        // Wait a bit for the server to start
        std::thread::sleep(Duration::from_secs(2));
    }

    tauri::Builder::default()
        .setup(move |app| {
            // Store server state in the app
            app.manage(server_state.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_server_command,
            stop_server_command,
            is_server_running_command
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn get_server_path() -> Option<PathBuf> {
    // Try to find the server executable relative to the current executable
    let current_exe = std::env::current_exe().ok()?;
    let current_dir = current_exe.parent()?;
    
    // Look for server in the parent directory structure
    // During development: desktop app is in apps/desktop, server is in apps/server
    // During build: the executable might be in target/debug or target/release
    let mut path = current_dir.to_path_buf();
    
    // Try to find the project root (where apps/ directory is)
    for _ in 0..6 {
        let apps_dir = path.join("apps");
        if apps_dir.exists() && apps_dir.is_dir() {
            let server_bin = apps_dir.join("server/dist/bin.js");
            if server_bin.exists() {
                return Some(server_bin);
            }
        }
        if !path.pop() {
            break;
        }
    }
    
    // Fallback: try common development paths relative to current executable
    let dev_paths = [
        "../../../apps/server/dist/bin.js",
        "../../apps/server/dist/bin.js", 
        "../apps/server/dist/bin.js",
        "apps/server/dist/bin.js",
        "../server/dist/bin.js",
        "server/dist/bin.js",
    ];
    
    for dev_path in dev_paths {
        let full_path = current_dir.join(dev_path);
        if full_path.exists() {
            return Some(full_path);
        }
    }
    
    // Last resort: try absolute paths from the current directory
    let abs_paths = [
        PathBuf::from("apps/server/dist/bin.js"),
        PathBuf::from("../apps/server/dist/bin.js"),
        PathBuf::from("../../apps/server/dist/bin.js"),
    ];
    
    for abs_path in abs_paths {
        if abs_path.exists() {
            return Some(abs_path);
        }
    }
    
    None
}

fn start_server(server_state: &Arc<ServerState>, server_path: &PathBuf) {
    let mut state = server_state.server_process.lock().unwrap();
    
    if state.is_some() {
        // Server is already running
        return;
    }
    
    let server_path_str = server_path.to_string_lossy();
    
    match Command::new("node")
        .arg(&*server_path_str)
        .spawn() {
        Ok(child) => {
            *state = Some(child);
            println!("Server started from: {}", server_path_str);
        }
        Err(e) => {
            eprintln!("Failed to start server: {}", e);
        }
    }
}

#[tauri::command]
fn start_server_command(state: tauri::State<'_, Arc<ServerState>>) -> Result<bool, String> {
    let server_path = get_server_path()
        .ok_or("Could not find server executable")?;

    start_server(&state, &server_path);
    Ok(true)
}

#[tauri::command]
fn stop_server_command(state: tauri::State<'_, Arc<ServerState>>) -> Result<bool, String> {
    let mut server_guard = state.server_process.lock().unwrap();

    if let Some(child) = server_guard.as_mut() {
        child.kill().map_err(|e| e.to_string())?;
        *server_guard = None;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
fn is_server_running_command(state: tauri::State<'_, Arc<ServerState>>) -> Result<bool, String> {
    let mut server_guard = state.server_process.lock().unwrap();
    Ok(server_guard.as_mut().map_or(false, |child| {
        child.try_wait().map_or(true, |result| result.is_none())
    }))
}