//! Owner-selected dashboard session. No browser credential extraction or routing
//! account association: this is a read-only current-login billing observation.
mod login;
mod parse;
pub(crate) use login::sign_in;
#[cfg(test)]
mod tests;

use crate::quota_store::{self, QuotaObservationRecord};
use anyhow::{bail, Context, Result};
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use time::{format_description::well_known::Rfc3339, Date, OffsetDateTime, Time};

fn cookie_path() -> Option<PathBuf> {
    match std::env::var_os("MISTRAL_DASHBOARD_COOKIE_FILE") {
        Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
        Some(_) => None,
        None => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/gah/mistral-dashboard.cookie")),
    }
}

pub(crate) fn configured() -> bool {
    if !cfg!(unix) {
        return false;
    }
    match std::env::var_os("MISTRAL_DASHBOARD_COOKIE_FILE") {
        Some(path) => !path.is_empty(),
        None => cookie_path()
            .is_some_and(|path| std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0)),
    }
}

#[cfg(unix)]
fn cookie(path: &Path) -> Result<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("auth_required: private Mistral dashboard cookie file unavailable")?;
    let metadata = file.metadata()?;
    // Check the opened file, not a path that could change between stat and read.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        bail!("auth_required: Mistral dashboard cookie must be an owner-only regular file");
    }
    let mut bytes = Vec::new();
    file.take(32769).read_to_end(&mut bytes)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("invalid Mistral dashboard cookie"))?;
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty()
        || text.len() > 32768
        || !text.is_ascii()
        || text.bytes().any(|byte| byte < 32 || byte == 127)
    {
        bail!("auth_required: invalid or empty Mistral dashboard cookie file");
    }
    Ok(text.to_owned())
}

#[cfg(not(unix))]
fn cookie(_path: &Path) -> Result<String> {
    bail!("owner-only Mistral dashboard cookie files are supported on macOS and Linux");
}

fn request(cookie: &str, endpoint: &str, input: serde_json::Value) -> Result<Vec<u8>> {
    let json = input.to_string();
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("input", &json)
        .finish();
    let escaped = cookie.replace('\\', "\\\\").replace('"', "\\\"");
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--silent",
        "--max-time",
        "15",
        "--max-filesize",
        "4194304",
        "--write-out",
        "\n%{http_code}",
        "-K",
        "-",
    ]);
    crate::runner::process::arm_child_pdeathsig(&mut command);
    let config = format!("url = \"https://admin.mistral.ai/api/local-trpc/{endpoint}?{encoded}\"\nheader = \"Cookie: {escaped}\"\nheader = \"Accept: application/json\"\n");
    let output = crate::runner::process::run_bounded_with_input(
        command,
        std::time::Duration::from_secs(20),
        config.as_bytes(),
    )
    .context("Mistral dashboard request failed or timed out")?;
    if !output.status.success() || output.stdout.len() > 4194308 {
        bail!("Mistral dashboard request failed or timed out");
    }
    let split = output
        .stdout
        .iter()
        .rposition(|byte| *byte == b'\n')
        .context("invalid Mistral dashboard HTTP response")?;
    match &output.stdout[split + 1..] {
        b"200" => Ok(output.stdout[..split].to_vec()),
        b"401" | b"403" => bail!(
            "auth_required: Mistral dashboard session expired; replace the selected cookie file"
        ),
        _ => bail!("Mistral dashboard HTTP request failed"),
    }
}

pub fn refresh() -> Result<QuotaObservationRecord> {
    let cookie =
        cookie(&cookie_path().context("auth_required: configure MISTRAL_DASHBOARD_COOKIE_FILE")?)?;
    refresh_with(&cookie, OffsetDateTime::now_utc(), request)
}

/// Named dashboard sources are passed explicitly, without changing process env.
pub(crate) fn refresh_cookie(cookie: &str) -> Result<QuotaObservationRecord> {
    refresh_with(cookie, OffsetDateTime::now_utc(), request)
}

fn refresh_with(
    cookie: &str,
    now: OffsetDateTime,
    mut request: impl FnMut(&str, &str, serde_json::Value) -> Result<Vec<u8>>,
) -> Result<QuotaObservationRecord> {
    // SuperJSON Date is a JavaScript millisecond timestamp. Match its precision
    // before comparing the echoed metadata, including equivalent UTC offsets.
    let now = now.replace_nanosecond(now.nanosecond() / 1_000_000 * 1_000_000)?;
    let start = Date::from_calendar_date(now.year(), now.month(), 1)?
        .with_time(Time::MIDNIGHT)
        .assume_utc();
    let input = serde_json::json!({"json":{"start":start.format(&Rfc3339)?,"end":now.format(&Rfc3339)?},"meta":{"values":{"start":["Date"],"end":["Date"]},"v":1}});
    let costs = request(cookie, "usage.costBreakdown", input.clone())?;
    let mut request_input = input;
    request_input["json"]["chartMetric"] = serde_json::json!("apiCalls");
    let requests = request(cookie, "usage.breakdownByModel", request_input)?;
    let prices = request(
        cookie,
        "usage.prices",
        serde_json::json!({"json":{"workspaceId":null}}),
    )?;
    let budget = request(
        cookie,
        "billing.budget",
        serde_json::json!({"json":null,"meta":{"values":["undefined"],"v":1}}),
    )
    .ok();
    parse::parse(&costs, &requests, &prices, budget.as_deref(), start, now)
}

/// One cookie source per node. Failed refreshes invalidate the previously
/// observed customer without inventing another account or retaining its values.
pub fn refresh_and_store(path: &Path) -> Result<Option<QuotaObservationRecord>> {
    let record = refresh_scheduled(path)?;
    if let Some(error) = record
        .as_ref()
        .and_then(|record| record.check_error.as_deref())
    {
        bail!("{error}");
    }
    Ok(record)
}

/// Scheduler receives the persisted failed check, preventing a second generic marker.
pub(crate) fn refresh_scheduled(path: &Path) -> Result<Option<QuotaObservationRecord>> {
    refresh_and_store_with(path, refresh, OffsetDateTime::now_utc())
}

fn refresh_and_store_with(
    path: &Path,
    refresh: impl FnOnce() -> Result<QuotaObservationRecord>,
    now: OffsetDateTime,
) -> Result<Option<QuotaObservationRecord>> {
    let record = match refresh() {
        Ok(record) => record,
        Err(error) => {
            let mut previous = quota_store::load(path)?
                .into_iter()
                .rev()
                .find(|record| {
                    record.backend == "mistral-dashboard" && record.credential_id.is_none()
                })
                .unwrap_or(QuotaObservationRecord {
                    backend: "mistral-dashboard".into(),
                    backend_instance: Some("mistral-dashboard".into()),
                    model: None,
                    quota_pool: Some("mistral-dashboard".into()),
                    quota_window: None,
                    quota_remaining_percent: None,
                    quota_reset_at: None,
                    observed_at: None,
                    checked_at: None,
                    check_error: None,
                    usage_source: Some("mistral_dashboard".into()),
                    account_usage: None,
                    credential_id: None,
                });
            previous.quota_window = None;
            previous.quota_remaining_percent = None;
            previous.quota_reset_at = None;
            previous.account_usage = None;
            previous.observed_at = None;
            previous.checked_at = Some(now.format(&Rfc3339)?);
            previous.check_error = Some(crate::redact::redact(&error.to_string()));
            previous
        }
    };
    quota_store::append(path, &record)?;
    Ok(Some(record))
}
