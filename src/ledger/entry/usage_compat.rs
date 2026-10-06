use super::{AttemptBehaviorMetrics, LedgerUsage};
use serde::Deserialize;

// Accept historical used-percent readings at the wire boundary so every
// consumer, including summary aggregation, sees the canonical remaining value.
#[derive(Deserialize)]
pub(super) struct LedgerUsageRaw {
    usage_source: Option<String>,
    usage_classification: Option<String>,
    backend_instance: Option<String>,
    provider: Option<String>,
    actual_model: Option<String>,
    actual_model_unknown_reason: Option<String>,
    provider_unknown_reason: Option<String>,
    account_label: Option<String>,
    auth_source_label: Option<String>,
    quota_pool: Option<String>,
    provider_attribution_source: Option<String>,
    pricing_source: Option<String>,
    pricing_version: Option<String>,
    cost_unknown_reason: Option<String>,
    observed_at: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: Option<u64>,
    requests_count: Option<u64>,
    estimated_cost_usd: Option<f64>,
    actual_cost_usd: Option<f64>,
    quota_window: Option<String>,
    quota_remaining_percent: Option<f64>,
    quota_reset_at: Option<String>,
    token_usage_unknown_reason: Option<String>,
    quota_unknown_reason: Option<String>,
    behavior_metrics: Option<AttemptBehaviorMetrics>,
    quota_used_percent: Option<f64>,
}

impl From<LedgerUsageRaw> for LedgerUsage {
    fn from(raw: LedgerUsageRaw) -> Self {
        Self {
            usage_source: raw.usage_source,
            usage_classification: raw.usage_classification,
            backend_instance: raw.backend_instance,
            provider: raw.provider,
            actual_model: raw.actual_model,
            actual_model_unknown_reason: raw.actual_model_unknown_reason,
            provider_unknown_reason: raw.provider_unknown_reason,
            account_label: raw.account_label,
            auth_source_label: raw.auth_source_label,
            quota_pool: raw.quota_pool,
            provider_attribution_source: raw.provider_attribution_source,
            pricing_source: raw.pricing_source,
            pricing_version: raw.pricing_version,
            cost_unknown_reason: raw.cost_unknown_reason,
            observed_at: raw.observed_at,
            input_tokens: raw.input_tokens,
            output_tokens: raw.output_tokens,
            reasoning_tokens: raw.reasoning_tokens,
            cache_read_tokens: raw.cache_read_tokens,
            cache_write_tokens: raw.cache_write_tokens,
            total_tokens: raw.total_tokens,
            requests_count: raw.requests_count,
            estimated_cost_usd: raw.estimated_cost_usd,
            actual_cost_usd: raw.actual_cost_usd,
            quota_window: raw.quota_window,
            quota_remaining_percent: raw
                .quota_remaining_percent
                .or_else(|| raw.quota_used_percent.map(|used| 100.0 - used)),
            quota_reset_at: raw.quota_reset_at,
            token_usage_unknown_reason: raw.token_usage_unknown_reason,
            quota_unknown_reason: raw.quota_unknown_reason,
            behavior_metrics: raw.behavior_metrics,
        }
    }
}
