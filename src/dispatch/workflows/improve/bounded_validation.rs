//! Bounded validation-failure compaction, split out of `improve.rs` to keep
//! that module under the source-size guard. Independent of the dispatch loop.

use crate::dispatch::text::{utf8_safe_prefix, utf8_safe_suffix};

/// Compress a verbose validation log to a bounded, high-signal excerpt so a
/// stalled/retry path can carry failure evidence without unbounded ledger
/// growth. Leading/trailing context around high-signal lines (FAILED,
/// panicked, error, fatal, assertion) is kept; otherwise a head/separator/tail
/// slice is returned.
pub(crate) fn bounded_validation_failure(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let lines = text.lines().collect::<Vec<_>>();
    let mut selected = std::collections::BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let high_signal = line.contains(" ... FAILED")
            || line.contains(" panicked at ")
            || trimmed == "failures:"
            || trimmed.starts_with("test result: FAILED")
            || trimmed.starts_with("error:")
            || trimmed.starts_with("error[")
            || trimmed.starts_with("npm error")
            || trimmed.starts_with("fatal:")
            || trimmed.contains("AssertionError")
            || trimmed.contains("TypeError:");
        if high_signal {
            for context_index in index.saturating_sub(2)..=(index + 2).min(lines.len() - 1) {
                selected.insert(context_index);
            }
        }
    }
    if !selected.is_empty() {
        let command = lines
            .iter()
            .find(|line| line.trim_start().starts_with("$ "))
            .copied()
            .unwrap_or("validation failed");
        let evidence = selected
            .into_iter()
            .map(|index| lines[index])
            .collect::<Vec<_>>()
            .join("\n");
        let focused = format!("{command}\n... failure evidence ...\n{evidence}");
        if focused.len() <= max_bytes {
            return focused;
        }
        return utf8_safe_suffix(&focused, max_bytes).to_string();
    }
    let separator = "\n... validation output omitted ...\n";
    let available = max_bytes.saturating_sub(separator.len());
    let head_bytes = available / 3;
    let tail_bytes = available.saturating_sub(head_bytes);
    format!(
        "{}{}{}",
        utf8_safe_prefix(text, head_bytes),
        separator,
        utf8_safe_suffix(text, tail_bytes)
    )
}
