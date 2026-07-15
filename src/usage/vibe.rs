use crate::ledger::LedgerUsage;
use serde_json::Value;

/// Parse the exact durable metadata written by Mistral Vibe for one
/// programmatic session. `active_model` is an alias; resolve it through the
/// same session snapshot's model table before calling it the actual model.
pub fn parse_vibe_session_metadata(metadata_json: &str) -> LedgerUsage {
    let Ok(root) = serde_json::from_str::<Value>(metadata_json) else {
        return LedgerUsage::default();
    };
    let stats = root.get("stats").unwrap_or(&Value::Null);
    let input_tokens = stats.get("session_prompt_tokens").and_then(Value::as_u64);
    let output_tokens = stats
        .get("session_completion_tokens")
        .and_then(Value::as_u64);
    let total_tokens = match (input_tokens, output_tokens) {
        (Some(input), Some(output)) => Some(input.saturating_add(output)),
        _ => stats
            .get("session_total_llm_tokens")
            .and_then(Value::as_u64),
    };
    let counters = [input_tokens, output_tokens, total_tokens];
    let counters_observed = counters.iter().any(Option::is_some);
    let model_was_invoked = !counters_observed || counters.into_iter().flatten().any(|n| n > 0);
    let (configured_model, provider) = resolve_configured_model(&root);
    let actual_model = model_was_invoked
        .then_some(configured_model)
        .flatten()
        .or_else(|| {
            model_was_invoked
                .then(|| root.get("model").and_then(Value::as_str))
                .flatten()
                .map(str::to_string)
        });
    let requests_count = if counters_observed && !model_was_invoked {
        Some(0)
    } else {
        stats.get("steps").and_then(Value::as_u64)
    };
    let observed_at = root
        .get("end_time")
        .and_then(Value::as_str)
        .or_else(|| root.get("start_time").and_then(Value::as_str))
        .map(str::to_string);

    if input_tokens.is_none()
        && output_tokens.is_none()
        && total_tokens.is_none()
        && requests_count.is_none()
        && actual_model.is_none()
        && provider.is_none()
    {
        return LedgerUsage::default();
    }

    LedgerUsage {
        usage_source: Some("vibe_session_metadata".to_string()),
        actual_model,
        provider,
        observed_at,
        input_tokens,
        output_tokens,
        total_tokens,
        requests_count,
        ..LedgerUsage::default()
    }
}

fn resolve_configured_model(root: &Value) -> (Option<String>, Option<String>) {
    let Some(config) = root.get("config") else {
        return (None, None);
    };
    let Some(active) = config.get("active_model").and_then(Value::as_str) else {
        return (None, None);
    };
    let selected = config
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models.iter().find(|model| {
                model.get("alias").and_then(Value::as_str) == Some(active)
                    || model.get("name").and_then(Value::as_str) == Some(active)
            })
        });
    let model = selected
        .and_then(|model| model.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(active)
        .to_string();
    let provider = selected
        .and_then(|model| model.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (Some(model), provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_active_alias_to_provider_model_name() {
        let usage = parse_vibe_session_metadata(
            r#"{
              "config":{"active_model":"mistral-medium-3.5","models":[{"name":"mistral-vibe-cli-latest","provider":"mistral","alias":"mistral-medium-3.5"}]},
              "stats":{"steps":1,"session_prompt_tokens":100,"session_completion_tokens":20}
            }"#,
        );
        assert_eq!(
            usage.actual_model.as_deref(),
            Some("mistral-vibe-cli-latest")
        );
        assert_eq!(usage.provider.as_deref(), Some("mistral"));
    }

    #[test]
    fn zero_token_auth_failure_does_not_claim_alias_was_used() {
        let usage = parse_vibe_session_metadata(
            r#"{
              "config":{"active_model":"mistral-medium-3.5","models":[{"name":"mistral-vibe-cli-latest","provider":"mistral","alias":"mistral-medium-3.5"}]},
              "stats":{"steps":1,"session_prompt_tokens":0,"session_completion_tokens":0,"session_total_llm_tokens":0}
            }"#,
        );
        assert_eq!(usage.actual_model, None);
        assert_eq!(usage.provider.as_deref(), Some("mistral"));
        assert_eq!(usage.requests_count, Some(0));
        assert_eq!(usage.total_tokens, Some(0));
    }
}
