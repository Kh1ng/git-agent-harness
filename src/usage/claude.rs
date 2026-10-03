//! Claude subscription allowances from the current native Claude Code login.
use crate::quota_store::QuotaObservationRecord;
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn access_token(input: &[u8], now: OffsetDateTime) -> Result<String> {
    let credentials: serde_json::Value =
        serde_json::from_slice(input).context("invalid Claude OAuth credentials")?;
    let oauth = &credentials["claudeAiOauth"];
    if oauth["expiresAt"]
        .as_i64()
        .is_some_and(|expiry| i128::from(expiry) <= now.unix_timestamp_nanos() / 1_000_000)
    {
        bail!("auth_required: Claude OAuth login expired; run claude auth login");
    }
    let token = oauth["accessToken"]
        .as_str()
        .filter(|token| !token.is_empty() && token.len() <= 8192)
        .context("auth_required: Claude OAuth access token is unavailable")?;
    if token.chars().any(char::is_control) {
        bail!("invalid Claude OAuth access token");
    }
    Ok(token.to_owned())
}

fn native_access_token(now: OffsetDateTime) -> Result<String> {
    let configured_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
    if configured_dir.as_ref().is_some_and(|path| path.is_empty()) {
        bail!("CLAUDE_CONFIG_DIR must name a directory");
    }
    #[cfg(target_os = "macos")]
    if configured_dir.is_none() {
        // Claude's default macOS login lives in Keychain; an old credentials
        // file can remain after login. Never fall through a custom config to
        // this default account, or a denied/malformed Keychain read to a file.
        let mut command = Command::new("/usr/bin/security");
        command.args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-a",
            &std::env::var("USER").context("Claude Keychain account unavailable")?,
            "-w",
        ]);
        let output =
            crate::runner::process::run_bounded(command, std::time::Duration::from_secs(5))
                .context("Claude Keychain read failed or timed out")?;
        if output.status.success() {
            return access_token(&output.stdout, now);
        }
        if output.status.code() != Some(44) {
            bail!("auth_required: Claude Keychain access unavailable");
        }
    }
    let directory = match configured_dir {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .context("Claude home unavailable")?,
        )
        .join(".claude"),
    };
    let input = std::fs::read(directory.join(".credentials.json"))
        .context("auth_required: Claude OAuth credentials are unavailable")?;
    access_token(&input, now)
}

pub fn refresh() -> Result<Vec<QuotaObservationRecord>> {
    let token = native_access_token(OffsetDateTime::now_utc())?;
    let escaped = token.replace('\\', "\\\\").replace('"', "\\\"");
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
    let mut child = command.spawn().context("Claude quota curl unavailable")?;
    child.stdin.take().context("Claude quota stdin unavailable")?.write_all(format!(
        "url = \"https://api.anthropic.com/api/oauth/usage\"\nheader = \"Authorization: Bearer {escaped}\"\nheader = \"anthropic-beta: oauth-2025-04-20\"\nheader = \"Accept: application/json\"\n"
    ).as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("Claude OAuth usage request failed (check native login access)");
    }
    parse(&output.stdout, OffsetDateTime::now_utc())
}

/// Only unambiguous account-wide windows apply to the default Claude runner.
/// Per-model and extra-usage budgets must not become a shared account balance.
pub(crate) fn parse(input: &[u8], now: OffsetDateTime) -> Result<Vec<QuotaObservationRecord>> {
    let body: serde_json::Value =
        serde_json::from_slice(input).context("invalid Claude usage response")?;
    let timestamp = now.format(&Rfc3339)?;
    let mut records = Vec::new();
    for (key, window) in [("five_hour", "5-hour"), ("seven_day", "weekly")] {
        let reading = &body[key];
        let Some(used) = reading["utilization"]
            .as_f64()
            .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
        else {
            continue;
        };
        records.push(QuotaObservationRecord {
            backend: "claude".into(),
            backend_instance: Some("claude".into()),
            model: None,
            quota_pool: None,
            quota_window: Some(window.into()),
            quota_used_percent: Some(used),
            quota_remaining_percent: Some(100.0 - used),
            quota_reset_at: reading["resets_at"]
                .as_str()
                .filter(|s| OffsetDateTime::parse(s, &Rfc3339).is_ok())
                .map(str::to_string),
            observed_at: Some(timestamp.clone()),
            checked_at: Some(timestamp.clone()),
            check_error: None,
            usage_source: Some("claude_oauth_usage".into()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        });
    }
    if records.is_empty() {
        bail!("Claude usage returned no recognized account quota windows");
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognized_account_windows_keep_percentages_and_resets_separate() {
        let records = parse(br#"{"five_hour":{"utilization":0,"resets_at":null},"seven_day":{"utilization":78,"resets_at":"2026-10-03T10:59:59Z"},"seven_day_opus":{"utilization":100},"extra_usage":{"utilization":null}}"#, OffsetDateTime::UNIX_EPOCH).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].quota_remaining_percent, Some(100.0));
        assert_eq!(records[0].quota_reset_at, None);
        assert_eq!(records[1].quota_remaining_percent, Some(22.0));
        assert_eq!(records[1].quota_window.as_deref(), Some("weekly"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        for record in &records {
            crate::quota_store::append(&path, record).unwrap();
        }
        let stored = crate::quota_store::load(&path).unwrap();
        assert_eq!(stored.len(), 2);
        assert!(stored
            .iter()
            .all(|record| record.backend_instance.as_deref() == Some("claude")));
        let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "claude",
            None::<String>,
            None::<String>,
        );
        assert_eq!(
            crate::quota_store::latest_windows_for_identity(&stored, &identity).len(),
            2
        );
        identity.backend_instance = "claude-second".into();
        assert!(crate::quota_store::latest_windows_for_identity(&stored, &identity).is_empty());
        for input in [
            br#"{}"#.as_slice(),
            br#"{"five_hour":{"utilization":101}}"#,
            br#"{"seven_day":{"utilization":-1}}"#,
            br#"{"five_hour":{"utilization":"20"}}"#,
        ] {
            assert!(parse(input, OffsetDateTime::UNIX_EPOCH).is_err());
        }
    }

    #[test]
    fn invalid_or_expired_credentials_fail_without_exposing_the_token() {
        let error = access_token(
            br#"{"claudeAiOauth":{"accessToken":"secret-canary","expiresAt":1}}"#,
            OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("expired"));
        assert!(!error.to_string().contains("secret-canary"));
        assert!(access_token(
            br#"{"claudeAiOauth":{"accessToken":"secret\ninjection"}}"#,
            OffsetDateTime::UNIX_EPOCH
        )
        .is_err());
        assert_eq!(
            access_token(
                br#"{"claudeAiOauth":{"accessToken":"test-token"}}"#,
                OffsetDateTime::UNIX_EPOCH
            )
            .unwrap(),
            "test-token"
        );
    }
}
