pub(super) fn first_markdown_heading(body: &str) -> Option<&str> {
    body.lines().map(str::trim).find_map(|line| {
        if !line.starts_with('#') {
            return None;
        }
        let stripped = line.trim_start_matches('#').trim();
        (!stripped.is_empty()).then_some(stripped)
    })
}

pub(super) fn extract_first_json_object(text: &str) -> Option<String> {
    if let Some(fenced) = extract_last_fenced_json_block(text) {
        return Some(fenced);
    }
    let bytes = text.as_bytes();
    let mut last_valid: Option<String> = None;
    let mut start = 0usize;
    while start < bytes.len() {
        if bytes[start] != b'{' {
            start += 1;
            continue;
        }
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        let mut matched_end = None;
        for (end, &byte) in bytes.iter().enumerate().skip(start) {
            let ch = byte as char;
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        matched_end = Some(end);
                        break;
                    }
                }
                _ => {}
            }
        }
        match matched_end {
            // Found a balanced top-level span -- validate it, then jump past
            // its closing brace entirely. Without this jump, the next outer
            // iteration would step into the span's interior and re-match any
            // nested object (e.g. a ticket sub-object inside a PM plan) as
            // its own "later" candidate, which is never what's wanted here.
            Some(end) => {
                let candidate = &text[start..=end];
                if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                    last_valid = Some(candidate.to_string());
                }
                start = end + 1;
            }
            None => start += 1,
        }
    }
    last_valid
}

/// Finds the last ` ```json ... ``` ` fenced block in `text` whose contents
/// parse as valid JSON, if any.
pub(super) fn extract_last_fenced_json_block(text: &str) -> Option<String> {
    const FENCE_OPEN: &str = "```json";
    const FENCE_CLOSE: &str = "```";
    let mut last_valid: Option<String> = None;
    let mut search_from = 0usize;
    while let Some(rel_open) = text[search_from..].find(FENCE_OPEN) {
        let content_start = search_from + rel_open + FENCE_OPEN.len();
        let Some(rel_close) = text[content_start..].find(FENCE_CLOSE) else {
            break;
        };
        let content_end = content_start + rel_close;
        let candidate = text[content_start..content_end].trim();
        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
            last_valid = Some(candidate.to_string());
        }
        search_from = content_end + FENCE_CLOSE.len();
    }
    last_valid
}

pub(super) fn normalize_match(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
