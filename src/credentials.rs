//! Node-local named credentials. Public results contain metadata only; callers
//! explicitly select a source before a secret enters a child process or request.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
pub(crate) mod quota;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    MistralDashboard,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialInfo {
    pub id: String,
    pub provider: String,
    pub kind: CredentialKind,
    pub account_label: String,
    pub env_var: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCredential {
    info: CredentialInfo,
    secret: String,
    #[serde(default)]
    revision: String,
}

/// Canonical provider vocabulary shared with execution's harness bindings.
pub fn canonical_provider(provider: &str) -> &str {
    match provider {
        "codex" => "openai",
        "claude" => "anthropic",
        "vibe" | "mistral-dashboard" => "mistral",
        "agy" | "agy-main" | "agy-second" => "antigravity",
        "gemini" => "google",
        "nous-portal" => "nous",
        "moonshot" => "kimi",
        "x-ai" => "xai",
        other => other,
    }
}

fn root() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".config/gah/credentials"))
        .context("credential home unavailable")
}

fn validate(info: &CredentialInfo) -> Result<()> {
    validate_id(&info.id)?;
    if info.provider.is_empty()
        || info.provider.len() > 64
        || looks_like_secret(&info.provider)
        || crate::redact::redact(&info.provider) != info.provider
        || !info.provider.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        bail!("unsupported credential provider");
    }
    if info.account_label.trim().is_empty()
        || info.account_label.len() > 128
        || info.account_label.chars().any(char::is_control)
        || crate::redact::redact(&info.account_label) != info.account_label
        || looks_like_secret(&info.account_label)
    {
        bail!("invalid credential account label");
    }
    match info.kind {
        CredentialKind::MistralDashboard
            if info.provider == "mistral" && info.env_var.is_none() => {}
        CredentialKind::MistralDashboard => {
            bail!("dashboard credentials require Mistral without an execution environment variable")
        }
        CredentialKind::ApiKey => {
            let name = info
                .env_var
                .as_deref()
                .context("API credentials require an environment variable")?;
            // Credential injection may never alter executable lookup, HOME,
            // loader settings or GAH policy/configuration.
            if name.len() > 80
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                || !name.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
                || !(name.ends_with("_API_KEY") || name.ends_with("_ACCESS_TOKEN"))
                || name.starts_with("GAH_")
                || name.starts_with("LD_")
                || name.starts_with("DYLD_")
            {
                bail!("invalid credential environment variable");
            }
        }
    }
    Ok(())
}

fn validate_value(info: &CredentialInfo, secret: &str) -> Result<()> {
    let limit = if info.kind == CredentialKind::MistralDashboard {
        32768
    } else {
        8192
    };
    if secret.is_empty()
        || secret.len() > limit
        || !secret.is_ascii()
        || secret.bytes().any(|byte| byte < 32 || byte == 127)
    {
        bail!("invalid credential value");
    }
    Ok(())
}

fn validate_metadata_value(info: &CredentialInfo, value: &str) -> Result<()> {
    if value.len() >= 4
        && [&info.id, &info.provider, &info.account_label]
            .iter()
            .any(|field| field.contains(value))
    {
        bail!("credential metadata must not contain its private value");
    }
    Ok(())
}

#[cfg(unix)]
fn private_directory(root: &Path, create: bool) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if create && !root.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .context("cannot create private credential directory")?;
    }
    let metadata =
        std::fs::symlink_metadata(root).context("private credential directory unavailable")?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        bail!("credential directory must be owner-only and not a symlink");
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_directory(_root: &Path, _create: bool) -> Result<()> {
    bail!("private named credential storage is unavailable on this platform")
}

#[cfg(unix)]
fn read_at(root: &Path, id: &str) -> Result<StoredCredential> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    validate_id(id)?;
    private_directory(root, false)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(format!("{id}.json")))
        .context("named credential unavailable")?;
    let metadata = file.metadata().context("named credential unavailable")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        bail!("named credential must be an owner-only regular file");
    }
    let mut bytes = Vec::new();
    file.take(131073)
        .read_to_end(&mut bytes)
        .context("named credential unavailable")?;
    if bytes.len() > 131072 {
        bail!("named credential exceeds size limit");
    }
    let stored: StoredCredential = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid named credential record"))?;
    validate(&stored.info)?;
    validate_value(&stored.info, &stored.secret)?;
    validate_metadata_value(&stored.info, &stored.secret)?;
    if stored.info.id != id {
        bail!("named credential identity mismatch");
    }
    Ok(stored)
}

#[cfg(not(unix))]
fn read_at(_root: &Path, _id: &str) -> Result<StoredCredential> {
    bail!("private named credential storage is unavailable on this platform")
}

#[cfg(unix)]
fn source_lock(root: &Path, id: &str) -> Result<std::fs::File> {
    use fs2::FileExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    validate_id(id)?;
    private_directory(root, true)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(format!(".{id}.lock")))
        .context("credential publication lock unavailable")?;
    let metadata = file
        .metadata()
        .context("credential publication lock unavailable")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        bail!("credential publication lock must be owner-only");
    }
    file.lock_exclusive()
        .context("credential publication lock unavailable")?;
    Ok(file)
}
#[cfg(not(unix))]
fn source_lock(_root: &Path, _id: &str) -> Result<std::fs::File> {
    bail!("private named credential storage is unavailable on this platform")
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || crate::redact::redact(id) != id
        || looks_like_secret(id)
    {
        bail!("invalid credential ID");
    }
    Ok(())
}

fn looks_like_secret(value: &str) -> bool {
    value
        .split_whitespace()
        .any(|word| word.len() >= 32 && word.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn default_env(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("OPENAI_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "google" => Some("GOOGLE_API_KEY"),
        "mistral" => Some("MISTRAL_API_KEY"),
        "nous" => Some("NOUS_API_KEY"),
        "xai" => Some("XAI_API_KEY"),
        "kimi" => Some("MOONSHOT_API_KEY"),
        "deepseek" => Some("DEEPSEEK_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "groq" => Some("GROQ_API_KEY"),
        "together" => Some("TOGETHER_API_KEY"),
        "cerebras" => Some("CEREBRAS_API_KEY"),
        _ => None,
    }
}

fn list_at(root: &Path) -> Result<Vec<CredentialInfo>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    private_directory(root, false)?;
    let mut records = Vec::new();
    for entry in std::fs::read_dir(root).context("credential list unavailable")? {
        let entry = entry.context("credential list unavailable")?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("invalid credential ID")?;
        records.push(read_at(root, id)?.info);
        if records.len() > 128 {
            bail!("too many named credentials");
        }
    }
    records.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(records)
}

#[cfg(test)]
fn save_at(root: &Path, info: CredentialInfo, secret: &str) -> Result<CredentialInfo> {
    save_with_quota_at(root, info, secret, None)
}

fn save_with_quota_at(
    root: &Path,
    mut info: CredentialInfo,
    secret: &str,
    quota_path: Option<&Path>,
) -> Result<CredentialInfo> {
    use std::io::Write;
    info.provider = canonical_provider(&info.provider).into();
    if info.kind == CredentialKind::ApiKey && info.env_var.is_none() {
        info.env_var = default_env(&info.provider).map(str::to_owned);
    }
    validate(&info)?;
    validate_value(&info, secret)?;
    validate_metadata_value(&info, secret)?;
    private_directory(root, true)?;
    let mut file =
        tempfile::NamedTempFile::new_in(root).context("cannot save private credential")?;
    // tempfile creates an owner-only regular file. Never follow the destination.
    serde_json::to_writer(
        &mut file,
        &StoredCredential {
            info: info.clone(),
            secret: secret.to_owned(),
            revision: uuid::Uuid::new_v4().to_string(),
        },
    )
    .map_err(|_| anyhow::anyhow!("cannot encode private credential"))?;
    file.flush().context("cannot save private credential")?;
    file.as_file()
        .sync_all()
        .context("cannot save private credential")?;
    let _guard = source_lock(root, &info.id)?;
    let destination = root.join(format!("{}.json", info.id));
    if std::fs::symlink_metadata(&destination).is_ok() {
        if let Some(path) = quota_path {
            quota::updated(&info, path)?;
        }
    }
    file.persist(destination)
        .map_err(|_| anyhow::anyhow!("cannot save private credential"))?;
    Ok(info)
}

pub fn list() -> Result<Vec<CredentialInfo>> {
    list_at(&root()?)
}
pub fn get(id: &str) -> Result<CredentialInfo> {
    Ok(read_at(&root()?, id)?.info)
}

/// A save-generation nonce invalidates cached children after rotation. This is
/// independent of key material and never identifies a billing pool.
pub fn revision(id: &str) -> Result<String> {
    Ok(read_at(&root()?, id)?.revision)
}
pub fn save(info: CredentialInfo, secret: &str) -> Result<CredentialInfo> {
    save_with_quota_at(
        &root()?,
        info,
        secret,
        Some(&crate::quota_store::store_path()),
    )
}
pub fn remove(id: &str) -> Result<()> {
    remove_at(&root()?, id, &crate::quota_store::store_path())
}
fn remove_at(root: &Path, id: &str, quota_path: &Path) -> Result<()> {
    let _guard = source_lock(root, id)?;
    let info = read_at(root, id)?.info;
    quota::removed(&info, quota_path)?;
    std::fs::remove_file(root.join(format!("{id}.json"))).context("named credential unavailable")
}

pub(crate) fn selected(id: &str) -> Result<(CredentialInfo, String)> {
    let stored = read_at(&root()?, id)?;
    Ok((stored.info, stored.secret))
}

/// Resolve a single explicitly bound API source. Dashboard cookies never enter
/// execution, and a provider mismatch cannot fall back to an ambient key.
pub fn execution_env(id: &str, expected_provider: &str) -> Result<Vec<(String, String)>> {
    let (info, secret) = match selected(id) {
        Ok(value) => value,
        Err(_) => bail!("named credential unavailable"),
    };
    execution_env_with(info, secret, expected_provider)
}

fn execution_env_with(
    info: CredentialInfo,
    secret: String,
    expected_provider: &str,
) -> Result<Vec<(String, String)>> {
    if info.kind != CredentialKind::ApiKey || info.provider != canonical_provider(expected_provider)
    {
        bail!("named credential does not match the execution provider");
    }
    Ok(vec![(
        info.env_var.context("credential environment unavailable")?,
        secret,
    )])
}
