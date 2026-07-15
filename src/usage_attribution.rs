use crate::ledger::{AttemptRecord, LedgerUsage};
use crate::routing::RouteDecision;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Copy)]
pub(crate) struct UsageAttribution<'a> {
    pub(crate) backend: Option<&'a str>,
    pub(crate) effective_model: Option<&'a str>,
    quota_pool: Option<&'a str>,
    cost_class: Option<&'a str>,
}

impl<'a> UsageAttribution<'a> {
    pub(crate) fn from_route(route: &'a RouteDecision) -> Self {
        Self {
            backend: Some(route.effective_backend.as_str()),
            effective_model: route.effective_model.as_deref(),
            quota_pool: route.effective_quota_pool.as_deref(),
            cost_class: route
                .routing_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.selected_cost_class.as_deref()),
        }
    }

    pub(crate) fn with_fallback_model(mut self, model: &'a str) -> Self {
        if self.effective_model.is_none() {
            self.effective_model = Some(model);
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn backend(backend: Option<&'a str>, effective_model: Option<&'a str>) -> Self {
        Self {
            backend,
            effective_model,
            quota_pool: None,
            cost_class: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn routed(
        backend: &'a str,
        effective_model: &'a str,
        quota_pool: &'a str,
        cost_class: &'a str,
    ) -> Self {
        Self {
            backend: Some(backend),
            effective_model: Some(effective_model),
            quota_pool: Some(quota_pool),
            cost_class: Some(cost_class),
        }
    }
}

fn provider_for_model(backend: Option<&str>, model: Option<&str>) -> Option<String> {
    let model = model.unwrap_or_default().to_ascii_lowercase();
    let inferred =
        if model.contains("claude") || model.contains("sonnet") || model.contains("haiku") {
            Some("anthropic")
        } else if model.contains("gemini") {
            Some("google")
        } else if model.contains("mistral") || model.contains("devstral") {
            Some("mistral")
        } else if model.contains("deepseek") {
            Some("deepseek")
        } else if model.contains("glm") || model.contains("z-ai") {
            Some("z-ai")
        } else if model.contains("hy3") || model.contains("tencent") {
            Some("tencent")
        } else if model.contains("gpt-") || model.contains("openai") {
            Some("openai")
        } else if model.contains("ollama") || model.contains("local/") {
            Some("local")
        } else {
            None
        };
    inferred.map(str::to_string).or_else(|| match backend {
        Some("claude") => Some("anthropic".to_string()),
        Some("codex") => Some("openai".to_string()),
        Some("vibe") => Some("mistral".to_string()),
        Some("agy" | "agy-main" | "agy-second") => Some("google".to_string()),
        _ => None,
    })
}

pub(crate) fn normalize_attempt_usage(
    mut usage: LedgerUsage,
    attribution: UsageAttribution<'_>,
    launched: bool,
) -> LedgerUsage {
    usage.backend_instance = attribution
        .backend
        .map(|backend| match attribution.quota_pool {
            Some(pool) => format!("{backend}:{pool}"),
            None => backend.to_string(),
        });
    usage.account_label = attribution.quota_pool.map(str::to_string);
    usage.usage_classification = match attribution.cost_class {
        Some("included_quota") => Some("quota_backed".to_string()),
        Some("paid") => Some("api_key_backed".to_string()),
        _ if attribution.backend == Some("opencode")
            && attribution
                .effective_model
                .is_some_and(|model| model.contains("ollama") || model.contains("local/")) =>
        {
            Some("local_unmetered".to_string())
        }
        _ => match attribution.backend {
            Some("claude" | "codex" | "vibe" | "agy" | "agy-main" | "agy-second") => {
                Some("quota_backed".to_string())
            }
            Some(_) => Some("unknown".to_string()),
            None => None,
        },
    };
    if usage.provider.is_none() {
        usage.provider = provider_for_model(attribution.backend, attribution.effective_model);
    }
    if usage.provider.is_none() {
        usage.provider_unknown_reason = Some(
            "backend artifact and configured model did not identify a model provider".to_string(),
        );
    }

    // AGY binds this label directly on the invocation and does not expose a
    // more specific model identity in successful CLI logs.
    if matches!(attribution.backend, Some("agy" | "agy-main" | "agy-second"))
        && usage.actual_model.is_none()
    {
        usage.actual_model = attribution.effective_model.map(str::to_string);
    }
    if usage.actual_model.is_none() {
        usage.actual_model_unknown_reason = Some(match attribution.effective_model {
            Some(model) => format!(
                "backend artifact did not report the actual model; invoked model was {model}"
            ),
            None => "neither the route nor backend artifact identified a model".to_string(),
        });
    }
    if matches!(attribution.backend, Some("agy" | "agy-main" | "agy-second")) {
        if usage.quota_window.is_none() {
            usage.quota_window = Some("AGY individual quota".to_string());
        }
        if usage.observed_at.is_none() {
            usage.observed_at = Some(
                OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .unwrap_or_default(),
            );
        }
    }
    if launched && usage.requests_count.is_none() {
        usage.requests_count = Some(1);
    }
    if usage.usage_source.is_none() {
        usage.usage_source = Some(if launched {
            "execution_observed".to_string()
        } else {
            "backend_launch_failed".to_string()
        });
    }
    if usage.input_tokens.is_none()
        && usage.output_tokens.is_none()
        && usage.reasoning_tokens.is_none()
        && usage.cache_read_tokens.is_none()
        && usage.cache_write_tokens.is_none()
        && usage.total_tokens.is_none()
    {
        usage.token_usage_unknown_reason =
            Some("run-scoped backend artifact did not expose exact token counters".to_string());
    } else {
        usage.token_usage_unknown_reason = None;
    }
    if usage.usage_classification.as_deref() == Some("quota_backed")
        && usage.quota_used_percent.is_none()
        && usage.quota_remaining_percent.is_none()
        && usage.quota_reset_at.is_none()
    {
        usage.quota_unknown_reason =
            Some("subscription backend did not expose exact per-execution quota state".to_string());
    } else {
        usage.quota_unknown_reason = None;
    }
    // Provider-reported API-equivalent costs from subscription CLIs are not
    // direct spend. Keep quota usage and API dollars separate even when the
    // transcript includes a hypothetical cost field.
    if usage.usage_classification.as_deref() == Some("quota_backed") {
        usage.actual_cost_usd = None;
        usage.estimated_cost_usd = None;
        usage.pricing_source = None;
        usage.pricing_version = None;
    }
    if usage.usage_classification.as_deref() == Some("local_unmetered")
        && usage.actual_cost_usd.is_none()
        && usage.estimated_cost_usd.is_none()
    {
        usage.actual_cost_usd = Some(0.0);
        usage.pricing_source = Some("local_unmetered".to_string());
    }
    if usage.actual_cost_usd.is_none() && usage.estimated_cost_usd.is_none() {
        usage.cost_unknown_reason = Some(match usage.usage_classification.as_deref() {
            Some("quota_backed") => {
                "subscription backend does not expose a defensible per-execution dollar cost"
                    .to_string()
            }
            Some("api_key_backed") => {
                "API usage or pricing data was insufficient to calculate exact cost".to_string()
            }
            Some("local_unmetered") => "local execution has no metered API charge".to_string(),
            _ => "usage classification is unknown, so cost cannot be attributed".to_string(),
        });
    }
    usage
}

pub(crate) fn usage_has_observation(usage: &LedgerUsage) -> bool {
    usage.usage_source.is_some()
        || usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.reasoning_tokens.is_some()
        || usage.cache_read_tokens.is_some()
        || usage.cache_write_tokens.is_some()
        || usage.total_tokens.is_some()
        || usage.requests_count.is_some()
        || usage.estimated_cost_usd.is_some()
        || usage.actual_cost_usd.is_some()
        || usage.quota_window.is_some()
        || usage.quota_used_percent.is_some()
        || usage.quota_remaining_percent.is_some()
        || usage.quota_reset_at.is_some()
}

enum AggregatedAttribution {
    Unknown,
    Agreed(String),
    Mixed,
    MixedOrUnknown,
}

fn aggregate_attribution<'a>(
    values: impl Iterator<Item = Option<&'a str>>,
) -> AggregatedAttribution {
    let values = values.collect::<Vec<_>>();
    let Some(first) = values.first().copied() else {
        return AggregatedAttribution::Unknown;
    };
    if values.iter().all(|value| *value == first) {
        return first
            .map(|value| AggregatedAttribution::Agreed(value.to_string()))
            .unwrap_or(AggregatedAttribution::Unknown);
    }
    if values.iter().any(|value| value.is_none()) {
        AggregatedAttribution::MixedOrUnknown
    } else {
        AggregatedAttribution::Mixed
    }
}

fn aggregate_label<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    match aggregate_attribution(values) {
        AggregatedAttribution::Unknown => None,
        AggregatedAttribution::Agreed(value) => Some(value),
        AggregatedAttribution::Mixed => Some("mixed".to_string()),
        AggregatedAttribution::MixedOrUnknown => Some("mixed_or_unknown".to_string()),
    }
}

pub(crate) fn aggregate_attempt_usage(attempts: &[AttemptRecord]) -> LedgerUsage {
    let mut aggregated = LedgerUsage::default();
    let observed = attempts
        .iter()
        .filter(|attempt| usage_has_observation(&attempt.usage))
        .map(|attempt| &attempt.usage)
        .collect::<Vec<_>>();
    if observed.is_empty() {
        return aggregated;
    }

    macro_rules! sum_optional {
        ($field:ident, $zero:expr) => {{
            let values = observed
                .iter()
                .filter_map(|usage| usage.$field)
                .collect::<Vec<_>>();
            if values.is_empty() {
                None
            } else {
                Some(values.into_iter().fold($zero, |sum, value| sum + value))
            }
        }};
    }
    aggregated.input_tokens = sum_optional!(input_tokens, 0_u64);
    aggregated.output_tokens = sum_optional!(output_tokens, 0_u64);
    aggregated.reasoning_tokens = sum_optional!(reasoning_tokens, 0_u64);
    aggregated.cache_read_tokens = sum_optional!(cache_read_tokens, 0_u64);
    aggregated.cache_write_tokens = sum_optional!(cache_write_tokens, 0_u64);
    aggregated.total_tokens = sum_optional!(total_tokens, 0_u64);
    aggregated.requests_count = sum_optional!(requests_count, 0_u64);
    aggregated.estimated_cost_usd = sum_optional!(estimated_cost_usd, 0.0_f64);
    aggregated.actual_cost_usd = sum_optional!(actual_cost_usd, 0.0_f64);

    if let Some(latest) = observed
        .iter()
        .filter(|usage| usage.observed_at.is_some())
        .max_by_key(|usage| usage.observed_at.as_deref())
    {
        aggregated.observed_at = latest.observed_at.clone();
        aggregated.quota_window = latest.quota_window.clone();
        aggregated.quota_used_percent = latest.quota_used_percent;
        aggregated.quota_remaining_percent = latest.quota_remaining_percent;
        aggregated.quota_reset_at = latest.quota_reset_at.clone();
    }

    aggregated.usage_classification = aggregate_label(
        observed
            .iter()
            .map(|usage| usage.usage_classification.as_deref()),
    );
    aggregated.backend_instance = aggregate_label(
        observed
            .iter()
            .map(|usage| usage.backend_instance.as_deref()),
    );
    aggregated.provider = aggregate_label(observed.iter().map(|usage| usage.provider.as_deref()));
    aggregated.account_label =
        aggregate_label(observed.iter().map(|usage| usage.account_label.as_deref()));
    aggregated.pricing_source =
        aggregate_label(observed.iter().map(|usage| usage.pricing_source.as_deref()));
    aggregated.pricing_version = aggregate_label(
        observed
            .iter()
            .map(|usage| usage.pricing_version.as_deref()),
    );
    match aggregate_attribution(observed.iter().map(|usage| usage.actual_model.as_deref())) {
        AggregatedAttribution::Agreed(model) => aggregated.actual_model = Some(model),
        AggregatedAttribution::Mixed => {
            aggregated.actual_model_unknown_reason =
                Some("attempts used different actual models".to_string());
        }
        AggregatedAttribution::MixedOrUnknown => {
            aggregated.actual_model_unknown_reason =
                Some("one or more attempts did not report an actual model".to_string());
        }
        AggregatedAttribution::Unknown => {
            aggregated.actual_model_unknown_reason = aggregate_label(
                observed
                    .iter()
                    .map(|usage| usage.actual_model_unknown_reason.as_deref()),
            );
        }
    }
    if aggregated.provider.is_none() {
        aggregated.provider_unknown_reason = aggregate_label(
            observed
                .iter()
                .map(|usage| usage.provider_unknown_reason.as_deref()),
        );
    }
    aggregated.cost_unknown_reason = if observed.iter().any(|usage| {
        usage.actual_cost_usd.is_none()
            && usage.estimated_cost_usd.is_none()
            && usage.cost_unknown_reason.is_some()
    }) {
        aggregate_label(
            observed
                .iter()
                .map(|usage| usage.cost_unknown_reason.as_deref()),
        )
    } else {
        None
    };
    aggregated.token_usage_unknown_reason = aggregate_label(
        observed
            .iter()
            .map(|usage| usage.token_usage_unknown_reason.as_deref()),
    );
    aggregated.quota_unknown_reason = aggregate_label(
        observed
            .iter()
            .map(|usage| usage.quota_unknown_reason.as_deref()),
    );
    aggregated.usage_source = Some("attempt_aggregate".to_string());
    aggregated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_route_records_safe_instance_and_api_classification() {
        let usage = normalize_attempt_usage(
            LedgerUsage::default(),
            UsageAttribution::routed(
                "opencode",
                "nous-portal/z-ai/glm-5.2",
                "nous-portal-api",
                "paid",
            ),
            true,
        );

        assert_eq!(
            usage.usage_classification.as_deref(),
            Some("api_key_backed")
        );
        assert_eq!(
            usage.backend_instance.as_deref(),
            Some("opencode:nous-portal-api")
        );
        assert_eq!(usage.account_label.as_deref(), Some("nous-portal-api"));
        assert_eq!(usage.provider.as_deref(), Some("z-ai"));
        assert!(usage.actual_model.is_none());
        assert!(usage.actual_model_unknown_reason.is_some());
        assert!(usage.cost_unknown_reason.is_some());
    }

    #[test]
    fn paid_route_keeps_provider_reported_exact_cost() {
        let usage = normalize_attempt_usage(
            LedgerUsage {
                actual_cost_usd: Some(0.01825),
                pricing_source: Some("opencode_session_provider_reported".into()),
                input_tokens: Some(100),
                output_tokens: Some(20),
                ..LedgerUsage::default()
            },
            UsageAttribution::routed(
                "opencode",
                "nous-portal/z-ai/glm-5.2",
                "nous-portal-api",
                "paid",
            ),
            true,
        );

        assert_eq!(usage.actual_cost_usd, Some(0.01825));
        assert!(usage.cost_unknown_reason.is_none());
    }

    #[test]
    fn quota_route_never_reports_api_equivalent_transcript_cost_as_spend() {
        let usage = normalize_attempt_usage(
            LedgerUsage {
                actual_cost_usd: Some(12.34),
                pricing_source: Some("backend_api_equivalent".into()),
                input_tokens: Some(100),
                output_tokens: Some(20),
                ..LedgerUsage::default()
            },
            UsageAttribution::routed("claude", "sonnet", "claude-main", "included_quota"),
            true,
        );

        assert_eq!(usage.actual_cost_usd, None);
        assert_eq!(usage.estimated_cost_usd, None);
        assert_eq!(usage.pricing_source, None);
        assert!(usage
            .cost_unknown_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("subscription")));
    }

    #[test]
    fn aggregate_preserves_agreement_and_labels_mixed_attribution() {
        let matching_usage = LedgerUsage {
            usage_source: Some("fixture".into()),
            usage_classification: Some("quota_backed".into()),
            backend_instance: Some("vibe:vibe-monthly".into()),
            provider: Some("mistral".into()),
            actual_model: Some("devstral-small".into()),
            input_tokens: Some(10),
            ..LedgerUsage::default()
        };
        let matching = vec![
            AttemptRecord {
                usage: matching_usage.clone(),
                ..AttemptRecord::default()
            },
            AttemptRecord {
                usage: matching_usage,
                ..AttemptRecord::default()
            },
        ];
        let aggregate = aggregate_attempt_usage(&matching);
        assert_eq!(aggregate.input_tokens, Some(20));
        assert_eq!(
            aggregate.usage_classification.as_deref(),
            Some("quota_backed")
        );
        assert_eq!(
            aggregate.backend_instance.as_deref(),
            Some("vibe:vibe-monthly")
        );
        assert_eq!(aggregate.provider.as_deref(), Some("mistral"));
        assert_eq!(aggregate.actual_model.as_deref(), Some("devstral-small"));

        let mixed = vec![
            matching[0].clone(),
            AttemptRecord {
                usage: LedgerUsage {
                    usage_source: Some("fixture".into()),
                    usage_classification: Some("api_key_backed".into()),
                    backend_instance: Some("opencode:nous-portal-api".into()),
                    provider: Some("z-ai".into()),
                    actual_model: Some("glm-5.2".into()),
                    input_tokens: Some(5),
                    ..LedgerUsage::default()
                },
                ..AttemptRecord::default()
            },
        ];
        let aggregate = aggregate_attempt_usage(&mixed);
        assert_eq!(aggregate.input_tokens, Some(15));
        assert_eq!(aggregate.usage_classification.as_deref(), Some("mixed"));
        assert_eq!(aggregate.backend_instance.as_deref(), Some("mixed"));
        assert_eq!(aggregate.provider.as_deref(), Some("mixed"));
        assert!(aggregate.actual_model.is_none());
        assert_eq!(
            aggregate.actual_model_unknown_reason.as_deref(),
            Some("attempts used different actual models")
        );
    }

    #[test]
    fn local_route_records_known_zero_cost_not_missing_cost() {
        let usage = normalize_attempt_usage(
            LedgerUsage::default(),
            UsageAttribution::routed(
                "opencode",
                "ollama/deepseek-local",
                "local-host",
                "standard",
            ),
            true,
        );

        assert_eq!(
            usage.usage_classification.as_deref(),
            Some("local_unmetered")
        );
        assert_eq!(usage.actual_cost_usd, Some(0.0));
        assert_eq!(usage.pricing_source.as_deref(), Some("local_unmetered"));
        assert!(usage.cost_unknown_reason.is_none());
    }
}
