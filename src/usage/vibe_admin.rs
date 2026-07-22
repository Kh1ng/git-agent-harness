//! Issue #154: parsers for the Mistral Admin API's aggregate usage, billing,
//! and limits endpoints (account-level, org-wide -- not per-attempt).
//!
//! Per-attempt numbers for Vibe stay exact via the CLI-wrap fallback in
//! `usage::vibe::parse_vibe_session_metadata`, which reads Vibe's own
//! session metadata; the Admin API cannot correlate its aggregate figures
//! back to a single dispatch/attempt, so it is only ever used here for
//! account-level aggregate/billing/limit data, never to override a known
//! per-attempt observation.
//!
//! `fetch_admin_endpoint`/`refresh_admin_data` below are the real callers:
//! GAH has no HTTP client *dependency* (every other backend/provider
//! integration is CLI-subprocess or PTY-mediated), so the Admin API is
//! fetched the same way -- shelling out to `curl` -- rather than pulling in
//! `reqwest`/`hyper`. The response bodies these parsers were developed
//! against are captured in `tests/fixtures/mistral-admin/PROVENANCE.md`.

use crate::ledger::summary::GroupQuotaObservation;
use crate::ledger::LedgerUsage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write as _;
use std::process::{Command, Stdio};

const MISTRAL_ADMIN_API_BASE: &str = "https://api.mistral.ai";

/// Env var carrying the Mistral Admin API key. Sourced the same way as
/// every other GAH-internal API credential (`Defaults::llm_api_key`,
/// `Profile::pat`'s `GITLAB_PAT`/`GITHUB_TOKEN`): a direct `std::env::var`
/// read, which `dispatch::environment::export_profile_env` populates from
/// the profile's `env_file`/`env_file_prod` before any backend runs. Never
/// invents a second credential-loading path.
const MISTRAL_ADMIN_API_KEY_ENV: &str = "MISTRAL_ADMIN_API_KEY";

/// The Mistral Admin API key, if configured. `None` (not an empty string)
/// when unset, so callers can distinguish "not configured" from "configured
/// as empty".
pub fn admin_api_key() -> Option<String> {
    std::env::var(MISTRAL_ADMIN_API_KEY_ENV)
        .ok()
        .filter(|v| !v.is_empty())
}

/// Fetch one Mistral Admin API endpoint via `curl`. The key is handed to
/// curl through its stdin config (`-K -`), never as a command-line
/// argument, so it never appears in `ps`/`/proc/<pid>/cmdline` on a shared
/// host. Any transport, auth, or non-2xx failure returns `Ok(None)` -- a
/// failed fetch means "no observation this cycle", never a fabricated
/// reading.
pub fn fetch_admin_endpoint(path: &str, api_key: &str) -> std::io::Result<Option<String>> {
    let mut child = Command::new("curl")
        .args(["-sS", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        let escaped_key = api_key.replace('\\', "\\\\").replace('"', "\\\"");
        let config = format!(
            "silent\nfail\nurl = \"{MISTRAL_ADMIN_API_BASE}{path}\"\nheader = \"Authorization: Bearer {escaped_key}\"\nheader = \"Accept: application/json\"\n"
        );
        stdin.write_all(config.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// One refresh cycle's worth of account-level Mistral Admin API data.
/// Each field independently falls back to its type's "no data" default when
/// its own endpoint fails to fetch or parse -- a rate-limit outage must not
/// also hide a spend-limit reading that came back clean.
#[derive(Debug, Clone, Default)]
pub struct AdminRefresh {
    pub workspace_usage: LedgerUsage,
    pub billing: LedgerUsage,
    pub rate_limits: AdminRateLimits,
    pub spend_limit: Option<GroupQuotaObservation>,
}

/// The real caller for every parser in this module: fetch all four Admin
/// API endpoints for the configured account and parse each response.
/// `window` bounds the aggregate-usage query as Unix-second
/// `(start_time, end_time)`.
pub fn refresh_admin_data(
    api_key: &str,
    window: (i64, i64),
    backend: &str,
    model: Option<&str>,
) -> AdminRefresh {
    let (start_time, end_time) = window;

    let workspace_usage = fetch_admin_endpoint(
        &format!(
            "/api/admin/analytics/vibe/usage/by_workspace?start_time={start_time}&end_time={end_time}"
        ),
        api_key,
    )
    .ok()
    .flatten()
    .map(|body| parse_vibe_workspace_analytics(&body))
    .unwrap_or_default();

    let billing = fetch_admin_endpoint("/api/admin/usage", api_key)
        .ok()
        .flatten()
        .map(|body| parse_admin_usage(&body))
        .unwrap_or_default();

    let rate_limits = fetch_admin_endpoint("/api/admin/rate-limit", api_key)
        .ok()
        .flatten()
        .map(|body| parse_admin_rate_limit(&body))
        .unwrap_or_default();

    let spend_limit = fetch_admin_endpoint("/api/admin/spend-limit", api_key)
        .ok()
        .flatten()
        .and_then(|body| admin_spend_limit_to_quota_observation(&body, backend, model));

    AdminRefresh {
        workspace_usage,
        billing,
        rate_limits,
        spend_limit,
    }
}

fn sum_u64_field(entries: &[Value], field: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut saw_any = false;
    for entry in entries {
        if let Some(n) = entry.get(field).and_then(Value::as_u64) {
            total = total.saturating_add(n);
            saw_any = true;
        }
    }
    saw_any.then_some(total)
}

/// Parse `GET /api/admin/analytics/vibe/usage/by_workspace`
/// (`VibeWorkspaceStatsOUT`) into an aggregate `LedgerUsage`: token and
/// request counts summed across the queried window. Never fabricates a
/// count when the response carries none.
pub fn parse_vibe_workspace_analytics(json: &str) -> LedgerUsage {
    let Ok(root) = serde_json::from_str::<Value>(json) else {
        return LedgerUsage::default();
    };

    let consumed_tokens = root
        .get("consumed_tokens")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let user_prompts = root
        .get("user_prompts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let input_tokens = sum_u64_field(&consumed_tokens, "input_tokens");
    let output_tokens = sum_u64_field(&consumed_tokens, "output_tokens");
    let cache_read_tokens = sum_u64_field(&consumed_tokens, "cached_tokens");
    let requests_count = sum_u64_field(&user_prompts, "nb_prompts_total");

    let total_tokens = match (input_tokens, output_tokens) {
        (Some(input), Some(output)) => Some(input.saturating_add(output)),
        _ => None,
    };

    if input_tokens.is_none()
        && output_tokens.is_none()
        && cache_read_tokens.is_none()
        && requests_count.is_none()
    {
        return LedgerUsage::default();
    }

    LedgerUsage {
        usage_source: Some("mistral_admin_analytics_vibe_by_workspace".to_string()),
        input_tokens,
        output_tokens,
        cache_read_tokens,
        total_tokens,
        requests_count,
        ..LedgerUsage::default()
    }
}

/// Parse `GET /api/admin/usage` (`UsageOUTJSON`) into a billing `LedgerUsage`.
/// `actual_cost_usd` is only ever set when the response's own `currency` is
/// USD -- converting a EUR/other-currency `vibe_usage` figure into a USD
/// number would fabricate an exchange rate this parser has no basis for, so
/// it leaves cost unknown (with `cost_unknown_reason` explaining why)
/// instead.
pub fn parse_admin_usage(json: &str) -> LedgerUsage {
    let Ok(root) = serde_json::from_str::<Value>(json) else {
        return LedgerUsage::default();
    };

    let vibe_usage = root.get("vibe_usage").and_then(Value::as_f64);
    let currency = root.get("currency").and_then(Value::as_str);
    let observed_at = root
        .get("end_date")
        .and_then(Value::as_str)
        .or_else(|| root.get("date").and_then(Value::as_str))
        .map(str::to_string);

    if vibe_usage.is_none() {
        return LedgerUsage::default();
    }

    let (actual_cost_usd, cost_unknown_reason) = match currency {
        Some("USD") => (vibe_usage, None),
        Some(other) => (
            None,
            Some(format!(
                "mistral_admin_usage reported vibe_usage in {other}, not USD"
            )),
        ),
        None => (
            None,
            Some("mistral_admin_usage currency unknown".to_string()),
        ),
    };

    LedgerUsage {
        usage_source: Some("mistral_admin_usage".to_string()),
        actual_cost_usd,
        cost_unknown_reason,
        observed_at,
        ..LedgerUsage::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AdminModelRateLimit {
    pub model: String,
    pub tokens_per_minute: Option<u64>,
    pub tokens_per_month: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AdminRateLimits {
    pub requests_per_second: Option<u64>,
    pub model_limits: Vec<AdminModelRateLimit>,
}

fn parse_admin_rate_limit_model_limits(value: Option<&Value>) -> Vec<AdminModelRateLimit> {
    match value {
        Some(Value::Object(models)) => models
            .iter()
            .map(|(model, limits)| AdminModelRateLimit {
                model: model.clone(),
                tokens_per_minute: limits.get("tokens_per_minute").and_then(Value::as_u64),
                tokens_per_month: limits.get("tokens_per_month").and_then(Value::as_u64),
            })
            .collect(),
        Some(Value::Array(models)) => models
            .iter()
            .map(|limits| AdminModelRateLimit {
                // The public docs playground example currently renders the
                // limits as an array without model keys, so preserve the
                // ceilings without inventing a model label.
                model: limits
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tokens_per_minute: limits.get("tokens_per_minute").and_then(Value::as_u64),
                tokens_per_month: limits.get("tokens_per_month").and_then(Value::as_u64),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse `GET /api/admin/rate-limit` (`RateLimitsOUT`). Per-model token
/// limits don't collapse into a single `quota_used_percent`-shaped value
/// (there's no "used" figure here, only configured ceilings), so this
/// returns its own struct rather than overloading `LedgerUsage`.
pub fn parse_admin_rate_limit(json: &str) -> AdminRateLimits {
    let Ok(root) = serde_json::from_str::<Value>(json) else {
        return AdminRateLimits::default();
    };

    let requests_per_second = root.get("requests_per_second").and_then(Value::as_u64);
    let model_limits = parse_admin_rate_limit_model_limits(root.get("tokens_limits_by_model"));

    AdminRateLimits {
        requests_per_second,
        model_limits,
    }
}

/// Parse `GET /api/admin/spend-limit` (`LimitsOUT`) into a `LedgerUsage`
/// carrying `quota_used_percent`/`quota_remaining_percent`.
///
/// `total_usage`/`usage_limit` give an exact ratio when both are present.
/// When the API only asserts `monthly_limit_reached: true` with no numeric
/// breakdown, that boolean *is* the evidence -- it directly means 100% used,
/// not an inferred fabrication. `monthly_limit_reached: false` with no
/// numbers stays unknown (anywhere from 0-99% is still consistent with
/// "not yet reached").
pub fn parse_admin_spend_limit(json: &str) -> LedgerUsage {
    let Ok(root) = serde_json::from_str::<Value>(json) else {
        return LedgerUsage::default();
    };
    let Some(completion) = root.get("limits").and_then(|l| l.get("completion")) else {
        return LedgerUsage::default();
    };

    let total_usage = completion.get("total_usage").and_then(Value::as_f64);
    let usage_limit = completion.get("usage_limit").and_then(Value::as_f64);
    let monthly_limit_reached = completion
        .get("monthly_limit_reached")
        .and_then(Value::as_bool);

    let quota_used_percent =
        admin_spend_limit_quota_used_percent(total_usage, usage_limit, monthly_limit_reached);

    if quota_used_percent.is_none() {
        return LedgerUsage::default();
    }

    LedgerUsage {
        usage_source: Some("mistral_admin_spend_limit".to_string()),
        quota_used_percent,
        quota_remaining_percent: quota_used_percent.map(|pct| 100.0 - pct),
        ..LedgerUsage::default()
    }
}

fn admin_spend_limit_quota_used_percent(
    total_usage: Option<f64>,
    usage_limit: Option<f64>,
    monthly_limit_reached: Option<bool>,
) -> Option<f64> {
    match (total_usage, usage_limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit * 100.0).clamp(0.0, 100.0)),
        _ => monthly_limit_reached.and_then(|reached| reached.then_some(100.0)),
    }
}

/// Same shape as `usage::codex_status_to_quota_observation`: turn a raw
/// Admin API spend-limit response into an account-level
/// `GroupQuotaObservation`, timestamped with the observation time (the
/// response body carries no observation timestamp of its own).
pub fn admin_spend_limit_to_quota_observation(
    json: &str,
    backend: &str,
    model: Option<&str>,
) -> Option<GroupQuotaObservation> {
    let usage = parse_admin_spend_limit(json);
    usage.usage_source.as_ref()?;
    Some(GroupQuotaObservation {
        backend: backend.to_string(),
        model: model.map(|m| m.to_string()),
        quota_window: Some("monthly".to_string()),
        quota_used_percent: usage.quota_used_percent,
        quota_remaining_percent: usage.quota_remaining_percent,
        quota_reset_at: None,
        observed_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok(),
        usage_source: usage.usage_source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIBE_WORKSPACE_USAGE: &str =
        include_str!("../../tests/fixtures/mistral-admin/vibe_workspace_usage.json");
    const ADMIN_USAGE: &str = include_str!("../../tests/fixtures/mistral-admin/usage.json");
    const ADMIN_USAGE_EUR: &str = r#"{
  "audio": { "models": [ [ [ [ null ] ] ] ] },
  "audio_characters": { "models": [ [ [ [ null ] ] ] ] },
  "chat": { "models": [ [ [ [ null ] ] ] ] },
  "completion": { "models": [ [ [ [ null ] ] ] ] },
  "connectors": { "models": [ [ [ [ null ] ] ] ] },
  "currency": "EUR",
  "currency_symbol": "€",
  "date": "2025-12-17T10:25:07.818693Z",
  "end_date": "2025-12-17T10:25:07.818693Z",
  "fine_tuning": { "storage": [87], "training": [ [ [ [ null ] ] ] ] },
  "libraries_api": {
    "audio_seconds": { "models": [ [ [ [ null ] ] ] ] },
    "pages": { "models": [ [ [ [ null ] ] ] ] },
    "tokens": { "models": [ [ [ [ null ] ] ] ] }
  },
  "next_month": null,
  "ocr": { "models": [ [ [ [ null ] ] ] ] },
  "previous_month": null,
  "prices": null,
  "start_date": "2025-12-17T10:25:07.818693Z",
  "vibe_usage": 37.8
}"#;
    const RATE_LIMIT: &str = include_str!("../../tests/fixtures/mistral-admin/rate_limit.json");
    // The public docs only publish this spend-limit example inline, so keep it
    // local to the parser test instead of presenting it as a captured fixture.
    const SPEND_LIMIT_DOCS: &str = r#"{
  "limits": {
    "completion": {
      "monthly_limit_reached": false
    },
    "currency": "ipsum eiusmod",
    "last_payment_failure": false,
    "last_payment_failure_protection": null
  }
}"#;

    fn spend_limit_ratio_body() -> String {
        serde_json::json!({
            "limits": {
                "completion": {
                    "no_monthly_limit": false,
                    "monthly_limit_reached": false,
                    "usage": 128.42,
                    "vibe_usage": 41.1,
                    "total_usage": 169.52,
                    "usage_limit": 500.0,
                    "usage_limit_organization": 500.0
                },
                "last_payment_failure": false,
                "last_payment_failure_protection": null,
                "currency": "USD"
            }
        })
        .to_string()
    }

    #[test]
    fn admin_api_key_reads_env_var_and_treats_empty_as_unset() {
        {
            let _guard = crate::test_support::MistralAdminKeyEnvGuard::unset();
            assert_eq!(admin_api_key(), None);
        }
        {
            let _guard = crate::test_support::MistralAdminKeyEnvGuard::set("");
            assert_eq!(admin_api_key(), None);
        }
        {
            let _guard = crate::test_support::MistralAdminKeyEnvGuard::set("sk-admin-test-key");
            assert_eq!(admin_api_key().as_deref(), Some("sk-admin-test-key"));
        }
    }

    #[test]
    fn parses_vibe_workspace_analytics_aggregate_tokens_and_requests() {
        let usage = parse_vibe_workspace_analytics(VIBE_WORKSPACE_USAGE);
        assert_eq!(
            usage.usage_source.as_deref(),
            Some("mistral_admin_analytics_vibe_by_workspace")
        );
        assert_eq!(usage.input_tokens, Some(78));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.cache_read_tokens, Some(32));
        assert_eq!(usage.total_tokens, Some(83));
        assert_eq!(usage.requests_count, Some(71));
    }

    #[test]
    fn vibe_workspace_analytics_empty_window_stays_unknown_not_zero() {
        let usage = parse_vibe_workspace_analytics(
            r#"{"start_time":1,"end_time":2,"sessions":[],"user_prompts":[],"active_users":[],"consumed_tokens":[],"tool_calls":[],"tool_calls_by_name":[],"session_durations":[]}"#,
        );
        assert!(usage.usage_source.is_none());
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.requests_count, None);
    }

    #[test]
    fn parses_admin_usage_billing_example_stays_unknown_when_currency_is_unset() {
        let usage = parse_admin_usage(ADMIN_USAGE);
        assert_eq!(usage.usage_source.as_deref(), Some("mistral_admin_usage"));
        assert_eq!(usage.actual_cost_usd, None);
        assert_eq!(
            usage.cost_unknown_reason.as_deref(),
            Some("mistral_admin_usage currency unknown")
        );
        assert_eq!(
            usage.observed_at.as_deref(),
            Some("2025-12-17T10:25:07.818693Z")
        );
    }

    #[test]
    fn admin_usage_non_usd_currency_never_fabricates_a_usd_cost() {
        let usage = parse_admin_usage(ADMIN_USAGE_EUR);
        assert_eq!(usage.actual_cost_usd, None);
        assert_eq!(
            usage.cost_unknown_reason.as_deref(),
            Some("mistral_admin_usage reported vibe_usage in EUR, not USD")
        );
    }

    #[test]
    fn parses_admin_rate_limit_per_model_ceilings() {
        let limits = parse_admin_rate_limit(RATE_LIMIT);
        assert_eq!(limits.requests_per_second, Some(87));
        assert_eq!(limits.model_limits.len(), 1);
        let model_limits = &limits.model_limits[0];
        assert!(model_limits.model.is_empty());
        assert_eq!(model_limits.tokens_per_minute, Some(14));
        assert_eq!(model_limits.tokens_per_month, Some(56));
    }

    #[test]
    fn parses_admin_spend_limit_exact_ratio() {
        let quota_used_percent =
            admin_spend_limit_quota_used_percent(Some(169.52), Some(500.0), Some(false));
        assert_eq!(quota_used_percent, Some(33.904));
    }

    #[test]
    fn spend_limit_reached_with_no_numeric_breakdown_reports_full_not_unknown() {
        let quota_used_percent = admin_spend_limit_quota_used_percent(None, None, Some(true));
        assert_eq!(quota_used_percent, Some(100.0));
    }

    #[test]
    fn spend_limit_not_reached_with_no_numbers_stays_unknown() {
        let usage = parse_admin_spend_limit(SPEND_LIMIT_DOCS);
        assert_eq!(usage.quota_used_percent, None);
        assert!(usage.usage_source.is_none());
    }

    #[test]
    fn admin_spend_limit_to_quota_observation_matches_codex_style_shape() {
        let obs = admin_spend_limit_to_quota_observation(&spend_limit_ratio_body(), "vibe", None)
            .expect("spend limit yields an observation");
        assert_eq!(obs.backend, "vibe");
        assert_eq!(obs.quota_used_percent, Some(33.904));
        assert_eq!(obs.quota_window.as_deref(), Some("monthly"));
        assert_eq!(
            obs.usage_source.as_deref(),
            Some("mistral_admin_spend_limit")
        );
        assert!(obs.observed_at.is_some());
    }

    #[test]
    fn admin_spend_limit_to_quota_observation_none_when_no_data() {
        assert!(admin_spend_limit_to_quota_observation("{}", "vibe", None).is_none());
    }

    #[test]
    fn malformed_json_never_panics_and_returns_empty() {
        assert!(parse_vibe_workspace_analytics("not json")
            .usage_source
            .is_none());
        assert!(parse_admin_usage("not json").usage_source.is_none());
        assert_eq!(
            parse_admin_rate_limit("not json"),
            AdminRateLimits::default()
        );
        assert!(parse_admin_spend_limit("not json").usage_source.is_none());
    }

    fn write_fake_curl(dir: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("curl");
        std::fs::write(&path, body).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    #[test]
    fn fetch_admin_endpoint_returns_stdout_on_success() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let dir = tempfile::tempdir().unwrap();
        write_fake_curl(
            dir.path(),
            "#!/bin/sh\ncat > /dev/null\nprintf '%s' '{\"ok\":true}'\n",
        );
        let _path_guard = crate::test_support::PathGuard::set(dir.path());

        let body = fetch_admin_endpoint("/api/admin/spend-limit", "sk-test")
            .unwrap()
            .expect("successful fetch returns a body");
        assert_eq!(body, r#"{"ok":true}"#);
    }

    #[test]
    fn fetch_admin_endpoint_returns_none_on_curl_failure() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let dir = tempfile::tempdir().unwrap();
        write_fake_curl(dir.path(), "#!/bin/sh\ncat > /dev/null\nexit 22\n");
        let _path_guard = crate::test_support::PathGuard::set(dir.path());

        let body = fetch_admin_endpoint("/api/admin/spend-limit", "sk-test").unwrap();
        assert!(body.is_none());
    }

    // The Admin API key must never be observable via `ps`/`/proc/<pid>/cmdline`
    // while curl runs -- it is passed through curl's stdin config (`-K -`),
    // never as a command-line argument.
    #[test]
    fn fetch_admin_endpoint_never_puts_api_key_in_argv() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let argv_path = dir.path().join("argv.txt");
        let stdin_path = dir.path().join("stdin.txt");
        write_fake_curl(
            dir.path(),
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{argv}'\ncat > '{stdin}'\nprintf '{{}}'\n",
                argv = argv_path.display(),
                stdin = stdin_path.display(),
            ),
        );
        let _path_guard = crate::test_support::PathGuard::set(dir.path());

        fetch_admin_endpoint("/api/admin/usage", "sk-super-secret").unwrap();

        let argv = std::fs::read_to_string(&argv_path).unwrap();
        assert!(!argv.contains("sk-super-secret"), "got argv: {argv}");
        let stdin = std::fs::read_to_string(&stdin_path).unwrap();
        assert!(stdin.contains("sk-super-secret"), "got stdin: {stdin}");
    }

    #[test]
    fn refresh_admin_data_fetches_and_parses_all_four_endpoints() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mistral-admin");
        let rate_limit_path = dir.path().join("rate_limit.json");
        let spend_limit_path = dir.path().join("spend_limit.json");
        std::fs::write(&rate_limit_path, RATE_LIMIT).unwrap();
        std::fs::write(&spend_limit_path, spend_limit_ratio_body()).unwrap();
        write_fake_curl(
            dir.path(),
            &format!(
                "#!/bin/sh\ncfg=$(cat)\ncase \"$cfg\" in\n  *analytics/vibe/usage/by_workspace*) cat '{fixtures}/vibe_workspace_usage.json' ;;\n  *api/admin/usage*) cat '{fixtures}/usage.json' ;;\n  *api/admin/rate-limit*) cat '{rate_limit}' ;;\n  *api/admin/spend-limit*) cat '{spend_limit}' ;;\n  *) exit 1 ;;\nesac\n",
                fixtures = fixtures,
                rate_limit = rate_limit_path.display(),
                spend_limit = spend_limit_path.display(),
            ),
        );
        let _path_guard = crate::test_support::PathGuard::set(dir.path());

        let refresh = refresh_admin_data("sk-test", (1_000, 2_000), "vibe", None);

        assert_eq!(refresh.workspace_usage.requests_count, Some(71));
        assert_eq!(refresh.billing.actual_cost_usd, None);
        assert_eq!(
            refresh.billing.cost_unknown_reason.as_deref(),
            Some("mistral_admin_usage currency unknown")
        );
        assert_eq!(refresh.rate_limits.requests_per_second, Some(87));
        let spend = refresh.spend_limit.expect("spend limit observation");
        assert_eq!(spend.backend, "vibe");
        assert_eq!(spend.quota_used_percent, Some(33.904));
    }

    #[test]
    fn refresh_admin_data_endpoint_failure_leaves_only_that_field_unknown() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let spend_limit_path = dir.path().join("spend_limit.json");
        std::fs::write(&spend_limit_path, spend_limit_ratio_body()).unwrap();
        write_fake_curl(
            dir.path(),
            &format!(
                "#!/bin/sh\ncfg=$(cat)\ncase \"$cfg\" in\n  *api/admin/spend-limit*) cat '{spend_limit}' ;;\n  *) exit 1 ;;\nesac\n",
                spend_limit = spend_limit_path.display(),
            ),
        );
        let _path_guard = crate::test_support::PathGuard::set(dir.path());

        let refresh = refresh_admin_data("sk-test", (1_000, 2_000), "vibe", None);

        assert!(refresh.workspace_usage.usage_source.is_none());
        assert!(refresh.billing.usage_source.is_none());
        assert_eq!(refresh.rate_limits, AdminRateLimits::default());
        assert!(refresh.spend_limit.is_some(), "spend limit still succeeds");
    }
}
