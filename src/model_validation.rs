/// A rejected model is reported before the agent takes a turn, so only the
/// leading lines of the output can carry the runner's own diagnostic.
const RUNNER_PREAMBLE_LINES: usize = 20;
const DETAIL_LINES: usize = 40;

/// Recognize runner diagnostics that conclusively reject the selected model.
/// Keep the runner's model list in the operator-facing error when present.
///
/// The phrases also occur in ordinary transcripts (an agent reading this file,
/// a reviewer echoing a diff), so a match must be the runner's own rejection:
/// a diagnostic line or error event in the leading lines of a run that GAH did
/// not kill. Anything else stays a retryable backend failure.
pub(crate) fn invalid_model_message(output: &str, candidate: &str) -> Option<String> {
    if output.contains("GAH: killed after ")
        || output.contains("GAH: harness process cleanup failed:")
    {
        return None;
    }
    let lines: Vec<_> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = lines
        .iter()
        .take(RUNNER_PREAMBLE_LINES)
        .position(|line| is_runner_rejection(line))?;
    let detail = lines[start..]
        .iter()
        .take(DETAIL_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "candidate {candidate}: invalid model configuration: {detail}"
    ))
}

fn is_runner_rejection(line: &str) -> bool {
    let line = line.trim();
    if let Ok(serde_json::Value::Object(event)) = serde_json::from_str::<serde_json::Value>(line) {
        // Structured streams carry agent text in item/message events; only an
        // error event is the runner speaking.
        let is_error_event = matches!(
            event.get("type").and_then(|value| value.as_str()),
            Some("error" | "turn.failed")
        );
        let is_bare_detail = event.len() == 1 && event.contains_key("detail");
        return (is_error_event || is_bare_detail) && names_unsupported_model(line);
    }
    let lower = line.to_ascii_lowercase();
    let diagnostic = ["error:", "fatal:"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
        .unwrap_or(&lower)
        .trim_start();
    diagnostic.starts_with("invalid model selection")
        || (diagnostic.starts_with("the '") && names_unsupported_model(diagnostic))
}

fn names_unsupported_model(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("invalid model selection")
        || lower.contains("model is not supported")
        || lower.contains("model' is not supported")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODEX_REJECTION: &str =
        "The 'default' model is not supported when using Codex with a ChatGPT account.";

    #[test]
    fn recognizes_agy_and_codex_without_confusing_transient_errors() {
        let agy = "invalid model selection (--model \"default\")\nAvailable models:\n  Gemini 3.7 Flash (High)\n  Claude Sonnet 4.6";
        let message = invalid_model_message(agy, "agy/default").unwrap();
        assert!(message.contains("agy/default"));
        assert!(message.contains("Gemini 3.7 Flash (High)"));
        assert!(invalid_model_message(CODEX_REJECTION, "codex/default").is_some());
        assert!(
            invalid_model_message(&format!("ERROR: {CODEX_REJECTION}"), "codex/default").is_some()
        );
        assert!(invalid_model_message("rate limit exceeded", "agy/default").is_none());
    }

    #[test]
    fn recognizes_codex_json_error_event_after_startup_events() {
        let started = serde_json::json!({"type": "thread.started", "thread_id": "t"});
        let turn = serde_json::json!({"type": "turn.started"});
        let error = serde_json::json!({
            "type": "error",
            "message": serde_json::json!({"detail": CODEX_REJECTION}).to_string(),
        });
        let log = format!("{started}\n{turn}\n{error}\n\n[backend internal log]\nnoise");
        assert!(invalid_model_message(&log, "codex/default").is_some());
    }

    #[test]
    fn transcript_that_quotes_the_phrases_is_not_a_rejection() {
        // An agent that read this file, then hit a quota error.
        let quoted_source = "    if !lower.contains(\"invalid model selection\")\n        && !(lower.contains(\"model is not supported\")\nError: rate limit exceeded";
        assert!(invalid_model_message(quoted_source, "claude/opus").is_none());

        // A reviewer echoing the diagnostic inside agent message events.
        let turn = serde_json::json!({"type": "turn.started"});
        let agent_event = serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "agent_message",
                "text": format!("invalid model selection\n{CODEX_REJECTION}"),
            },
        });
        let quota = serde_json::json!({"type": "error", "message": "usage limit reached"});
        let stream = format!("{turn}\n{agent_event}\n{quota}");
        assert!(invalid_model_message(&stream, "codex/gpt").is_none());

        // The same diagnostic text late in a long transcript is agent output.
        let late = format!("{}invalid model selection\n", "agent line\n".repeat(60));
        assert!(invalid_model_message(&late, "agy/default").is_none());
    }

    #[test]
    fn harness_killed_runs_are_never_invalid_model() {
        let stalled = "invalid model selection (--model \"x\")\nGAH: killed after 600s with no new worktree progress (stalled before changes, not just slow).";
        assert!(invalid_model_message(stalled, "agy/x").is_none());
        let cleanup =
            "invalid model selection (--model \"x\")\nGAH: harness process cleanup failed: boom";
        assert!(invalid_model_message(cleanup, "agy/x").is_none());
    }
}
