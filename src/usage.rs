use crate::ledger::LedgerUsage;
use regex::Regex;

/// Parse generic usage text from backend output logs.
/// Uses word boundaries to prevent matching partial words like "my_input_tokens_value".
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
        || usage.cache_read_tokens.is_some()
        || usage.cache_write_tokens.is_some()
        || usage.requests_count.is_some()
        || usage.quota_used_percent.is_some()
        || usage.quota_remaining_percent.is_some()
        || usage.quota_window.is_some()
        || usage.quota_reset_at.is_some()
    {
        usage.usage_source = Some(source_hint.to_string());
    }
    usage
}

fn find_u64(text: &str, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        // Use word boundaries to prevent matching partial words like "my_input_tokens_value"
        let re = Regex::new(&format!(
            r"(?i)\b{}\b\s*[:=]\s*([0-9]+)",
            regex::escape(key)
        ))
        .ok()?;
        re.captures(text)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse::<u64>().ok())
    })
}

fn find_f64(text: &str, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        // Use word boundaries to prevent matching partial words
        let re = Regex::new(&format!(
            r"(?i)\b{}\b\s*[:=]\s*([0-9]+(?:\.[0-9]+)?)",
            regex::escape(key)
        ))
        .ok()?;
        re.captures(text)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse::<f64>().ok())
    })
}

/// A real quota-window/quota-reset-at value is a short human string ("weekly",
/// "5h", an ISO timestamp). `[^\n\r]+` alone is unbounded and backend log
/// text is not always newline-delimited per logical line (e.g. a diff or
/// source snippet dumped with literal `\n` escapes rather than real
/// newline bytes) -- if that text happens to contain the literal substring
/// "quota_window" (a backend session working on this very field, dogfooding
/// GAH on itself, will print exactly that), this used to capture hundreds
/// of bytes of unrelated source code as the "value". Bound the capture and
/// reject anything that still looks code-shaped rather than data-shaped.
const MAX_QUOTA_STRING_LEN: usize = 64;
/// Regex capture bound, deliberately larger than MAX_QUOTA_STRING_LEN so an
/// overly-long value is actually captured (and then rejected by
/// `looks_like_quota_value`'s length check) instead of being silently
/// truncated down to a length that passes.
const MAX_QUOTA_CAPTURE_LEN: usize = 256;

fn looks_like_quota_value(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_QUOTA_STRING_LEN {
        return false;
    }
    !["{", "}", "<", ">", "::", "#[", "\\n", "pub ", "fn "]
        .iter()
        .any(|marker| s.contains(marker))
}

fn find_string_after(text: &str, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        // Use word boundaries to prevent matching partial words
        let re = Regex::new(&format!(
            r"(?i)\b{}\b\s*[:=]\s*([^\n\r]{{1,{}}})",
            regex::escape(key),
            MAX_QUOTA_CAPTURE_LEN
        ))
        .ok()?;
        re.captures(text)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().trim().trim_matches('"').to_string())
            .filter(|value| looks_like_quota_value(value))
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

    #[test]
    fn rejects_code_shaped_text_masquerading_as_a_quota_value() {
        // Regression: a backend session dogfooding GAH's own quota_window
        // field printed its own source (with literal `\n` escapes, not real
        // newline bytes, so it reads as one line to the parser) and the old
        // unbounded `[^\n\r]+` capture grabbed hundreds of bytes of it.
        let text = "quota_window: Option<String>,\\n    pub quota_used_percent: Option<f64>,\\n}";
        let usage = parse_generic_usage(text, "generic");
        assert_eq!(usage.quota_window, None);
    }

    #[test]
    fn rejects_overly_long_captures_even_without_code_markers() {
        let long_value = "x".repeat(200);
        let text = format!("quota_reset_at: {}", long_value);
        let usage = parse_generic_usage(&text, "generic");
        assert_eq!(usage.quota_reset_at, None);
    }
}
