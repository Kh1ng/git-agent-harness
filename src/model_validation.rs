/// Recognize runner diagnostics that conclusively reject the selected model.
/// Keep the runner's model list in the operator-facing error when present.
pub(crate) fn invalid_model_message(output: &str, candidate: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    if !lower.contains("invalid model selection")
        && !(lower.contains("model is not supported")
            || (lower.contains("model' is not supported")))
    {
        return None;
    }
    let lines: Vec<_> = output.lines().collect();
    let start = lines.iter().position(|line| {
        let line = line.to_ascii_lowercase();
        line.contains("invalid model selection")
            || line.contains("model is not supported")
            || line.contains("model' is not supported")
    })?;
    let detail = lines[start..]
        .iter()
        .take(40)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "candidate {candidate}: invalid model configuration: {detail}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_agy_and_codex_without_confusing_transient_errors() {
        let agy = "invalid model selection (--model \"default\")\nAvailable models:\n  Gemini 3.7 Flash (High)\n  Claude Sonnet 4.6";
        let message = invalid_model_message(agy, "agy/default").unwrap();
        assert!(message.contains("agy/default"));
        assert!(message.contains("Gemini 3.7 Flash (High)"));
        assert!(invalid_model_message(
            "The 'default' model is not supported when using Codex with a ChatGPT account.",
            "codex/default"
        )
        .is_some());
        assert!(invalid_model_message("rate limit exceeded", "agy/default").is_none());
    }
}
