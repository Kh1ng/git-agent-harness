//! Backend-output extraction for human-facing commit and pull-request prose.
//!
//! Raw backend logs remain available as session artifacts and telemetry input.
//! This module is the narrower trust boundary for text that may be published
//! to a repository.

use serde_json::Value;
use std::fs;

const RAW_TAIL_BYTES: usize = 20_000;
const SUMMARY_MAX_BYTES: usize = 2_000;
const FALLBACK_CONTEXT_MAX_BYTES: usize = 300;

fn utf8_safe_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn utf8_safe_suffix(value: &str, max_bytes: usize) -> &str {
    let mut start = value.len().saturating_sub(max_bytes);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn is_test_runner_noise_line(line: &str) -> bool {
    let patterns = [
        r"^running \d+ tests?$",
        r"^test \S.*\.\.\. (ok|FAILED|ignored)$",
        r"^test result: (ok|FAILED)\.",
        r"^\$ \S",
        r"^onl; \S",
        r"^Doc-tests \S",
        r"^\d+ \w+(, \d+ \w+)* in [\d.]+s\b",
        r"^(Passed!|Failed!|Test Run (Successful|Failed))\b",
    ];
    patterns
        .iter()
        .any(|pattern| regex::Regex::new(pattern).unwrap().is_match(line))
}

fn strip_terminal_noise(text: &str) -> String {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    let osc = regex::Regex::new(r"\x1b\].*?(?:\x07|\x1b\\)").unwrap();
    let ansi_text = ansi.replace_all(text, "");
    let without_ansi = osc.replace_all(&ansi_text, "");

    without_ansi
        .lines()
        .map(|line| line.trim_matches(['│', '╭', '╮', '╰', '╯', '─', ' ']))
        .filter(|line| {
            !(line.is_empty()
                || *line == "Goodbye! 👋"
                || line.starts_with("Conversation ID:")
                || line.contains("openhands --resume")
                || line.contains("resume this")
                || *line == "conversation."
                || is_test_runner_noise_line(line))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_codex_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "thread.started"
            | "turn.started"
            | "turn.completed"
            | "turn.failed"
            | "item.started"
            | "item.updated"
            | "item.completed"
            | "error"
    )
}

/// Return `None` for an ordinary text log. Once a Codex JSONL event is seen,
/// return a safe summary or a deterministic fallback; raw event lines are
/// never allowed to fall through to plain-text tail extraction.
fn extract_codex_jsonl_summary(text: &str, fallback_context: Option<&str>) -> Option<String> {
    let mut saw_codex_event = false;
    let mut pending_agent_message = None;
    let mut completed_agent_message = None;

    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !is_codex_event_type(event_type) {
            continue;
        }
        saw_codex_event = true;

        match event_type {
            "item.completed"
                if event.pointer("/item/type").and_then(Value::as_str) == Some("agent_message") =>
            {
                pending_agent_message = event
                    .pointer("/item/text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|message| !message.is_empty())
                    .map(str::to_string);
            }
            "turn.completed" => {
                // The newest terminal turn is authoritative. A completed turn
                // without an agent message must not reuse prose from an older
                // turn.
                completed_agent_message = pending_agent_message.take();
            }
            "turn.started" => {
                // Likewise, an incomplete newest turn must not publish the
                // previous turn as though the whole execution completed.
                pending_agent_message = None;
                completed_agent_message = None;
            }
            "turn.failed" | "error" => {
                pending_agent_message = None;
                completed_agent_message = None;
            }
            _ => {}
        }
    }

    if !saw_codex_event {
        return None;
    }

    let summary = completed_agent_message
        .map(|message| strip_terminal_noise(&message))
        .filter(|message| !message.is_empty())
        .map(|message| crate::redact::redact(utf8_safe_prefix(&message, SUMMARY_MAX_BYTES)))
        .unwrap_or_else(|| codex_summary_fallback(fallback_context));
    Some(summary)
}

fn codex_summary_fallback(context: Option<&str>) -> String {
    let context = context
        .and_then(|value| value.lines().find(|line| !line.trim().is_empty()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            crate::redact::redact(utf8_safe_prefix(value, FALLBACK_CONTEXT_MAX_BYTES)).to_string()
        });
    match context {
        Some(context) => format!(
            "Backend completed work for {context}, but did not emit a human-readable final summary."
        ),
        None => "Backend completed without a human-readable final summary.".to_string(),
    }
}

/// Extract bounded, redacted prose suitable for both a generated commit body
/// and a pull-request description.
pub(crate) fn extract_backend_summary(
    backend: &str,
    log_path: &str,
    fallback_context: Option<&str>,
) -> String {
    let log_text = fs::read_to_string(log_path).unwrap_or_default();
    if log_text.is_empty() {
        return String::new();
    }
    if backend == "codex" {
        if let Some(summary) = extract_codex_jsonl_summary(&log_text, fallback_context) {
            return summary;
        }
    }

    let wide_tail = utf8_safe_suffix(&log_text, RAW_TAIL_BYTES);
    let cleaned = strip_terminal_noise(wide_tail);
    crate::redact::redact(utf8_safe_suffix(&cleaned, SUMMARY_MAX_BYTES)).to_string()
}

#[cfg(test)]
mod tests {
    use super::{extract_backend_summary, extract_codex_jsonl_summary, strip_terminal_noise};

    #[test]
    fn codex_jsonl_selects_only_final_completed_agent_message() {
        let log = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"secret-internal-id\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"aggregated_output\":\"do not publish tool output\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"First turn summary\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":123}}\n",
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Implemented the notification fix.\\n\\nTests pass.\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":3556388,\"cached_input_tokens\":3424128}}\n",
        );

        let summary = extract_codex_jsonl_summary(log, Some("issue #235")).unwrap();
        assert_eq!(summary, "Implemented the notification fix.\nTests pass.");
        assert!(!summary.contains("input_tokens"));
        assert!(!summary.contains("command_execution"));
        assert!(!summary.contains("secret-internal-id"));
    }

    #[test]
    fn codex_jsonl_without_completed_message_uses_bounded_fallback() {
        let log = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"internal\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"aggregated_output\":\"private tool output\"}}\n",
            "{malformed final line\n",
        );

        let summary = extract_codex_jsonl_summary(log, Some("#361 — summary hardening")).unwrap();
        assert_eq!(
            summary,
            "Backend completed work for #361 — summary hardening, but did not emit a human-readable final summary."
        );
        assert!(!summary.contains("private tool output"));
        assert!(!summary.contains("thread.started"));
    }

    #[test]
    fn codex_jsonl_does_not_reuse_summary_before_incomplete_new_turn() {
        let log = concat!(
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Stale summary\"}}\n",
            "{\"type\":\"turn.completed\"}\n",
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\"}}\n",
        );

        let summary = extract_codex_jsonl_summary(log, Some("issue #361")).unwrap();
        assert_eq!(
            summary,
            "Backend completed work for issue #361, but did not emit a human-readable final summary."
        );
        assert!(!summary.contains("Stale summary"));
    }

    #[test]
    fn terminal_noise_is_removed_from_plain_backend_output() {
        let raw = "\u{1b}[36m│\u{1b}[0m Fixed the eligibility gate. \u{1b}[36m│\u{1b}[0m\n\
                   Goodbye! 👋\n\
                   Conversation ID: internal\n";
        assert_eq!(strip_terminal_noise(raw), "Fixed the eligibility gate.");
    }

    #[test]
    fn plain_summary_survives_test_spam_past_output_cap() {
        let mut log = String::new();
        for index in 0..80 {
            log.push_str(&format!("test some_test_case_{index} ... ok\n"));
        }
        log.push_str(
            "test result: ok. 80 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
        );
        log.push_str("All tests pass. Fixed the bug.");

        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("backend-output.log");
        std::fs::write(&log_path, &log).unwrap();

        assert_eq!(
            extract_backend_summary("vibe", log_path.to_str().unwrap(), None),
            "All tests pass. Fixed the bug."
        );
    }

    #[test]
    fn non_codex_json_shaped_text_keeps_plain_backend_behavior() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("backend-output.log");
        std::fs::write(
            &log_path,
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1}}\nVibe summary.",
        )
        .unwrap();

        let summary = extract_backend_summary("vibe", log_path.to_str().unwrap(), None);
        assert!(summary.contains("turn.completed"));
        assert!(summary.ends_with("Vibe summary."));
    }
}
