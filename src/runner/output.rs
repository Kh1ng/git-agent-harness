//! Trust boundary for backend text that may be published to a repository.
//!
//! Backend stdout/stderr stays available as a diagnostic artifact, but it is
//! never a source for commit or pull-request prose. Adapters extract an
//! authoritative final assistant message and pass it through this module.

use serde_json::Value;
use std::path::Path;

const SUMMARY_MAX_BYTES: usize = 2_000;
const FALLBACK_CONTEXT_MAX_BYTES: usize = 300;

fn utf8_safe_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn strip_control_sequences(text: &str) -> String {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    let osc = regex::Regex::new(r"\x1b\].*?(?:\x07|\x1b\\)").unwrap();
    let ansi_text = ansi.replace_all(text, "");
    osc.replace_all(&ansi_text, "").into_owned()
}

fn normalize_workspace_paths(text: &str, worktree: &Path) -> String {
    let worktree = worktree.to_string_lossy();
    let normalized = text.replace(worktree.as_ref(), ".");
    let absolute_markdown_link = regex::Regex::new(r"\[([^\]]+)\]\((/[^)]+)\)").unwrap();
    absolute_markdown_link
        .replace_all(&normalized, |captures: &regex::Captures<'_>| {
            let label = &captures[1];
            let target = &captures[2];
            let relative = ["/src/", "/tests/", "/apps/", "/packages/", "/docs/"]
                .iter()
                .find_map(|marker| target.rfind(marker).map(|index| &target[index + 1..]));
            match relative {
                Some(relative) => format!("[{label}]({relative})"),
                None => label.to_string(),
            }
        })
        .into_owned()
}

/// Apply the one common publication policy after a backend-specific parser
/// has selected an authoritative assistant message.
pub(crate) fn sanitize_final_summary(text: &str, worktree: &Path) -> Option<String> {
    let without_controls = strip_control_sequences(text);
    let normalized = normalize_workspace_paths(&without_controls, worktree);
    let mut previous_blank = false;
    let normalized = normalized
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let blank = line.is_empty();
            let keep = !blank || !previous_blank;
            previous_blank = blank;
            keep
        })
        .collect::<Vec<_>>()
        .join("\n");
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return None;
    }
    let redacted = crate::redact::redact(normalized);
    Some(utf8_safe_prefix(&redacted, SUMMARY_MAX_BYTES).to_string())
}

/// Return the adapter-provided summary or deterministic work-item prose. Raw
/// terminal output is deliberately not accepted by this API.
pub(crate) fn publishable_summary(
    final_summary: Option<&str>,
    fallback_context: Option<&str>,
    worktree: &Path,
) -> String {
    final_summary
        .and_then(|summary| sanitize_final_summary(summary, worktree))
        .unwrap_or_else(|| summary_fallback(fallback_context))
}

fn summary_fallback(context: Option<&str>) -> String {
    let context = context
        .and_then(|value| value.lines().find(|line| !line.trim().is_empty()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let value = strip_control_sequences(value);
            let redacted = crate::redact::redact(&value);
            utf8_safe_prefix(&redacted, FALLBACK_CONTEXT_MAX_BYTES).to_string()
        });
    match context {
        Some(context) => format!(
            "Backend completed work for {context}, but did not emit a trustworthy final summary."
        ),
        None => "Backend completed without a trustworthy final summary.".to_string(),
    }
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

/// Extract only the final agent message belonging to the newest completed
/// Codex turn. Tool events, usage records, stale turns, and incomplete turns
/// are never returned.
pub(crate) fn extract_codex_jsonl_summary(text: &str) -> Option<String> {
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
            "turn.completed" => completed_agent_message = pending_agent_message.take(),
            "turn.started" | "turn.failed" | "error" => {
                pending_agent_message = None;
                completed_agent_message = None;
            }
            _ => {}
        }
    }

    saw_codex_event.then_some(())?;
    completed_agent_message
}

/// Claude transcripts contain repeated snapshots of an assistant message.
/// The final completed assistant record with text content is authoritative.
pub(crate) fn extract_claude_transcript_summary(text: &str) -> Option<String> {
    let mut summary = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if event
            .pointer("/message/stop_reason")
            .and_then(Value::as_str)
            != Some("end_turn")
        {
            continue;
        }
        let Some(content) = event.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        let text_parts = content
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>();
        if !text_parts.is_empty() {
            summary = Some(text_parts.join("\n"));
        }
    }
    summary
}

/// Vibe persists the authoritative conversation separately from its terminal
/// renderer. Select only the last assistant message that did not request a
/// tool call.
pub(crate) fn extract_vibe_messages_summary(text: &str) -> Option<String> {
    let mut summary = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let has_tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty());
        let content = message.get("content").and_then(Value::as_str);
        if !has_tool_calls {
            if let Some(content) = content.filter(|content| !content.trim().is_empty()) {
                summary = Some(content.to_string());
            }
        }
    }
    summary
}

/// OpenHands headless JSON has used both message-shaped and event-shaped
/// assistant records. Accept only explicit assistant text, never observations,
/// tool output, metrics, or arbitrary JSON fields.
pub(crate) fn extract_openhands_jsonl_summary(text: &str) -> Option<String> {
    let mut summary = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = event
            .get("role")
            .or_else(|| event.pointer("/message/role"))
            .and_then(Value::as_str);
        if role != Some("assistant") {
            continue;
        }
        let content = event
            .get("content")
            .or_else(|| event.pointer("/message/content"));
        let candidate = match content {
            Some(Value::String(text)) => Some(text.clone()),
            Some(Value::Array(parts)) => {
                let parts = parts
                    .iter()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>();
                (!parts.is_empty()).then(|| parts.join("\n"))
            }
            _ => None,
        };
        if candidate
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
        {
            summary = candidate;
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_selects_only_final_completed_agent_message() {
        let log = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"secret\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"aggregated_output\":\"tool output\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"First turn\"}}\n",
            "{\"type\":\"turn.completed\"}\n",
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Final summary\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":123}}\n",
        );
        assert_eq!(
            extract_codex_jsonl_summary(log).as_deref(),
            Some("Final summary")
        );
    }

    #[test]
    fn codex_does_not_reuse_summary_before_incomplete_turn() {
        let log = concat!(
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Stale\"}}\n",
            "{\"type\":\"turn.completed\"}\n",
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\"}}\n",
        );
        assert_eq!(extract_codex_jsonl_summary(log), None);
    }

    #[test]
    fn vibe_fixture_reproducing_370_returns_only_final_assistant_text() {
        let log = concat!(
            "{\"role\":\"assistant\",\"content\":\"Running tests\",\"tool_calls\":[{\"id\":\"secret\"}]}\n",
            "{\"role\":\"tool\",\"content\":\"src/a.rs\\ntests/a.rs\\nFinished test\"}\n",
            "{\"role\":\"assistant\",\"content\":\"Implemented the lifecycle fix.\",\"tool_calls\":[]}\n",
        );
        assert_eq!(
            extract_vibe_messages_summary(log).as_deref(),
            Some("Implemented the lifecycle fix.")
        );
    }

    #[test]
    fn claude_uses_only_completed_end_turn_text() {
        let log = concat!(
            "{\"type\":\"assistant\",\"message\":{\"stop_reason\":\"tool_use\",\"content\":[{\"type\":\"text\",\"text\":\"I will run tests\"},{\"type\":\"tool_use\"}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":\"tool output\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"stop_reason\":\"end_turn\",\"content\":[{\"type\":\"text\",\"text\":\"Implemented the fix.\"}]}}\n",
        );
        assert_eq!(
            extract_claude_transcript_summary(log).as_deref(),
            Some("Implemented the fix.")
        );
    }

    #[test]
    fn openhands_ignores_observations_and_usage_events() {
        let log = concat!(
            "{\"role\":\"tool\",\"content\":\"private command output\"}\n",
            "{\"type\":\"usage\",\"input_tokens\":999}\n",
            "{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Completed the change.\"}]}\n",
        );
        assert_eq!(
            extract_openhands_jsonl_summary(log).as_deref(),
            Some("Completed the change.")
        );
    }

    #[test]
    fn missing_summary_uses_deterministic_fallback_not_terminal_text() {
        let worktree = Path::new("/tmp/worktrees/run-1");
        let summary = publishable_summary(None, Some("#371 structured summaries"), worktree);
        assert_eq!(
            summary,
            "Backend completed work for #371 structured summaries, but did not emit a trustworthy final summary."
        );
    }

    #[test]
    fn sanitization_redacts_controls_and_normalizes_worktree_paths() {
        let worktree = Path::new("/tmp/worktrees/run-1");
        let summary = sanitize_final_summary(
            "Updated [file](/tmp/worktrees/run-1/src/lib.rs).\n\x1b[31mTests pass.\x1b[0m",
            worktree,
        )
        .unwrap();
        assert_eq!(summary, "Updated [file](./src/lib.rs).\nTests pass.");
    }

    #[test]
    fn sanitization_does_not_publish_unrelated_absolute_markdown_paths() {
        let worktree = Path::new("/tmp/worktrees/run-1");
        let summary = sanitize_final_summary(
            "See [source](/home/user/elsewhere/src/lib.rs:42) and [secret](/home/user/.config/key).",
            worktree,
        )
        .unwrap();
        assert_eq!(summary, "See [source](src/lib.rs:42) and secret.");
        assert!(!summary.contains("/home/"));
    }
}
