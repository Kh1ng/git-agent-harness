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
//! This module only parses response bodies already captured elsewhere (see
//! `tests/fixtures/mistral-admin/PROVENANCE.md`) -- like `quota_parser.rs`,
//! it makes no live HTTP calls itself. GAH has no HTTP client dependency
//! today (every other backend/provider integration is CLI-subprocess or
//! PTY-mediated); wiring an actual `/api/admin/...` fetch is a mechanical
//! follow-up once that dependency lands, matching this ticket's
//! `queue:dependency-blocked` status.
#![allow(dead_code)]

use crate::ledger::summary::GroupQuotaObservation;
use crate::ledger::LedgerUsage;
use serde_json::Value;

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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdminModelRateLimit {
    pub model: String,
    pub tokens_per_minute: Option<u64>,
    pub tokens_per_month: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdminRateLimits {
    pub requests_per_second: Option<u64>,
    pub model_limits: Vec<AdminModelRateLimit>,
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
    let model_limits = root
        .get("tokens_limits_by_model")
        .and_then(Value::as_object)
        .map(|models| {
            models
                .iter()
                .map(|(model, limits)| AdminModelRateLimit {
                    model: model.clone(),
                    tokens_per_minute: limits.get("tokens_per_minute").and_then(Value::as_u64),
                    tokens_per_month: limits.get("tokens_per_month").and_then(Value::as_u64),
                })
                .collect()
        })
        .unwrap_or_default();

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

    let quota_used_percent = match (total_usage, usage_limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit * 100.0).clamp(0.0, 100.0)),
        _ => monthly_limit_reached.and_then(|reached| reached.then_some(100.0)),
    };

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
    const ADMIN_USAGE_EUR: &str = include_str!("../../tests/fixtures/mistral-admin/usage_eur.json");
    const RATE_LIMIT: &str = include_str!("../../tests/fixtures/mistral-admin/rate_limit.json");
    const SPEND_LIMIT: &str = include_str!("../../tests/fixtures/mistral-admin/spend_limit.json");
    const SPEND_LIMIT_REACHED_NO_AMOUNT: &str =
        include_str!("../../tests/fixtures/mistral-admin/spend_limit_reached_no_amount.json");

    #[test]
    fn admin_api_key_reads_env_var_and_treats_empty_as_unset() {
        // Serialized via the shared env-mutation lock pattern used
        // elsewhere in this crate would be ideal, but this crate has no
        // such lock yet for this specific var; keep this test narrowly
        // scoped to avoid cross-test interference.
        std::env::remove_var(MISTRAL_ADMIN_API_KEY_ENV);
        assert_eq!(admin_api_key(), None);

        std::env::set_var(MISTRAL_ADMIN_API_KEY_ENV, "");
        assert_eq!(admin_api_key(), None);

        std::env::set_var(MISTRAL_ADMIN_API_KEY_ENV, "sk-admin-test-key");
        assert_eq!(admin_api_key().as_deref(), Some("sk-admin-test-key"));

        std::env::remove_var(MISTRAL_ADMIN_API_KEY_ENV);
    }

    #[test]
    fn parses_vibe_workspace_analytics_aggregate_tokens_and_requests() {
        let usage = parse_vibe_workspace_analytics(VIBE_WORKSPACE_USAGE);
        assert_eq!(
            usage.usage_source.as_deref(),
            Some("mistral_admin_analytics_vibe_by_workspace")
        );
        // 154200 + 188300
        assert_eq!(usage.input_tokens, Some(342500));
        // 38650 + 44210
        assert_eq!(usage.output_tokens, Some(82860));
        // 12000 + 15400
        assert_eq!(usage.cache_read_tokens, Some(27400));
        assert_eq!(usage.total_tokens, Some(342500 + 82860));
        // 210 + 265
        assert_eq!(usage.requests_count, Some(475));
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
    fn parses_admin_usage_billing_in_usd() {
        let usage = parse_admin_usage(ADMIN_USAGE);
        assert_eq!(usage.usage_source.as_deref(), Some("mistral_admin_usage"));
        assert_eq!(usage.actual_cost_usd, Some(41.1));
        assert!(usage.cost_unknown_reason.is_none());
        assert_eq!(usage.observed_at.as_deref(), Some("2026-07-31T23:59:59Z"));
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
        assert_eq!(limits.requests_per_second, Some(5));
        assert_eq!(limits.model_limits.len(), 2);
        let vibe_cli = limits
            .model_limits
            .iter()
            .find(|m| m.model == "mistral-vibe-cli-latest")
            .expect("vibe cli model limit present");
        assert_eq!(vibe_cli.tokens_per_minute, Some(500000));
        assert_eq!(vibe_cli.tokens_per_month, Some(200000000));
    }

    #[test]
    fn parses_admin_spend_limit_exact_ratio() {
        let usage = parse_admin_spend_limit(SPEND_LIMIT);
        assert_eq!(
            usage.usage_source.as_deref(),
            Some("mistral_admin_spend_limit")
        );
        // 169.52 / 500.0 * 100
        assert_eq!(usage.quota_used_percent, Some(33.904));
        assert_eq!(usage.quota_remaining_percent, Some(66.096));
    }

    #[test]
    fn spend_limit_reached_with_no_numeric_breakdown_reports_full_not_unknown() {
        let usage = parse_admin_spend_limit(SPEND_LIMIT_REACHED_NO_AMOUNT);
        assert_eq!(usage.quota_used_percent, Some(100.0));
        assert_eq!(usage.quota_remaining_percent, Some(0.0));
    }

    #[test]
    fn spend_limit_not_reached_with_no_numbers_stays_unknown() {
        let usage = parse_admin_spend_limit(
            r#"{"limits":{"completion":{"monthly_limit_reached":false},"last_payment_failure":false,"last_payment_failure_protection":null,"currency":"USD"}}"#,
        );
        assert_eq!(usage.quota_used_percent, None);
        assert!(usage.usage_source.is_none());
    }

    #[test]
    fn admin_spend_limit_to_quota_observation_matches_codex_style_shape() {
        let obs = admin_spend_limit_to_quota_observation(SPEND_LIMIT, "vibe", None)
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
}
