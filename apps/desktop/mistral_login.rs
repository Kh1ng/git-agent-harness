//! A provider-owned sign-in window hands only its verified Mistral session to
//! this node's native quota collector. No remote page can invoke these commands.
use serde::Serialize;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{webview::Cookie, Emitter, Manager};

const WINDOW: &str = "mistral-login";
const URL: &str = "https://admin.mistral.ai/organization/usage";
const API_PATH: &str = "/api/local-trpc/";
const EVENT: &str = "gah:mistral-login";
static CHECKING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
pub struct LoginStatus {
    state: &'static str,
    installed: bool,
    message: &'static str,
}

fn status(state: &'static str, installed: bool, message: &'static str) -> LoginStatus {
    LoginStatus {
        state,
        installed,
        message,
    }
}

#[tauri::command]
pub async fn mistral_login_start(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<LoginStatus, String> {
    super::local_only(&window)?;
    if !cfg!(unix) {
        return Ok(status(
            "unavailable",
            false,
            "Mistral dashboard sign-in currently requires macOS or Linux.",
        ));
    }
    if super::installed_gah().is_err() {
        return Ok(status(
            "unavailable",
            false,
            "Install GAH on this computer before connecting Mistral.",
        ));
    }
    if !default_cookie_source(
        &super::config_dir(),
        std::env::var_os("MISTRAL_DASHBOARD_COOKIE_FILE").as_deref(),
    ) {
        return Ok(custom_cookie_status());
    }
    if let Some(login) = app.get_webview_window(WINDOW) {
        login.show().map_err(|_| "Cannot open Mistral sign-in.")?;
        login
            .set_focus()
            .map_err(|_| "Cannot focus Mistral sign-in.")?;
    } else {
        let builder = tauri::WebviewWindowBuilder::new(
            &app,
            WINDOW,
            tauri::WebviewUrl::External(URL.parse().expect("fixed URL")),
        )
        .title("Connect Mistral to GAH")
        .inner_size(1000.0, 760.0)
        .on_page_load(|login, payload| {
            if payload.event() != tauri::webview::PageLoadEvent::Finished
                || !authenticated_page(payload.url())
            {
                return;
            }
            // Native callback only: the remote provider has no IPC grant.
            tauri::async_runtime::spawn_blocking(move || {
                let reply = finish(&login);
                if let Some(settings) = login.app_handle().get_webview_window("dashboard") {
                    if super::local_only(&settings).is_ok() {
                        let _ = settings.emit(EVENT, reply);
                    }
                }
            });
        });
        #[cfg(target_os = "macos")]
        let builder = builder.data_store_identifier(*b"gah-mistral-auth");
        #[cfg(not(target_os = "macos"))]
        let builder = builder.data_directory(super::config_dir().join("mistral-webview"));
        builder
            .build()
            .map_err(|_| "Cannot create the Mistral sign-in window.")?;
    }
    Ok(status("pending",true,"Sign in to Mistral in the new window. GAH checks the connection when the usage page opens."))
}

#[tauri::command]
pub async fn mistral_login_finish(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<LoginStatus, String> {
    super::local_only(&window)?;
    let Some(login) = app.get_webview_window(WINDOW) else {
        return Ok(status(
            "cancelled",
            super::installed_gah().is_ok(),
            "The sign-in window is closed. Connect Mistral to try again.",
        ));
    };
    tauri::async_runtime::spawn_blocking(move || finish(&login))
        .await
        .map_err(|_| "Mistral connection check failed.".into())
}

fn authenticated_page(url: &tauri::Url) -> bool {
    url.origin().ascii_serialization() == "https://admin.mistral.ai"
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "/organization/usage" | "/organization/usage/")
}

struct Checking;
impl Drop for Checking {
    fn drop(&mut self) {
        CHECKING.store(false, Ordering::Release);
    }
}

fn finish(login: &tauri::WebviewWindow) -> LoginStatus {
    if CHECKING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return status("pending", true, "Checking the Mistral connection.");
    }
    let _checking = Checking;
    let Ok(gah) = super::installed_gah() else {
        return status(
            "unavailable",
            false,
            "Install GAH on this computer before connecting Mistral.",
        );
    };
    if !login.url().is_ok_and(|url| authenticated_page(&url)) {
        return status(
            "pending",
            true,
            "Finish signing in to Mistral, then check the connection.",
        );
    }
    // Wry's macOS cookies_for_url uses exact domain equality. Filter the
    // provider-owned store explicitly to include applicable parent domains.
    let cookies = match login.cookies() {
        Ok(cookies) => cookies,
        Err(_) => {
            return status(
                "unavailable",
                true,
                "Cannot read the Mistral sign-in session. Try connecting again.",
            )
        }
    };
    let cookie = match cookie_header(&cookies, unix_seconds()) {
        Ok(cookie) => cookie,
        Err(_) => {
            return status(
                "pending",
                true,
                "Finish signing in to Mistral, then check the connection.",
            )
        }
    };
    #[cfg(unix)]
    let reply = verify_and_save(&gah, &super::config_dir(), &cookie);
    #[cfg(not(unix))]
    let reply = {
        let _ = (gah, cookie);
        status(
            "unavailable",
            true,
            "Mistral dashboard sign-in currently requires macOS or Linux.",
        )
    };
    if reply.state == "connected" {
        let _ = login.close();
    }
    reply
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn default_cookie_source(root: &Path, configured: Option<&std::ffi::OsStr>) -> bool {
    configured.is_none_or(|path| Path::new(path) == root.join("mistral-dashboard.cookie"))
}

fn custom_cookie_status() -> LoginStatus {
    status("unavailable",true,"A custom Mistral Cookie file is configured. Clear MISTRAL_DASHBOARD_COOKIE_FILE before connecting with this app.")
}

fn matches_api_path(path: &str) -> bool {
    path == API_PATH
        || API_PATH.starts_with(path)
            && (path.ends_with('/') || API_PATH.as_bytes().get(path.len()) == Some(&b'/'))
}

fn cookie_header(cookies: &[Cookie<'_>], now: i64) -> Result<String, ()> {
    let mut selected = Vec::new();
    if cookies.len() > 512 {
        return Err(());
    }
    for cookie in cookies {
        let domain = cookie.domain().unwrap_or_default().trim_start_matches('.');
        if !matches!(domain, "admin.mistral.ai" | "mistral.ai")
            || !matches_api_path(cookie.path().unwrap_or("/"))
            || cookie
                .expires_datetime()
                .is_some_and(|expiry| expiry.unix_timestamp() <= now)
        {
            continue;
        }
        let name = cookie.name();
        let value = cookie.value();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
            || !value
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte) && !b"\";,\\".contains(&byte))
        {
            return Err(());
        }
        selected.push((
            cookie.path().unwrap_or("/").len(),
            format!("{name}={value}"),
        ));
    }
    selected.sort_by_key(|cookie| std::cmp::Reverse(cookie.0));
    let header = selected
        .into_iter()
        .map(|(_, cookie)| cookie)
        .collect::<Vec<_>>()
        .join("; ");
    if header.is_empty() || header.len() > 32768 {
        return Err(());
    }
    Ok(header)
}

#[cfg(unix)]
struct PrivateCheck {
    dir: PathBuf,
}
#[cfg(unix)]
impl PrivateCheck {
    fn new(root: &Path, cookie: &str) -> Result<Self, ()> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ())?
            .as_nanos();
        let dir = root.join(format!(".mistral-login-{}-{nonce}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|_| ())?;
        let check = Self { dir };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(check.dir.join("cookie"))
            .map_err(|_| ())?;
        file.write_all(cookie.as_bytes()).map_err(|_| ())?;
        file.sync_all().map_err(|_| ())?;
        Ok(check)
    }
}
#[cfg(unix)]
impl Drop for PrivateCheck {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(unix)]
fn verify_and_save(gah: &Path, root: &Path, cookie: &str) -> LoginStatus {
    if !default_cookie_source(
        root,
        std::env::var_os("MISTRAL_DASHBOARD_COOKIE_FILE").as_deref(),
    ) {
        return custom_cookie_status();
    }
    let Ok(check) = PrivateCheck::new(root, cookie) else {
        return status(
            "unavailable",
            true,
            "Cannot create a private Mistral connection check.",
        );
    };
    let store = check.dir.join("quota.jsonl");
    let mut command = super::command(gah.to_string_lossy().as_ref());
    command
        .args([
            "quota",
            "refresh",
            "--backend",
            "mistral-dashboard",
            "--store",
        ])
        .arg(&store)
        .env("MISTRAL_DASHBOARD_COOKIE_FILE", check.dir.join("cookie"));
    let succeeded = run_gah(command, Duration::from_secs(95));
    let observation = read_observation(&store);
    if !succeeded {
        return if observation
            .as_ref()
            .and_then(|record| record["check_error"].as_str())
            .is_some_and(|error| error.starts_with("auth_required:"))
        {
            status("pending",true,"Mistral has not accepted this session. Finish signing in and check the connection.")
        } else {
            status(
                "unavailable",
                true,
                "Mistral usage could not be verified. Check the connection again or update GAH.",
            )
        };
    }
    if !observation.is_some_and(|record| {
        record["backend"] == "mistral-dashboard"
            && record["account_usage"].is_object()
            && record["check_error"].is_null()
    }) {
        return status(
            "unavailable",
            true,
            "Update GAH on this computer to finish connecting Mistral.",
        );
    }
    if std::fs::rename(
        check.dir.join("cookie"),
        root.join("mistral-dashboard.cookie"),
    )
    .is_err()
    {
        return status(
            "unavailable",
            true,
            "The session was verified but could not be saved privately.",
        );
    }
    // Supported import publishes the already verified live observation without
    // making a second set of provider reads or copying credentials centrally.
    let mut command = super::command(gah.to_string_lossy().as_ref());
    command.args(["quota", "record"]);
    let Ok(input) = std::fs::File::open(&store) else {
        return status("unavailable",true,"Mistral is connected, but its usage could not be published. Check the connection again.");
    };
    command.stdin(Stdio::from(input));
    if !run_gah(command, Duration::from_secs(10)) {
        return status("unavailable",true,"Mistral is connected, but its usage could not be published. Check the connection again.");
    }
    status(
        "connected",
        true,
        "Mistral is connected on this computer. Usage and allowance are refreshed automatically.",
    )
}

#[cfg(unix)]
fn read_observation(path: &Path) -> Option<serde_json::Value> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(16 * 1024 + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() > 16 * 1024 {
        return None;
    }
    serde_json::from_str(&text).ok()
}

#[cfg(unix)]
fn run_gah(mut command: Command, timeout: Duration) -> bool {
    use std::os::unix::process::CommandExt;
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                // This process group belongs only to the verification CLI and
                // its curl children. No browser or worker process is stopped.
                unsafe extern "C" {
                    fn kill(pid: i32, signal: i32) -> i32;
                }
                unsafe {
                    kill(-(child.id() as i32), 9);
                }
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests;
