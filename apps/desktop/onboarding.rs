//! Local GUI onboarding. No terminal, password prompt, or shell supplied by the webview.
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use tauri::Emitter;

/// Use host tools rather than AppImage library mounts or the desktop launcher's minimal PATH.
pub(crate) fn host_command(program: &str) -> Command {
    let mut cmd = super::command(program);
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Ok(appdir) = std::env::var("APPDIR") {
        let vars = std::env::vars().collect::<Vec<_>>();
        for (key, value) in super::setup_check::host_env(vars, &appdir) {
            match value {
                Some(value) => {
                    cmd.env(key, value);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
    }
    #[cfg(unix)]
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        let mut paths = vec![home.join(".cargo/bin"), home.join(".local/bin")];
        let original = std::env::var_os("PATH").unwrap_or_default();
        let appdir = std::env::var("APPDIR").unwrap_or_default();
        paths.extend(
            std::env::split_paths(&original)
                .filter(|path| appdir.is_empty() || !path.starts_with(&appdir)),
        );
        if let Ok(path) = std::env::join_paths(paths) {
            cmd.env("PATH", path);
        }
    }
    cmd
}

pub use super::onboarding_choices::Choices;

#[tauri::command]
pub async fn onboarding_login(
    window: tauri::WebviewWindow,
    provider: String,
    token: String,
) -> Result<(), String> {
    super::local_only(&window)?;
    if !cfg!(target_os = "linux") {
        return Err("In-app repository onboarding currently requires Linux.".into());
    }
    let program = match provider.as_str() {
        "github" => "gh",
        "gitlab" => "glab",
        _ => return Err("Unsupported provider.".into()),
    };
    // Missing packages stop here, before credentials are sent anywhere.
    if !super::repository_tools::repository_tool_installed(program) {
        return Err(format!(
            "Install {program} using its official guide, then retry."
        ));
    }
    if token.trim().is_empty() {
        return Err("Enter a repository access token.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = host_command(program);
        if program == "gh" {
            cmd.args(["auth", "login", "--hostname", "github.com", "--with-token"]);
        } else {
            cmd.args(["auth", "login", "--hostname", "gitlab.com", "--stdin"]);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Cannot start repository login.")?;
        let written = child
            .stdin
            .take()
            .ok_or("Login input unavailable.")?
            .write_all(token.trim().as_bytes());
        let status = child.wait().map_err(|_| "Cannot check repository login.")?;
        written.map_err(|_| "Cannot send repository credentials.")?;
        if status.success() {
            Ok(())
        } else {
            Err("Repository login failed. Check the token permissions and retry.".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn onboarding_agent_login(
    window: tauri::WebviewWindow,
    agent: String,
) -> Result<(), String> {
    super::local_only(&window)?;
    if !cfg!(target_os = "linux") {
        return Err("In-app coding-agent onboarding currently requires Linux.".into());
    }
    let args: &[&str] = match agent.as_str() {
        "codex" => &["login", "--device-auth"],
        "claude" => &["auth", "login"],
        _ => return Err("Use Provider connections for this agent's API credentials.".into()),
    };
    let mut cmd = host_command(&agent);
    cmd.args(args);
    tauri::async_runtime::spawn_blocking(move || stream(cmd, &window))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn onboarding_install_cli(window: tauri::WebviewWindow) -> Result<(), String> {
    super::local_only(&window)?;
    if !cfg!(target_os = "linux") {
        return Err("In-app installation currently requires Linux.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        for program in ["git", "cargo"] {
            if !host_command(program)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
            {
                return Err(format!(
                    "Install {program} from its official guide, then retry. No build has started."
                ));
            }
        }
        std::fs::create_dir_all(super::config_dir()).map_err(|e| e.to_string())?;
        let source = super::config_dir().join("onboarding-source");
        if !source.exists() {
            let temporary = super::config_dir().join(format!(
                "onboarding-download-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos()
            ));
            let clone = if super::repository_tools::repository_tool_installed("gh") {
                let mut clone = host_command("gh");
                clone
                    .args(["repo", "clone", "Kh1ng/git-agent-harness"])
                    .arg(&temporary)
                    .args(["--", "--depth", "1"]);
                clone
            } else {
                let mut clone = host_command("git");
                clone
                    .args([
                        "clone",
                        "--depth",
                        "1",
                        "https://github.com/Kh1ng/git-agent-harness.git",
                    ])
                    .arg(&temporary);
                clone
            };
            if let Err(error) = stream(clone, &window) {
                let _ = std::fs::remove_dir_all(&temporary);
                return Err(error);
            }
            std::fs::rename(&temporary, &source).map_err(|e| e.to_string())?;
        }
        if !source.join("Cargo.toml").is_file() || !source.join("scripts/install.sh").is_file() {
            return Err(
                "The onboarding source checkout is incomplete. Restore or remove it, then retry."
                    .into(),
            );
        }
        let mut build = host_command("cargo");
        build
            .args(["install", "--locked", "--path"])
            .arg(&source)
            .args(["--bin", "gah", "--force"]);
        stream(build, &window)?;
        let mut settings = super::read_settings();
        settings.repository_path = source.to_string_lossy().into_owned();
        super::write_settings(&settings)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn stream(mut cmd: Command, window: &tauri::WebviewWindow) -> Result<(), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Setup diagnostics unavailable.")?;
    let output = child.stdout.take().ok_or("Setup progress unavailable.")?;
    let other = window.clone();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            let _ = other.emit("gah:onboarding-progress", line);
        }
    });
    for line in std::io::BufReader::new(output)
        .lines()
        .map_while(Result::ok)
    {
        let _ = window.emit("gah:onboarding-progress", line);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = reader.join();
    if status.success() {
        Ok(())
    } else {
        Err("Setup failed. Review the progress above, correct the failed step and retry.".into())
    }
}

#[tauri::command]
pub async fn onboarding_run(
    window: tauri::WebviewWindow,
    choices: Choices,
    gateway_key: String,
) -> Result<String, String> {
    super::local_only(&window)?;
    let args = choices.args()?;
    if !cfg!(target_os = "linux") {
        return Err("In-app service installation currently requires Linux; other platform validation is pending.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let gah = super::installed_gah()?;
        if super::read_settings().repository_path.is_empty() { return Err("Install the GAH CLI using the in-app button to prepare its source checkout, then retry.".into()); }
        // Never let --yes install packages or launch terminal authentication implicitly.
        let check = host_command(gah.to_string_lossy().as_ref()).args(&args).args(["--check", "--json"]).output().map_err(|e| e.to_string())?;
        if !check.status.success() { return Err("Prerequisite check failed. Select Check again and retry.".into()); }
        let report: serde_json::Value = serde_json::from_slice(&check.stdout).map_err(|_| "Prerequisite report unavailable; update GAH and retry.")?;
        if report["ready"] != true { return Err("Required prerequisites are unresolved. Complete the checklist and retry; setup has not started.".into()); }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = super::config_dir().join("gui-privilege");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
            // PolicyKit owns the native privilege dialog. The app never sees a sudo password.
            std::fs::write(dir.join("sudo"), "#!/bin/sh\nif [ \"$1\" = -n ]; then shift; exec /usr/bin/sudo -n \"$@\"; fi\nexec /usr/bin/pkexec \"$@\"\n").map_err(|e| e.to_string())?;
            std::fs::set_permissions(dir.join("sudo"), std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
            if !std::path::Path::new("/usr/bin/pkexec").is_file() { return Err("PolicyKit is unavailable. Install your platform's PolicyKit package and enable a desktop authentication agent, then retry.".into()); }
            let host = host_command("sh");
            let paths = host.get_envs().find(|(key, _)| *key == "PATH").and_then(|(_, value)| value).map(std::ffi::OsStr::to_os_string).unwrap_or_else(|| std::env::var_os("PATH").unwrap_or_default());
            let path = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&paths))).map_err(|e| e.to_string())?;
            let mut cmd = host_command(gah.to_string_lossy().as_ref());
            cmd.args(args).arg("--yes")
                .arg("--no-prerequisite-actions").arg("--source").arg(&super::read_settings().repository_path).env("PATH", path).env("GAH_GATEWAY_API_KEY", gateway_key).env("GAH_SERVER_HOST", "127.0.0.1");
            stream(cmd, &window)?;
        }
        let mut settings = super::read_settings();
        settings.central_url = "http://127.0.0.1:3773".into();
        super::write_settings(&settings)?;
        Ok(settings.central_url)
    }).await.map_err(|e| e.to_string())?
}
