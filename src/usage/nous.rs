//! Nous Portal subscription balances use the account endpoint, not OpenCode telemetry.
use crate::quota_store::QuotaObservationRecord;
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub fn refresh() -> Result<QuotaObservationRecord> {
    let key = std::env::var("NOUS_API_KEY")
        .ok()
        .filter(|key| !key.is_empty())
        .context("auth_required: NOUS_API_KEY is not configured")?;
    if key.chars().any(char::is_control) {
        bail!("invalid NOUS_API_KEY");
    }
    let escaped = key.replace('\\', "\\\\").replace('"', "\\\"");
    let mut command = Command::new("curl");
    command
        .args([
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
    parse(&output.stdout, OffsetDateTime::now_utc())
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
        quota_used_percent: remaining_percent.map(|v| 100.0 - v),
        quota_remaining_percent: remaining_percent,
        quota_reset_at: reset,
        observed_at: Some(timestamp.clone()),
        checked_at: Some(timestamp),
        check_error: None,
        usage_source: Some("nous_portal_account".into()),
        mistral_admin: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
