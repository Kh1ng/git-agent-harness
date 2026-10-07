//! Nous Portal subscription balances use the account endpoint, not OpenCode telemetry.
use crate::quota_store::QuotaObservationRecord;
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub fn refresh() -> Result<QuotaObservationRecord> {
    refresh_with_sources(
        std::env::var("NOUS_API_KEY")
            .ok()
            .filter(|key| !key.is_empty()),
        native_access_token,
        fetch_account,
        OffsetDateTime::now_utc(),
    )
}

/// Explicit named keys never fall back to native Hermes or another account.
pub(crate) fn refresh_key(key: &str) -> Result<QuotaObservationRecord> {
    if key.is_empty() || key.len() > 8192 || key.chars().any(char::is_control) {
        bail!("invalid Nous credential");
    }
    parse_named(&fetch_account(key)?, OffsetDateTime::now_utc())
}

fn parse_named(input: &[u8], now: OffsetDateTime) -> Result<QuotaObservationRecord> {
    use sha2::{Digest, Sha256};
    let mut record = parse(input, now)?;
    record.backend_instance = None;
    record.quota_pool = None;
    let body: serde_json::Value =
        serde_json::from_slice(input).context("invalid Nous account response")?;
    if let Some(id) = body["organisation"]["id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
    {
        if body["paid_service_access"]["organisation_id"]
            .as_str()
            .is_some_and(|other| other != id)
        {
            bail!("Nous account billing identity mismatch");
        }
        let pool = format!("nous:{:x}", Sha256::digest(id.as_bytes()));
        record.backend_instance = Some(format!("opencode:{pool}"));
        record.quota_pool = Some(pool);
    }
    Ok(record)
}

fn refresh_with_sources(
    explicit_key: Option<String>,
    native_key: impl FnOnce() -> Result<String>,
    request: impl FnOnce(&str) -> Result<Vec<u8>>,
    now: OffsetDateTime,
) -> Result<QuotaObservationRecord> {
    let key = match explicit_key {
        Some(key) => key,
        None => native_key()?,
    };
    if key.is_empty() || key.len() > 8192 || key.chars().any(char::is_control) {
        bail!("invalid Nous credential");
    }
    parse(&request(&key)?, now)
}

/// Native OAuth remains in Hermes's selected auth home. GAH never copies its
/// refresh token or resets the inference credential pool's exhaustion status.
fn native_auth_path() -> Option<PathBuf> {
    match std::env::var_os("HERMES_HOME") {
        Some(home) if !home.is_empty() => Some(PathBuf::from(home).join("auth.json")),
        Some(_) => None,
        None => std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| PathBuf::from(home).join(".hermes/auth.json")),
    }
}

fn has_native_auth(path: &Path) -> bool {
    let read = || -> Option<serde_json::Value> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(65537)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 65536 {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    };
    read().is_some_and(|state| {
        ["access_token", "agent_key", "refresh_token"]
            .iter()
            .any(|field| {
                state["providers"]["nous"][field]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            })
    })
}

pub(crate) fn configured() -> bool {
    std::env::var("NOUS_API_KEY").is_ok_and(|key| !key.is_empty())
        || native_auth_path().is_some_and(|path| has_native_auth(&path))
}

// Fixed adapter to Hermes's refresh-aware Portal API. The token's own JWT expiry
// avoids rereading auth.json after releasing Hermes's cross-process auth lock.
const HERMES_PORTAL_TOKEN: &str = r#"
import base64, contextlib, json, sys
with contextlib.redirect_stdout(sys.stderr):
    from hermes_cli.auth import resolve_nous_access_token
    token = resolve_nous_access_token(timeout_seconds=15)
claims = json.loads(base64.urlsafe_b64decode(token.split('.')[1] + '==='))
print(json.dumps({'token': token, 'expires_at': claims['exp']}))
"#;

fn native_access_token() -> Result<String> {
    let path = native_auth_path()
        .filter(|path| has_native_auth(path))
        .context("auth_required: configure NOUS_API_KEY or sign in to Nous through Hermes")?;
    // Installation is independent of HERMES_HOME, which can select a different
    // profile's auth store. Never fall back to another auth home.
    let mut installations = Vec::new();
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        installations.push(PathBuf::from(home).join(".hermes/hermes-agent/venv/bin/python"));
    }
    installations.push(PathBuf::from("/usr/local/lib/hermes-agent/venv/bin/python"));
    let python = installations
        .into_iter()
        .find(|python| python.is_file())
        .context("auth_required: supported Hermes Python installation unavailable")?;
    let mut command = Command::new(python);
    command.args(["-I", "-c", HERMES_PORTAL_TOKEN]);
    command.env(
        "HERMES_HOME",
        path.parent().context("Hermes auth home unavailable")?,
    );
    native_token_from_command(command, std::time::Duration::from_secs(20))
}

fn native_token_from_command(mut command: Command, timeout: std::time::Duration) -> Result<String> {
    crate::runner::process::arm_child_pdeathsig(&mut command);
    let output = crate::runner::process::run_bounded(command, timeout)
        .context("auth_required: Hermes Nous renewal failed or timed out")?;
    if !output.status.success() || output.stdout.len() > 16384 {
        bail!("auth_required: Hermes Nous renewal failed; check native sign-in");
    }
    native_token_from_output(&output.stdout, OffsetDateTime::now_utc())
}

fn native_token_from_output(output: &[u8], now: OffsetDateTime) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Credential {
        token: String,
        expires_at: i64,
    }
    let credential: Credential = serde_json::from_slice(output)
        .map_err(|_| anyhow::anyhow!("invalid Hermes Nous credential response"))?;
    if credential.token.is_empty()
        || credential.token.len() > 8192
        || credential.token.chars().any(char::is_control)
        || credential.expires_at <= now.unix_timestamp() + 20
    {
        bail!("auth_required: Hermes returned an invalid or expiring Nous credential");
    }
    Ok(credential.token)
}

fn fetch_account(key: &str) -> Result<Vec<u8>> {
    let escaped = key.replace('\\', "\\\\").replace('"', "\\\"");
    let mut command = Command::new("curl");
    command
        .args([
            "--disable",
            "--silent",
            "--fail",
            "--max-time",
            "15",
            "--max-filesize",
            "524288",
            "-K",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::runner::process::arm_child_pdeathsig(&mut command);
    let mut child = command.spawn().context("Nous quota curl unavailable")?;
    child.stdin.take().context("Nous quota stdin unavailable")?.write_all(format!("url = \"https://portal.nousresearch.com/api/oauth/account\"\nheader = \"Authorization: Bearer {escaped}\"\nheader = \"Accept: application/json\"\n").as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("Nous account usage request failed (check credential access)");
    }
    Ok(output.stdout)
}

/// Only a real subscription cap supplies a denominator. Purchased/rollover credits
/// are separate funds and cannot be converted into a monthly percentage.
pub(crate) fn parse(input: &[u8], now: OffsetDateTime) -> Result<QuotaObservationRecord> {
    let body: serde_json::Value =
        serde_json::from_slice(input).context("invalid Nous account response")?;
    let subscription = &body["subscription"];
    let cap = subscription["monthly_credits"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0);
    let remaining = subscription["credits_remaining"]
        .as_f64()
        .filter(|v| v.is_finite() && *v >= 0.0);
    let remaining_percent = match (cap, remaining) {
        (Some(cap), Some(remaining)) if remaining <= cap => Some(remaining / cap * 100.0),
        _ => None,
    };
    let timestamp = now.format(&Rfc3339)?;
    let reset = subscription["current_period_end"]
        .as_str()
        .filter(|s| OffsetDateTime::parse(s, &Rfc3339).is_ok())
        .map(str::to_string);
    Ok(QuotaObservationRecord {
        backend: "opencode".into(),
        backend_instance: Some("opencode:nous-portal-api".into()),
        model: None,
        quota_pool: Some("nous-portal-api".into()),
        quota_window: remaining_percent.map(|_| "subscription-monthly".into()),
        quota_remaining_percent: remaining_percent,
        quota_reset_at: reset,
        observed_at: Some(timestamp.clone()),
        checked_at: Some(timestamp),
        check_error: None,
        usage_source: Some("nous_portal_account".into()),
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_keys_share_only_the_verified_organisation_billing_pool() {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let source = br#"{"organisation":{"id":"synthetic-org"},"user":{"email":"first@example.invalid"},"paid_service_access":{"organisation_id":"synthetic-org"},"subscription":{"monthly_credits":20,"credits_remaining":10}}"#;
        let first = parse_named(source, now).unwrap();
        let other_key = br#"{"organisation":{"id":"synthetic-org"},"user":{"email":"second@example.invalid"},"subscription":{"monthly_credits":20,"credits_remaining":10}}"#;
        let second = parse_named(other_key, now).unwrap();
        assert_eq!(first.quota_pool, second.quota_pool);
        assert!(first.quota_pool.as_deref().unwrap().starts_with("nous:"));
        assert!(!first
            .quota_pool
            .as_deref()
            .unwrap()
            .contains("synthetic-org"));
        let unknown = parse_named(
            br#"{"subscription":{"monthly_credits":20,"credits_remaining":10}}"#,
            now,
        )
        .unwrap();
        assert_eq!(unknown.quota_pool, None);
        assert!(parse_named(br#"{"organisation":{"id":"first"},"paid_service_access":{"organisation_id":"second"}}"#, now).is_err());
    }
    #[test]
    fn expired_native_hourly_token_is_not_reused_after_resolution_failure() {
        let now = OffsetDateTime::from_unix_timestamp(3600).unwrap();
        assert!(native_token_from_output(
            br#"{"token":"old-hourly-token","expires_at":3600}"#,
            now
        )
        .is_err());
        let record = refresh_with_sources(
            None,
            || {
                native_token_from_output(
                    br#"{"token":"renewed-hourly-token","expires_at":7200}"#,
                    now,
                )
            },
            |token| {
                assert_eq!(token, "renewed-hourly-token");
                Ok(br#"{"subscription":{"monthly_credits":22,"credits_remaining":11}}"#.to_vec())
            },
            now,
        )
        .unwrap();
        assert_eq!(record.quota_remaining_percent, Some(50.0));
        let result = refresh_with_sources(
            None,
            || bail!("native renewal unavailable"),
            |_| panic!("failed renewal must not make an account request"),
            now,
        );
        assert!(result.is_err());
    }

    #[test]
    fn explicit_keys_keep_precedence_even_when_upstream_rejects_them() {
        let result = refresh_with_sources(
            Some("explicit-key".into()),
            || panic!("explicit credentials must not switch accounts"),
            |key| {
                assert_eq!(key, "explicit-key");
                bail!("upstream rejected explicit key")
            },
            OffsetDateTime::UNIX_EPOCH,
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            "upstream rejected explicit key"
        );
    }

    #[test]
    fn native_auth_detection_uses_only_selected_nous_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        assert!(!has_native_auth(&path));
        std::fs::write(
            &path,
            br#"{"providers":{"other":{"access_token":"not-nous"}}}"#,
        )
        .unwrap();
        assert!(!has_native_auth(&path));
        std::fs::write(
            &path,
            br#"{"providers":{"nous":{"refresh_token":"native-refresh"}}}"#,
        )
        .unwrap();
        assert!(has_native_auth(&path));
    }

    #[test]
    fn fixed_native_helper_renews_expired_hourly_auth_without_clearing_pool_cooldown() {
        let directory = tempfile::tempdir().unwrap();
        let package = directory.path().join("hermes_cli");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("__init__.py"), "").unwrap();
        std::fs::write(package.join("auth.py"), r#"
import base64, json, os, time
from pathlib import Path
def resolve_nous_access_token(timeout_seconds):
    assert timeout_seconds == 15
    path = Path(os.environ['HERMES_HOME']) / 'auth.json'
    state = json.loads(path.read_text())
    if state['expires_at'] <= time.time() + 120:
        state['expires_at'] = int(time.time()) + 3600
        claims = base64.urlsafe_b64encode(json.dumps({'exp': state['expires_at']}).encode()).decode().rstrip('=')
        state['token'] = 'header.' + claims + '.renewed'
        state['renewals'] += 1
        path.write_text(json.dumps(state))
    return state['token']
"#).unwrap();
        let path = directory.path().join("auth.json");
        std::fs::write(&path, br#"{"expires_at":0,"token":"expired-hourly-token","renewals":0,"pool_cooldown":"exhausted"}"#).unwrap();
        let source = format!(
            "import sys; sys.path.insert(0, {});\n{}",
            serde_json::to_string(directory.path().to_str().unwrap()).unwrap(),
            HERMES_PORTAL_TOKEN
        );
        let mut command = Command::new("python3");
        command
            .args(["-I", "-c", &source])
            .env("HERMES_HOME", directory.path());
        let record = refresh_with_sources(
            None,
            || native_token_from_command(command, std::time::Duration::from_secs(2)),
            |token| {
                assert!(token.ends_with(".renewed"));
                Ok(br#"{"subscription":{"monthly_credits":22,"credits_remaining":11}}"#.to_vec())
            },
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(record.quota_remaining_percent, Some(50.0));
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(state["renewals"], 1);
        assert_eq!(state["pool_cooldown"], "exhausted");
    }

    #[cfg(unix)]
    #[test]
    fn native_resolver_failure_and_timeout_never_emit_helper_credentials() {
        let mut rejected = Command::new("/bin/sh");
        rejected.args([
            "-c",
            "printf secret-token; printf secret-refresh >&2; exit 1",
        ]);
        let error =
            native_token_from_command(rejected, std::time::Duration::from_secs(1)).unwrap_err();
        assert!(!error.to_string().contains("secret"));
        let mut stalled = Command::new("/bin/sh");
        stalled.args(["-c", "sleep 30"]);
        let start = std::time::Instant::now();
        assert!(native_token_from_command(stalled, std::time::Duration::from_millis(30)).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn monthly_balance_never_turns_topups_or_rollover_into_a_percent() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let record = parse(br#"{"subscription":{"monthly_credits":22,"credits_remaining":0},"purchased_credits_remaining":100}"#, now).unwrap();
        assert_eq!(record.quota_remaining_percent, Some(0.0));
        assert_eq!(
            record.backend_instance.as_deref(),
            Some("opencode:nous-portal-api")
        );
        for input in [
            br#"{"purchased_credits_remaining":100}"#.as_slice(),
            br#"{"subscription":{"monthly_credits":22,"credits_remaining":30}}"#,
        ] {
            assert_eq!(parse(input, now).unwrap().quota_remaining_percent, None);
        }
    }
}
