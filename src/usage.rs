use crate::ledger::LedgerUsage;
use regex::Regex;

pub fn parse_generic_usage(text: &str, source_hint: &str) -> LedgerUsage {
    let input_tokens = find_u64(text, &["input_tokens", "input tokens", "prompt_tokens"]);
    let output_tokens = find_u64(
        text,
        &["output_tokens", "output tokens", "completion_tokens"],
    );
    let cache_read_tokens = find_u64(text, &["cache_read_tokens", "cache read tokens"]);
    let cache_write_tokens = find_u64(text, &["cache_write_tokens", "cache write tokens"]);
    let total_tokens = find_u64(text, &["total_tokens", "total tokens"]);
    let requests_count = find_u64(text, &["requests_count", "requests count"]);
    let estimated_cost_usd = find_f64(
        text,
        &["estimated_cost_usd", "estimated cost usd", "cost usd"],
    );
    let actual_cost_usd = find_f64(text, &["actual_cost_usd", "actual cost usd"]);
    let quota_used_percent = find_f64(text, &["quota_used_percent", "quota used percent"]);
    let quota_remaining_percent = find_f64(
        text,
        &["quota_remaining_percent", "quota remaining percent"],
    );
    let quota_window = find_string_after(text, &["quota_window", "quota window"]);
    let quota_reset_at = find_string_after(text, &["quota_reset_at", "quota reset at"]);

    let mut usage = LedgerUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens,
        requests_count,
        estimated_cost_usd,
        actual_cost_usd,
        quota_used_percent,
        quota_remaining_percent,
        quota_window,
        quota_reset_at,
        ..LedgerUsage::default()
    };

    if usage.total_tokens.is_none() {
        usage.total_tokens = match (usage.input_tokens, usage.output_tokens) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        };
    }
    if usage.requests_count.is_none()
        && (usage.input_tokens.is_some()
            || usage.output_tokens.is_some()
            || usage.total_tokens.is_some())
    {
        usage.requests_count = Some(1);
    }
    if usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.total_tokens.is_some()
        || usage.estimated_cost_usd.is_some()
        || usage.actual_cost_usd.is_some()
    {
        usage.usage_source = Some(source_hint.to_string());
    }
    usage
}

fn find_u64(text: &str, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        let re = Regex::new(&format!(r"(?i){}\s*[:=]\s*([0-9]+)", regex::escape(key))).ok()?;
        re.captures(text)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse::<u64>().ok())
    })
}

fn find_f64(text: &str, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let re = Regex::new(&format!(
            r"(?i){}\s*[:=]\s*([0-9]+(?:\.[0-9]+)?)",
            regex::escape(key)
        ))
        .ok()?;
        re.captures(text)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse::<f64>().ok())
    })
}

fn find_string_after(text: &str, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let re = Regex::new(&format!(r"(?i){}\s*[:=]\s*([^\n\r]+)", regex::escape(key))).ok()?;
        re.captures(text)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().trim().trim_matches('"').to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::parse_generic_usage;

    #[test]
    fn parses_basic_usage_fields() {
        let usage = parse_generic_usage(
            "input_tokens: 10\noutput_tokens: 20\nestimated_cost_usd: 0.12\nquota_window: weekly",
            "generic",
        );
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(30));
        assert_eq!(usage.estimated_cost_usd, Some(0.12));
        assert_eq!(usage.quota_window.as_deref(), Some("weekly"));
    }
}
