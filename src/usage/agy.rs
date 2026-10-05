//! Account quota reported by the native Antigravity `/usage` slash command.
use crate::quota_store::QuotaObservationRecord;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub fn refresh(
    executable: &str,
    backend: &str,
    home: Option<&Path>,
) -> Result<Vec<QuotaObservationRecord>> {
    let mut command = Command::new(executable);
    command.args(["-p", "/usage", "--output-format", "json"]);
    if let Some(home) = home {
        command.env("HOME", home);
    }
    let output = crate::runner::process::run_bounded(command, Duration::from_secs(30))
        .context("Antigravity usage command failed or timed out")?;
    if !output.status.success() {
        bail!("Antigravity usage command failed");
    }
    parse(&output.stdout, backend, OffsetDateTime::now_utc())
}

fn parse(output: &[u8], backend: &str, now: OffsetDateTime) -> Result<Vec<QuotaObservationRecord>> {
    let root: Value = serde_json::from_slice(output).context("invalid Antigravity usage JSON")?;
    if root["status"] != "SUCCESS" || root["command"]["name"] != "usage" {
        bail!("Antigravity did not return a usage result");
    }
    let groups = root["command"]["data"]["groups"]
        .as_array()
        .context("Antigravity usage groups missing")?;
    let timestamp = now.format(&Rfc3339)?;
    let mut records = Vec::new();
    for group in groups {
        let Some(buckets) = group["buckets"].as_array() else {
            continue;
        };
        for bucket in buckets {
            let Some(id) = bucket["id"].as_str() else {
                continue;
            };
            let pool = match id {
                "gemini-weekly" | "gemini-5h" => "google-native",
                "3p-weekly" | "3p-5h" => "external",
                _ => continue,
            };
            let window = match id {
                "gemini-weekly" | "3p-weekly" => "weekly",
                _ => "5-hour",
            };
            let Some(fraction) = bucket["remaining_fraction"]
                .as_f64()
                .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            else {
                continue;
            };
            let quota_pool = format!("{backend}:{pool}");
            records.push(QuotaObservationRecord {
                backend: backend.into(),
                backend_instance: Some(quota_pool.clone()),
                credential_id: None,
                model: None,
                quota_pool: Some(quota_pool),
                quota_window: Some(window.into()),
                quota_used_percent: Some((1.0 - fraction) * 100.0),
                quota_remaining_percent: Some(fraction * 100.0),
                quota_reset_at: bucket["reset_time"]
                    .as_str()
                    .filter(|value| OffsetDateTime::parse(value, &Rfc3339).is_ok())
                    .map(str::to_owned),
                observed_at: Some(timestamp.clone()),
                checked_at: Some(timestamp.clone()),
                check_error: None,
                usage_source: Some("agy_cli_usage".into()),
                mistral_admin: None,
                account_usage: None,
            });
        }
    }
    if records.is_empty() {
        bail!("Antigravity usage returned no recognized quota windows");
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_distinct_pools_and_windows_from_native_usage() {
        let output = br#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"buckets":[{"id":"gemini-weekly","remaining_fraction":0.85,"reset_time":"2026-10-11T07:56:28Z"},{"id":"gemini-5h","remaining_fraction":0.98}]},{"buckets":[{"id":"3p-weekly","remaining_fraction":1},{"id":"3p-5h","remaining_fraction":0.5}]}]}}}"#;
        let records = parse(output, "agy-second", OffsetDateTime::UNIX_EPOCH).unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(
            records[0].backend_instance.as_deref(),
            Some("agy-second:google-native")
        );
        assert_eq!(records[0].quota_window.as_deref(), Some("weekly"));
        assert_eq!(records[0].quota_remaining_percent, Some(85.0));
        assert_eq!(
            records[0].quota_reset_at.as_deref(),
            Some("2026-10-11T07:56:28Z")
        );
        assert_eq!(
            records[3].quota_pool.as_deref(),
            Some("agy-second:external")
        );
        assert_eq!(records[3].quota_window.as_deref(), Some("5-hour"));
        assert_eq!(records[3].quota_remaining_percent, Some(50.0));
    }

    #[test]
    fn rejects_failed_or_unrecognized_readings() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert!(parse(
            br#"{"status":"ERROR","command":{"name":"usage","data":{"groups":[]}}}"#,
            "agy",
            now
        )
        .is_err());
        assert!(parse(br#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"buckets":[{"id":"gemini-weekly","remaining_fraction":1.2}]}]}}}"#, "agy", now).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refresh_invokes_slash_command_and_keeps_selected_home() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-agy");
        let args = dir.path().join("args");
        let selected_home = dir.path().join("selected-home");
        std::fs::write(&script, format!(
            "#!/bin/sh\nprintf '%s|%s|%s|%s|%s' \"$1\" \"$2\" \"$3\" \"$4\" \"$HOME\" > '{}'\nprintf '%s' '{{\"status\":\"SUCCESS\",\"command\":{{\"name\":\"usage\",\"data\":{{\"groups\":[{{\"buckets\":[{{\"id\":\"gemini-weekly\",\"remaining_fraction\":0.4}}]}}]}}}}}}'\n",
            args.display()
        )).unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        let records =
            refresh(script.to_str().unwrap(), "agy-second", Some(&selected_home)).unwrap();
        assert_eq!(records[0].quota_remaining_percent, Some(40.0));
        assert_eq!(
            std::fs::read_to_string(args).unwrap(),
            format!("-p|/usage|--output-format|json|{}", selected_home.display())
        );
    }
}
