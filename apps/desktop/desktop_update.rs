//! Desktop release updates (issue #1416). The Tauri updater plugin checks a
//! signed `latest.json` published on the edge release and installs the new
//! bundle in place, replacing today's "run `gah update` by hand" flow for the
//! desktop app itself. No source checkout, cargo, or npm is involved.
//!
//! The feed's minisign public key is provisioned at runtime (environment or
//! `gah/desktop-updater.json`), not baked into the build: the signing key
//! pair is a deployment secret, and a missing key simply reports
//! "updater not configured" instead of breaking app startup.

use serde::Serialize;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

pub const DEFAULT_ENDPOINT: &str =
    "https://github.com/Kh1ng/git-agent-harness/releases/download/edge/latest.json";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateStatus {
    pub available: bool,
    pub current_version: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub error: Option<String>,
}

/// The updater feed configuration: endpoint + minisign public key. The key
/// comes from GAH_DESKTOP_UPDATER_PUBKEY, or from
/// `$XDG_CONFIG_HOME/gah/desktop-updater.json`
/// (`{"endpoint": "...", "pubkey": "..."}`) so launchd/registry-managed
/// installs can provision it without rebuilding the app.
struct UpdaterConfig {
    endpoint: String,
    pub_key: String,
}

fn config_path() -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    config_home.join("gah").join("desktop-updater.json")
}

fn updater_config() -> Result<UpdaterConfig, String> {
    let explicit = std::env::var("GAH_DESKTOP_UPDATER_CONFIG").ok();
    let config_file = explicit
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(config_path);
    if config_file.is_file() {
        let text = std::fs::read_to_string(&config_file).map_err(|error| {
            format!(
                "cannot read the desktop updater config {}: {error}",
                config_file.display()
            )
        })?;
        let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
            format!(
                "invalid desktop updater config {}: {error}",
                config_file.display()
            )
        })?;
        let endpoint = parsed
            .get("endpoint")
            .and_then(|value| value.as_str())
            .unwrap_or(DEFAULT_ENDPOINT)
            .to_string();
        let pub_key = parsed
            .get("pubkey")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        if pub_key.is_empty() {
            return Err("updater not configured: no signing public key".to_string());
        }
        return Ok(UpdaterConfig { endpoint, pub_key });
    }
    let pub_key = std::env::var("GAH_DESKTOP_UPDATER_PUBKEY").unwrap_or_default();
    let endpoint = std::env::var("GAH_DESKTOP_UPDATER_ENDPOINT")
        .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    if pub_key.is_empty() {
        return Err("updater not configured: set GAH_DESKTOP_UPDATER_PUBKEY (or GAH_DESKTOP_UPDATER_CONFIG)".to_string());
    }
    Ok(UpdaterConfig { endpoint, pub_key })
}

async fn check(app: &AppHandle) -> Result<Option<tauri_plugin_updater::Update>, String> {
    let config = updater_config()?;
    let endpoint = config
        .endpoint
        .parse()
        .map_err(|error| format!("invalid updater endpoint {}: {error}", config.endpoint))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|error| format!("invalid updater endpoints: {error}"))?
        .pubkey(config.pub_key)
        .build()
        .map_err(|error| format!("cannot build the updater: {error}"))?;
    updater.check().await.map_err(|error| error.to_string())
}

/// Non-blocking status for the Settings row and the launch check: never
/// errors the caller, it reports `error` in the payload instead.
#[tauri::command]
pub async fn desktop_update_status(app: AppHandle) -> DesktopUpdateStatus {
    match check(&app).await {
        Ok(Some(update)) => DesktopUpdateStatus {
            available: true,
            current_version: update.current_version.clone(),
            version: Some(update.version.clone()),
            notes: update.body.clone(),
            error: None,
        },
        Ok(None) => {
            let current = app.package_info().version.to_string();
            DesktopUpdateStatus {
                available: false,
                current_version: current,
                version: None,
                notes: None,
                error: None,
            }
        }
        Err(error) => DesktopUpdateStatus {
            available: false,
            current_version: app.package_info().version.to_string(),
            version: None,
            notes: None,
            error: Some(error),
        },
    }
}

/// Download, verify, and install the pending update, then relaunch. The
/// plugin verifies the artifact's minisign signature against the configured
/// public key before writing anything.
#[tauri::command]
pub async fn desktop_apply_update(app: AppHandle) -> Result<(), String> {
    let update = check(&app)
        .await?
        .ok_or_else(|| "no update is available".to_string())?;
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|error| format!("the update download or install failed: {error}"))?;
    app.restart();
}
