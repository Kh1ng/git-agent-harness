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

/// #155 (TICKET-066 / #151): parse AGY's own quota/reset messages from a
/// **run-scoped cli.log delta** (the bytes `runner` captured between the
/// pre-run byte offset and the post-run position -- never a fresh read of
/// the whole log).
///
/// AGY emits real lines such as:
///   "RESOURCE_EXHAUSTED: ... Individual quota reached ..."
///   "Quota exceeded. Resets in 16m44s."
///   "Your quota resets at 2026-07-10 12:34:56 UTC."
///   "Daily limit reached. Resets in 3h12m."
///
/// We extract the human-facing reset description only. We deliberately do
/// **not** invent `quota_used_percent`: per the owner's explicit spec it
/// stays `None` unless a real AGY endpoint exposes an exact percentage
/// (none has been discovered). Fabricating a percentage would be a worse
/// bug than leaving it unknown.
///
/// An empty/absent delta yields an all-`None` `LedgerUsage` (unknown,
/// never fabricated zero) so the caller can merge it safely.
pub fn parse_agy_cli_log_delta(delta: &str, source_hint: &str) -> LedgerUsage {
    let mut usage = LedgerUsage::default();

    // AGY-wide exhaustion / quota-exceeded signals.
    let quota_exhausted = delta.contains("RESOURCE_EXHAUSTED")
        || delta.contains("Individual quota reached")
        || delta.contains("Quota exceeded")
        || delta.contains("quota has been reached")
        || delta.contains("quota reached");
    usage.quota_window = if quota_exhausted {
        Some("AGY individual quota".to_string())
    } else {
        None
    };

    // Reset description. Prefer an explicit timestamp; otherwise capture the
    // "Resets in <dur>" relative form. Real AGY lines ("Resets in 16m44s.",
    // "Your quota resets at 2026-07-10 12:34:56 UTC.") do NOT use a `:`
    // separator, so we use a tolerant finder that accepts an optional `:`/`:`
    // and bounds the capture. Never synthesize a percentage.
    if let Some(ts) = agy_find_after(delta, &["resets at", "resets:"]) {
        usage.quota_reset_at = Some(ts);
    } else if let Some(dur) = agy_find_after(delta, &["resets in", "reset in"]) {
        usage.quota_reset_at = Some(format!("in {dur}"));
    }

    if usage.quota_window.is_some() || usage.quota_reset_at.is_some() {
        usage.usage_source = Some(source_hint.to_string());
    }
    usage
}

/// Like `find_string_after`, but tolerant of AGY's separator-less style
/// ("Resets in 16m44s" / "quota resets at 2026-...") where the value follows
/// the keyword with only optional whitespace (an optional `:`/`=` is allowed).
/// Reuses the same length/shape guards as the generic version.
fn agy_find_after(text: &str, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        // Stop at the first period (a reset description "...resets at X." is
        // one sentence) so we don't swallow the rest of the log line. The
        // generic `find_string_after`'s `[^\n\r]+` would grab everything to
        // end-of-line, which is too greedy for AGY's prose-style messages.
        let re = Regex::new(&format!(
            r"(?i)\b{}\b\s*:?\s*([^.\n\r]{{1,{}}})",
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

/// Merge `other` into `base`, keeping the first `Some` value for each field
/// (so the generic stdout parse wins over the cli.log delta when both
/// report the same field, and the delta fills in quota/reset info the
/// stdout parse doesn't have). Returns a new `LedgerUsage`.
pub fn merge_usage(base: LedgerUsage, other: LedgerUsage) -> LedgerUsage {
    LedgerUsage {
        input_tokens: base.input_tokens.or(other.input_tokens),
        output_tokens: base.output_tokens.or(other.output_tokens),
        cache_read_tokens: base.cache_read_tokens.or(other.cache_read_tokens),
        cache_write_tokens: base.cache_write_tokens.or(other.cache_write_tokens),
        total_tokens: base.total_tokens.or(other.total_tokens),
        requests_count: base.requests_count.or(other.requests_count),
        estimated_cost_usd: base.estimated_cost_usd.or(other.estimated_cost_usd),
        actual_cost_usd: base.actual_cost_usd.or(other.actual_cost_usd),
        quota_used_percent: base.quota_used_percent.or(other.quota_used_percent),
        quota_remaining_percent: base
            .quota_remaining_percent
            .or(other.quota_remaining_percent),
        quota_window: base.quota_window.or(other.quota_window),
        quota_reset_at: base.quota_reset_at.or(other.quota_reset_at),
        usage_source: match (base.usage_source, other.usage_source) {
            (Some(a), Some(b)) => Some(format!("{a}+{b}")),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        },
        observed_at: base.observed_at.or(other.observed_at),
    }
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

/// Parse JSONL output from `codex exec --json`.
/// Scans for `turn.completed` events and aggregates their usage data into
/// a `LedgerUsage` struct. Returns an empty (all-`None`) `LedgerUsage` when
/// no structured usage data is found — callers distinguish "no JSON events"
/// from "parsed successfully" by checking `usage_source`.
pub fn parse_codex_exec_json(output: &str) -> LedgerUsage {
    let mut input_tokens: Option<u64> = None;
    let mut output_tokens: Option<u64> = None;
    let mut cache_read_tokens: Option<u64> = None;
    let mut turns_found = 0u64;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["type"].as_str() != Some("turn.completed") {
            continue;
        }
        let Some(usage_obj) = value.get("usage") else {
            continue;
        };

        turns_found += 1;

        if let Some(v) = usage_obj.get("input_tokens").and_then(|v| v.as_u64()) {
            input_tokens = Some(input_tokens.unwrap_or(0) + v);
        }
        if let Some(v) = usage_obj.get("output_tokens").and_then(|v| v.as_u64()) {
            output_tokens = Some(output_tokens.unwrap_or(0) + v);
        }
        if let Some(v) = usage_obj
            .get("cached_input_tokens")
            .and_then(|v| v.as_u64())
        {
            cache_read_tokens = Some(cache_read_tokens.unwrap_or(0) + v);
        }
        // reasoning_output_tokens are billed as output tokens — add them in
        if let Some(v) = usage_obj
            .get("reasoning_output_tokens")
            .and_then(|v| v.as_u64())
        {
            output_tokens = Some(output_tokens.unwrap_or(0) + v);
        }
    }

    if turns_found == 0 {
        return LedgerUsage::default();
    }

    let mut usage = LedgerUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        ..LedgerUsage::default()
    };

    usage.total_tokens = match (usage.input_tokens, usage.output_tokens) {
        (Some(input), Some(output)) => Some(input + output),
        _ => usage.total_tokens,
    };
    usage.requests_count = Some(turns_found);
    usage.usage_source = Some("codex_exec_json".to_string());

    usage
}

/// Parse JSON output from `codex status --json`.
/// Extracts rate-limit and quota data (primary/secondary windows, reset
/// timestamps) into the quota fields of `LedgerUsage`. Returns an empty
/// (all-`None`) `LedgerUsage` when the payload does not contain a
/// `rateLimits` object.
#[allow(dead_code)]
pub fn parse_codex_status_json(output: &str) -> LedgerUsage {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(output) else {
        return LedgerUsage::default();
    };

    let Some(rate_limits) = root.get("rateLimits") else {
        return LedgerUsage::default();
    };

    let mut usage = LedgerUsage::default();
    let mut has_data = false;

    if let Some(primary) = rate_limits.get("primary") {
        if let Some(pct) = primary.get("usedPercent").and_then(|v| v.as_f64()) {
            usage.quota_used_percent = Some(pct);
            has_data = true;
        }
        if let Some(mins) = primary.get("windowDurationMins").and_then(|v| v.as_u64()) {
            usage.quota_window = Some(format!("{}m", mins));
            has_data = true;
        }
        if let Some(ts) = primary.get("resetsAt").and_then(|v| v.as_i64()) {
            if let Ok(dt) = time::OffsetDateTime::from_unix_timestamp(ts) {
                if let Ok(formatted) = dt.format(&time::format_description::well_known::Rfc3339) {
                    usage.quota_reset_at = Some(formatted);
                    has_data = true;
                }
            }
        }
    }

    // `quota_remaining_percent` must be the complement of `quota_used_percent`
    // for the *same* window (primary) -- it previously read `secondary`'s
    // usedPercent instead, silently mixing a different quota window (e.g.
    // Codex's 5h primary vs its weekly secondary) into what looked like one
    // consistent used/remaining pair. `secondary` isn't represented in this
    // single-row `LedgerUsage` shape yet; a real per-window breakdown is
    // issue #166's job when it wires this into a live refresh path.
    if let Some(pct) = usage.quota_used_percent {
        usage.quota_remaining_percent = Some(100.0 - pct);
    }

    if has_data {
        usage.usage_source = Some("codex_status_json".to_string());
    }

    usage
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
    use super::parse_agy_cli_log_delta;
    use super::parse_codex_exec_json;
    use super::parse_codex_status_json;
    use super::parse_generic_usage;

    // ── codex exec --json (Issue #152) ───────────────────────────────────

    const CODEX_EXEC_JSON: &str = include_str!("../tests/fixtures/codex-exec-json.jsonl");

    #[test]
    fn codex_exec_json_aggregates_usage_across_turns() {
        let usage = parse_codex_exec_json(CODEX_EXEC_JSON);
        assert_eq!(usage.input_tokens, Some(14230 + 5200));
        assert_eq!(usage.output_tokens, Some(2150 + 890 + 340));
        assert_eq!(usage.cache_read_tokens, Some(11800));
        assert_eq!(usage.total_tokens, Some(14230 + 5200 + 2150 + 890 + 340));
        assert_eq!(usage.requests_count, Some(2));
        assert_eq!(usage.usage_source.as_deref(), Some("codex_exec_json"));
    }

    #[test]
    fn codex_exec_json_returns_empty_for_non_json_output() {
        let text = "some plain text\ninput_tokens: 500\n";
        let usage = parse_codex_exec_json(text);
        assert_eq!(usage.usage_source, None);
        assert_eq!(usage.input_tokens, None);
    }

    #[test]
    fn codex_exec_json_returns_empty_for_unrelated_json() {
        let text = r#"{"type":"item.agent_message","content":"hello"}"#;
        let usage = parse_codex_exec_json(text);
        assert_eq!(usage.usage_source, None);
    }

    #[test]
    fn codex_exec_json_returns_empty_for_empty_input() {
        assert_eq!(parse_codex_exec_json("").usage_source, None);
        assert_eq!(parse_codex_exec_json("\n\n").usage_source, None);
    }

    // ── codex status --json (Issue #152) ─────────────────────────────────

    const CODEX_STATUS_JSON: &str = include_str!("../tests/fixtures/codex-status-json.json");

    #[test]
    fn codex_status_json_extracts_quota_fields() {
        let usage = parse_codex_status_json(CODEX_STATUS_JSON);
        assert_eq!(usage.quota_used_percent, Some(25.0));
        // Must be the complement of quota_used_percent for the SAME
        // (primary) window, not a mix-in of secondary's usedPercent.
        assert_eq!(usage.quota_remaining_percent, Some(75.0));
        assert_eq!(usage.quota_window.as_deref(), Some("300m"));
        // 1777534802 -> 2026-04-29-ish (UTC)
        assert!(usage.quota_reset_at.is_some());
        assert_eq!(usage.usage_source.as_deref(), Some("codex_status_json"));
    }

    #[test]
    fn codex_status_json_returns_empty_for_non_json_input() {
        let usage = parse_codex_status_json("not json at all");
        assert_eq!(usage.usage_source, None);
    }

    #[test]
    fn codex_status_json_returns_empty_for_missing_rate_limits() {
        let usage = parse_codex_status_json(r#"{"some":"data"}"#);
        assert_eq!(usage.usage_source, None);
    }

    // ── Existing generic parser tests ────────────────────────────────────

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

    // --- #155: AGY cli.log delta parsing (TICKET-066 / #151) -----------------

    #[test]
    fn agy_delta_parses_relative_reset_when_quota_exhausted() {
        // Real AGY lines do NOT use a `:` separator ("Resets in 16m44s.").
        let delta = "ERROR: RESOURCE_EXHAUSTED: Individual quota reached for this account.\nQuota exceeded. Resets in 16m44s.";
        let usage = parse_agy_cli_log_delta(delta, "agy_cli_log_delta");
        assert_eq!(usage.quota_window.as_deref(), Some("AGY individual quota"));
        assert_eq!(usage.quota_reset_at.as_deref(), Some("in 16m44s"));
        // Critical spec point: percentage is never fabricated.
        assert_eq!(usage.quota_used_percent, None);
        assert_eq!(usage.usage_source.as_deref(), Some("agy_cli_log_delta"));
    }

    #[test]
    fn agy_delta_parses_explicit_reset_timestamp() {
        let delta = "Your quota resets at 2026-07-10 12:34:56 UTC. Please try again after that.";
        let usage = parse_agy_cli_log_delta(delta, "agy_cli_log_delta");
        assert_eq!(
            usage.quota_reset_at.as_deref(),
            Some("2026-07-10 12:34:56 UTC")
        );
        assert_eq!(usage.quota_used_percent, None);
    }

    #[test]
    fn agy_delta_leaves_percentage_unknown_when_absent() {
        // Even when other quota signals are present, percentage stays None.
        let delta = "Quota exceeded. Daily limit reached. Resets in 3h12m. You have used 80% of your quota.";
        let usage = parse_agy_cli_log_delta(delta, "agy_cli_log_delta");
        assert!(usage.quota_window.is_some());
        assert!(usage.quota_reset_at.is_some());
        // The free-text "80%" is NOT a structured percentage source; we must
        // not guess/estimate a number from prose.
        assert_eq!(usage.quota_used_percent, None);
    }

    #[test]
    fn agy_delta_is_empty_not_zero_for_non_quota_runs() {
        // A successful AGY run with no quota signal must report unknown,
        // never a fabricated zero.
        let delta = "agent started\nmade some edits\nagent finished";
        let usage = parse_agy_cli_log_delta(delta, "agy_cli_log_delta");
        assert_eq!(usage.quota_window, None);
        assert_eq!(usage.quota_reset_at, None);
        assert_eq!(usage.quota_used_percent, None);
        assert_eq!(usage.usage_source, None);
    }

    #[test]
    fn agy_delta_empty_string_yields_all_unknown() {
        let usage = parse_agy_cli_log_delta("", "agy_cli_log_delta");
        assert_eq!(usage.quota_window, None);
        assert_eq!(usage.quota_reset_at, None);
        assert_eq!(usage.quota_used_percent, None);
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.usage_source, None);
    }
}
