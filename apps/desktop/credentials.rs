//! Only bundled local Settings can manage node-local provider credentials.
//! Secrets go to the installed CLI's stdin and never enter argv or a reply.
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub(crate) mod cli_args;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialInfo {
    pub id: String,
    pub provider: String,
    pub kind: String,
    pub account_label: String,
    pub env_var: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialInput {
    pub id: String,
    pub provider: String,
    pub kind: String,
    pub account_label: String,
    pub env_var: Option<String>,
    pub secret: String,
}

#[derive(Serialize)]
pub struct CredentialInstance {
    profile: String,
    instance: String,
    runner_kind: String,
    credential_id: Option<String>,
}

#[derive(Deserialize)]
struct InstanceCatalog {
    profiles: std::collections::BTreeMap<String, ProfileInstances>,
}

#[derive(Deserialize)]
struct ProfileInstances {
    backend_instances: Vec<ConfiguredInstance>,
}

#[derive(Deserialize)]
struct ConfiguredInstance {
    backend_instance: String,
    runner_kind: String,
    declared: bool,
    #[serde(default)]
    credential_id: Option<String>,
}

const FAILURE: &str =
    "Provider connection could not be updated. Check the fields and update GAH on this computer.";

#[tauri::command]
pub async fn credential_list(window: tauri::WebviewWindow) -> Result<Vec<CredentialInfo>, String> {
    super::local_only(&window)?;
    tauri::async_runtime::spawn_blocking(|| {
        let output = invoke(cli_args::list(), None, Duration::from_secs(10))?;
        serde_json::from_slice(&output).map_err(|_| FAILURE.to_owned())
    })
    .await
    .map_err(|_| FAILURE.to_owned())?
}

#[tauri::command]
pub async fn credential_save(
    window: tauri::WebviewWindow,
    input: CredentialInput,
) -> Result<CredentialInfo, String> {
    super::local_only(&window)?;
    tauri::async_runtime::spawn_blocking(move || save(input))
        .await
        .map_err(|_| FAILURE.to_owned())?
}

pub(crate) fn save(input: CredentialInput) -> Result<CredentialInfo, String> {
    save_with_gah(&super::installed_gah()?, input)
}

pub(crate) fn save_with_gah(
    gah: &std::path::Path,
    input: CredentialInput,
) -> Result<CredentialInfo, String> {
    if input.secret.is_empty() || input.secret.len() > 32768 {
        return Err("Enter a provider key or session of at most 32 KB.".to_owned());
    }
    let output = invoke_with_gah(
        gah,
        cli_args::save(
            &input.id,
            &input.provider,
            &input.kind,
            &input.account_label,
            input.env_var.as_deref(),
        ),
        Some(input.secret.as_bytes()),
        Duration::from_secs(10),
    )?;
    serde_json::from_slice(&output).map_err(|_| FAILURE.to_owned())
}

#[tauri::command]
pub async fn credential_remove(window: tauri::WebviewWindow, id: String) -> Result<(), String> {
    super::local_only(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        invoke(cli_args::remove(&id), None, Duration::from_secs(10)).map(|_| ())
    })
    .await
    .map_err(|_| FAILURE.to_owned())?
}

#[tauri::command]
pub async fn credential_refresh(window: tauri::WebviewWindow, id: String) -> Result<(), String> {
    super::local_only(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        invoke(cli_args::refresh(&id), None, Duration::from_secs(95)).map(|_| ())
    })
    .await
    .map_err(|_| "Provider usage check failed.".to_owned())?
}

#[tauri::command]
pub async fn credential_instances(
    window: tauri::WebviewWindow,
) -> Result<Vec<CredentialInstance>, String> {
    super::local_only(&window)?;
    tauri::async_runtime::spawn_blocking(|| {
        let output = invoke(cli_args::instances(), None, Duration::from_secs(10))?;
        let catalog: InstanceCatalog =
            serde_json::from_slice(&output).map_err(|_| FAILURE.to_owned())?;
        Ok(catalog
            .profiles
            .into_iter()
            .flat_map(|(profile, data)| {
                data.backend_instances
                    .into_iter()
                    .filter(|instance| instance.declared)
                    .map(move |instance| CredentialInstance {
                        profile: profile.clone(),
                        instance: instance.backend_instance,
                        runner_kind: instance.runner_kind,
                        credential_id: instance.credential_id,
                    })
            })
            .collect())
    })
    .await
    .map_err(|_| FAILURE.to_owned())?
}

#[tauri::command]
pub async fn credential_bind(
    window: tauri::WebviewWindow,
    profile: String,
    instance: String,
    credential_id: String,
) -> Result<(), String> {
    super::local_only(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        invoke(
            cli_args::bind(&profile, &instance, &credential_id),
            None,
            Duration::from_secs(10),
        )
        .map(|_| ())
    })
    .await
    .map_err(|_| FAILURE.to_owned())?
}

#[tauri::command]
pub async fn credential_add_instance(
    window: tauri::WebviewWindow,
    profile: String,
    instance: String,
    runner_kind: String,
    credential_id: String,
) -> Result<(), String> {
    super::local_only(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let output = invoke(cli_args::list(), None, Duration::from_secs(10))?;
        let connections: Vec<CredentialInfo> =
            serde_json::from_slice(&output).map_err(|_| FAILURE.to_owned())?;
        let connection = connections
            .into_iter()
            .find(|info| info.id == credential_id && info.kind == "api_key")
            .ok_or_else(|| FAILURE.to_owned())?;
        invoke(
            cli_args::add_instance(
                &profile,
                &instance,
                &runner_kind,
                &credential_id,
                &connection.account_label,
            ),
            None,
            Duration::from_secs(10),
        )
        .map(|_| ())
    })
    .await
    .map_err(|_| FAILURE.to_owned())?
}

fn invoke(args: Vec<String>, input: Option<&[u8]>, timeout: Duration) -> Result<Vec<u8>, String> {
    let gah = super::installed_gah()?;
    invoke_with_gah(&gah, args, input, timeout)
}

fn invoke_with_gah(
    gah: &std::path::Path,
    args: Vec<String>,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut command = super::command(gah.to_string_lossy().as_ref());
    command.args(args);
    run(command, input, timeout).map_err(|_| FAILURE.to_owned())
}

#[cfg(not(unix))]
fn run(
    _command: std::process::Command,
    _input: Option<&[u8]>,
    _timeout: Duration,
) -> Result<Vec<u8>, ()> {
    Err(())
}

#[cfg(unix)]
fn anonymous_file(root: &std::path::Path) -> Result<std::fs::File, ()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ())?
        .as_nanos();
    let path = root.join(format!(
        ".credential-io-{}-{now}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| ())?;
    std::fs::remove_file(path).map_err(|_| ())?;
    Ok(file)
}

#[cfg(unix)]
fn run(
    command: std::process::Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Vec<u8>, ()> {
    run_at(&super::config_dir(), command, input, timeout)
}

#[cfg(unix)]
fn run_at(
    root: &std::path::Path,
    mut command: std::process::Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Vec<u8>, ()> {
    use std::io::{Read, Seek, Write};
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::Instant;
    // A fresh installation has not saved settings or created this directory yet.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        .map_err(|_| ())?;
    let mut output = anonymous_file(root)?;
    let stdout = output.try_clone().map_err(|_| ())?;
    if let Some(input) = input {
        let mut stdin = anonymous_file(root)?;
        stdin.write_all(input).map_err(|_| ())?;
        stdin.rewind().map_err(|_| ())?;
        command.stdin(Stdio::from(stdin));
    } else {
        command.stdin(Stdio::null());
    }
    command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn().map_err(|_| ())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Err(()),
            Ok(None)
                if Instant::now() < deadline
                    && output.metadata().is_ok_and(|m| m.len() <= 262144) =>
            {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                unsafe extern "C" {
                    fn kill(pid: i32, signal: i32) -> i32;
                }
                unsafe {
                    kill(-(child.id() as i32), 9);
                }
                let _ = child.wait();
                return Err(());
            }
        }
    }
    output.rewind().map_err(|_| ())?;
    let mut bytes = Vec::new();
    output
        .take(262145)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > 262144 {
        return Err(());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn first_launch_initializes_private_io_without_persistent_secrets() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "gah-credential-first-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!root.exists());
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            "read -r value; test \"$value\" = 'test-only-secret'; printf 'metadata'",
        ]);
        let output = run_at(
            &root,
            command,
            Some(b"test-only-secret\n"),
            Duration::from_secs(2),
        );
        assert_eq!(output.unwrap(), b"metadata");
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stdin_secret_is_not_in_arguments_or_response() {
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            "read -r value; test \"$value\" = 'test-only-secret'; printf 'metadata' ",
        ]);
        assert_eq!(
            run(command, Some(b"test-only-secret\n"), Duration::from_secs(2)).unwrap(),
            b"metadata"
        );
        assert!(
            cli_args::save("account-one", "mistral", "api_key", "Personal", None)
                .iter()
                .all(|arg| !arg.contains("test-only-secret"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn timeout_stops_collector_process_group() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 20"]);
        let started = std::time::Instant::now();
        assert!(run(command, None, Duration::from_millis(40)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn metadata_contract_rejects_secret_field() {
        assert!(serde_json::from_value::<CredentialInfo>(serde_json::json!({
            "id":"one", "provider":"nous", "kind":"api_key", "account_label":"Personal", "env_var":"NOUS_API_KEY", "secret":"test-only-secret"
        })).is_err());
    }
}
