//! Account quota reported by the native Antigravity `/usage` slash command.
use crate::quota_store::QuotaObservationRecord;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub const USAGE_SOURCE: &str = "agy_cli_usage";

/// The windows one account reports: bucket id, model pool, limit window.
const WINDOWS: [(&str, &str, &str); 4] = [
    ("gemini-weekly", "google-native", "weekly"),
    ("gemini-5h", "google-native", "5-hour"),
    ("3p-weekly", "external", "weekly"),
    ("3p-5h", "external", "5-hour"),
];

/// Read every window of one Antigravity account (`agy`, `agy-second`).
/// A window the response does not report readably comes back as a failed
/// check for that window, so an earlier balance for it is not kept.
pub fn refresh(
    executable: &str,
    account: &str,
    home: Option<&Path>,
) -> Result<Vec<QuotaObservationRecord>> {
    let run = |args: &[&str], seconds: u64| {
        let mut command = Command::new(executable);
        command.args(args);
        if let Some(home) = home {
            command.env("HOME", home);
        }
        crate::runner::process::run_bounded(command, Duration::from_secs(seconds))
    };
    // A build that does not run slash commands in print mode would send
    // `/usage` to the model as a prompt on every check. Only builds whose help
    // documents print-mode slash commands are asked.
    let help = run(&["--help"], 10)
        .context("Antigravity CLI could not be run (not installed, or it timed out)")?;
    if ![&help.stdout, &help.stderr]
        .iter()
        .any(|text| String::from_utf8_lossy(text).contains("--disable-slash-commands"))
    {
        bail!("this Antigravity build does not run slash commands in print mode; usage is not sampled");
    }
    let output = run(&["--print", "/usage", "--output-format", "json"], 30)
        .context("Antigravity usage command timed out")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail: String = detail.trim().chars().take(200).collect();
        bail!(
            "Antigravity usage command failed ({}): {}",
            output.status,
            crate::redact::redact(&detail)
        );
    }
    parse(&output.stdout, account, OffsetDateTime::now_utc())
}

/// Logical backends that draw on one account's quota. Readings are stored
/// once per name, because a route only sees readings for its own backend.
fn logical_backends(account: &str) -> Vec<&str> {
    let mut backends = vec![account];
    backends.extend(
        ["agy-main"]
            .into_iter()
            .filter(|alias| crate::availability::agy_account(alias) == Some(account)),
    );
    backends
}

fn parse(output: &[u8], account: &str, now: OffsetDateTime) -> Result<Vec<QuotaObservationRecord>> {
    let root: Value = serde_json::from_slice(output).context("invalid Antigravity usage JSON")?;
    // A model turn means the CLI treated `/usage` as a prompt.
    if root["status"] != "SUCCESS"
        || root["command"]["name"] != "usage"
        || root["num_turns"].as_u64().is_some_and(|turns| turns > 0)
    {
        bail!("Antigravity did not return a usage result");
    }
    let buckets: Vec<&Value> = root["command"]["data"]["groups"]
        .as_array()
        .context("Antigravity usage groups missing")?
        .iter()
        .filter_map(|group| group["buckets"].as_array())
        .flatten()
        .collect();
    if !buckets
        .iter()
        .any(|bucket| WINDOWS.iter().any(|(id, _, _)| bucket["id"] == *id))
    {
        bail!("Antigravity usage returned no recognized quota windows");
    }
    let timestamp = now.format(&Rfc3339)?;
    let mut records = Vec::new();
    for (id, pool, window) in WINDOWS {
        let bucket = buckets.iter().find(|bucket| bucket["id"] == id);
        // The response omits zero-valued fields, so a reported window with no
        // fraction is an exhausted one. Anything else unreadable is unknown.
        let fraction = bucket.and_then(|bucket| match &bucket["remaining_fraction"] {
            Value::Null => Some(0.0),
            value => value
                .as_f64()
                .filter(|value| value.is_finite() && (0.0..=1.0).contains(value)),
        });
        for backend in logical_backends(account) {
            records.push(QuotaObservationRecord {
                backend: backend.into(),
                backend_instance: Some(format!("{backend}:{pool}")),
                credential_id: None,
                model: None,
                quota_pool: Some(format!("{account}:{pool}")),
                quota_window: Some(window.into()),
                quota_remaining_percent: fraction.map(|fraction| fraction * 100.0),
                quota_reset_at: bucket
                    .and_then(|bucket| bucket["reset_time"].as_str())
                    .filter(|value| OffsetDateTime::parse(value, &Rfc3339).is_ok())
                    .map(str::to_owned),
                observed_at: fraction.map(|_| timestamp.clone()),
                checked_at: Some(timestamp.clone()),
                check_error: fraction.is_none().then(|| {
                    format!("Antigravity usage reported no readable {window} balance for {pool}")
                }),
                usage_source: Some(USAGE_SOURCE.into()),
                mistral_admin: None,
                account_usage: None,
            });
        }
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
        assert!(records.iter().all(|record| record.check_error.is_none()));
    }

    fn usage(buckets: &str) -> Vec<u8> {
        format!(
            r#"{{"status":"SUCCESS","num_turns":0,"command":{{"name":"usage","data":{{"groups":[{{"buckets":[{buckets}]}}]}}}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn default_account_readings_reach_routes_named_agy_main() {
        let output = usage(r#"{"id":"gemini-weekly","remaining_fraction":0.25}"#);
        let records = parse(&output, "agy", OffsetDateTime::UNIX_EPOCH).unwrap();
        let alias = records
            .iter()
            .find(|record| record.backend == "agy-main" && record.quota_remaining_percent.is_some())
            .unwrap();
        // The shape a route configured as `agy-main` matches readings on.
        let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "agy-main",
            Some("Gemini 3.5 Flash"),
            crate::availability::derive_quota_pool("agy-main", Some("Gemini 3.5 Flash")),
        );
        assert_eq!(
            alias.backend_instance.as_deref(),
            Some(identity.backend_instance.as_str())
        );
        assert_eq!(alias.quota_pool, identity.quota_pool);
        assert_eq!(alias.quota_remaining_percent, Some(25.0));
        assert!(records
            .iter()
            .any(|record| record.backend == "agy" && record.quota_remaining_percent == Some(25.0)));
    }

    #[test]
    fn exhausted_and_unreadable_windows_do_not_keep_an_older_balance() {
        // No fraction on a reported window: exhausted. An out-of-range
        // fraction or a window missing from the response: unknown.
        let output = usage(
            r#"{"id":"gemini-weekly","reset_time":"2026-10-11T07:56:28Z"},{"id":"gemini-5h","remaining_fraction":1.2}"#,
        );
        let records = parse(&output, "agy-second", OffsetDateTime::UNIX_EPOCH).unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(records[0].quota_remaining_percent, Some(0.0));
        assert!(records[0].check_error.is_none());
        for unreadable in &records[1..] {
            assert!(unreadable.quota_remaining_percent.is_none());
            assert!(unreadable.observed_at.is_none());
            assert!(unreadable.check_error.is_some());
            assert!(unreadable.quota_window.is_some());
        }
    }

    #[test]
    fn rejects_responses_that_are_not_a_usage_result() {
        let now = OffsetDateTime::UNIX_EPOCH;
        for output in [
            r#"{"status":"ERROR","command":{"name":"usage","data":{"groups":[]}}}"#,
            // The CLI answered `/usage` with a model turn.
            r#"{"status":"SUCCESS","num_turns":1,"command":{"name":"usage","data":{"groups":[{"buckets":[{"id":"gemini-weekly","remaining_fraction":1}]}]}}}"#,
            r#"{"status":"SUCCESS","response":"Here is how usage works"}"#,
            r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"buckets":[{"id":"other","remaining_fraction":1}]}]}}}"#,
        ] {
            assert!(parse(output.as_bytes(), "agy", now).is_err(), "{output}");
        }
    }

    #[cfg(unix)]
    fn fake_agy(dir: &Path, help: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("fake-agy");
        let args = dir.join("args");
        std::fs::write(&script, format!(
            "#!/bin/sh\nif [ \"$1\" = --help ]; then echo '{help}' >&2; exit 0; fi\nprintf '%s|%s|%s|%s|%s' \"$1\" \"$2\" \"$3\" \"$4\" \"$HOME\" > '{}'\nprintf '%s' '{{\"status\":\"SUCCESS\",\"command\":{{\"name\":\"usage\",\"data\":{{\"groups\":[{{\"buckets\":[{{\"id\":\"gemini-weekly\",\"remaining_fraction\":0.4}}]}}]}}}}}}'\n",
            args.display()
        )).unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        (script, args)
    }

    #[cfg(unix)]
    #[test]
    fn refresh_invokes_slash_command_and_keeps_selected_home() {
        let dir = tempfile::tempdir().unwrap();
        let (script, args) = fake_agy(dir.path(), "  --disable-slash-commands  Disable them");
        let selected_home = dir.path().join("selected-home");
        let records =
            refresh(script.to_str().unwrap(), "agy-second", Some(&selected_home)).unwrap();
        assert_eq!(records[0].quota_remaining_percent, Some(40.0));
        assert_eq!(
            std::fs::read_to_string(args).unwrap(),
            format!(
                "--print|/usage|--output-format|json|{}",
                selected_home.display()
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn refresh_never_sends_usage_to_a_build_without_print_mode_slash_commands() {
        let dir = tempfile::tempdir().unwrap();
        let (script, args) = fake_agy(dir.path(), "  --print  Run a single prompt");
        let error = refresh(script.to_str().unwrap(), "agy", None).unwrap_err();
        assert!(error.to_string().contains("slash commands"), "{error}");
        assert!(!args.exists(), "the usage prompt must not have been sent");
    }
}
