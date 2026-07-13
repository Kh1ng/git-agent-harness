//! Central, conservative secret redaction for every durable or external sink.
//!
//! This is intentionally on by by default.  Callers should pass text through
//! [`redact`] before writing an artifact, ledger/event record, provider body,
//! or notification.  The function is idempotent, so applying it at more than
//! one boundary is safe and gives individual sinks a useful last line of
//! defense.

use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

fn github_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b")
            .expect("valid GitHub token regex")
    })
}

fn gitlab_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\bglpat-[A-Za-z0-9_-]{20,}\b").expect("valid GitLab token regex")
    })
}

fn api_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bsk-(?:[A-Za-z0-9_-]{20,})\b").expect("valid API key regex"))
}

fn bearer_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(authorization\s*:\s*bearer\s+)[^\s\"']+"#)
            .expect("valid bearer token regex")
    })
}

fn basic_auth_url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(https?://)[^/\s@]+@").expect("valid basic-auth URL regex"))
}

fn query_secret_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)([?&](?:access_token|api[_-]?key|token|password)=)[^&#\s]+")
            .expect("valid query secret regex")
    })
}

fn secret_env_values() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(name, value)| is_secret_env_name(name) && value.len() >= 4)
        .collect()
}

fn is_secret_env_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "PRIVATE_KEY",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

/// Redact known live environment secrets plus common provider-token forms.
pub fn redact(text: &str) -> String {
    redact_with_known_secrets(text, &secret_env_values())
}

/// Redact text using explicit secrets as well as provider token patterns.
///
/// Exposed primarily for deterministic unit tests and callers that hold a
/// credential outside the process environment.  A short secret is ignored to
/// avoid replacing ordinary prose accidentally.
pub fn redact_with_known_secrets(text: &str, secrets: &[(String, String)]) -> String {
    let mut output = text.to_string();
    let mut secrets: Vec<_> = secrets
        .iter()
        .filter(|(_, value)| value.len() >= 4)
        .collect();
    // Longest-first prevents a prefix secret from leaving the suffix behind.
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.1.len()));
    for (name, value) in secrets {
        // A raw substring replace corrupts unrelated text whenever a short
        // secret value (e.g. a test harness's `GITHUB_TOKEN=token`, or any
        // real value that is coincidentally a short common word) also
        // appears inside a longer, unrelated word -- "token" inside
        // "input_tokens" being the concrete case that surfaced this. Anchor
        // on word boundaries so only a standalone occurrence of the value is
        // redacted, never a substring of a different identifier.
        let Ok(pattern) = Regex::new(&format!(r"\b{}\b", regex::escape(value))) else {
            continue;
        };
        output = pattern
            .replace_all(&output, format!("[REDACTED:{name}]"))
            .into_owned();
    }
    output = github_token_re()
        .replace_all(&output, "[REDACTED:GITHUB_TOKEN]")
        .into_owned();
    output = gitlab_token_re()
        .replace_all(&output, "[REDACTED:GITLAB_TOKEN]")
        .into_owned();
    output = api_key_re()
        .replace_all(&output, "[REDACTED:API_KEY]")
        .into_owned();
    output = bearer_re()
        .replace_all(&output, "$1[REDACTED:TOKEN]")
        .into_owned();
    output = basic_auth_url_re()
        .replace_all(&output, "$1[REDACTED:URL_CREDENTIAL]@")
        .into_owned();
    query_secret_re()
        .replace_all(&output, "$1[REDACTED:URL_CREDENTIAL]")
        .into_owned()
}

/// Recursively redact every string in a JSON value before serialization.
pub fn redact_json_value(value: &mut Value) {
    redact_json_value_with_known_secrets(value, &secret_env_values());
}

fn redact_json_value_with_known_secrets(value: &mut Value, secrets: &[(String, String)]) {
    match value {
        Value::String(text) => *text = redact_with_known_secrets(text, secrets),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| redact_json_value_with_known_secrets(value, secrets)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| redact_json_value_with_known_secrets(value, secrets)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{redact_json_value_with_known_secrets, redact_with_known_secrets};

    fn redact(text: &str) -> String {
        redact_with_known_secrets(
            text,
            &[("DEMO_TOKEN".to_string(), "s3cr3t-value-123".to_string())],
        )
    }

    #[test]
    fn short_known_secret_value_does_not_corrupt_a_longer_unrelated_word() {
        // Regression: a test harness (or a real deployment) can set a short
        // *_TOKEN env value like "token". A raw substring replace turned
        // "input_tokens: 500" into "input_[REDACTED:...]s: 500", silently
        // destroying the field name and losing real usage data downstream.
        let result = redact_with_known_secrets(
            "input_tokens: 500\noutput_tokens: 120",
            &[("GITHUB_TOKEN".to_string(), "token".to_string())],
        );
        assert_eq!(result, "input_tokens: 500\noutput_tokens: 120");
    }

    #[test]
    fn short_known_secret_value_is_still_redacted_as_a_standalone_word() {
        let result = redact_with_known_secrets(
            "auth failed: token invalid",
            &[("GITHUB_TOKEN".to_string(), "token".to_string())],
        );
        assert_eq!(result, "auth failed: [REDACTED:GITHUB_TOKEN] invalid");
    }

    #[test]
    fn redacts_known_values_and_provider_patterns_without_losing_context() {
        let result = redact(
            "failed with s3cr3t-value-123; glpat-abcdefghijklmnopqrstuv; Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
        );
        assert_eq!(
            result,
            "failed with [REDACTED:DEMO_TOKEN]; [REDACTED:GITLAB_TOKEN]; Authorization: Bearer [REDACTED:TOKEN]"
        );
    }

    #[test]
    fn redacts_github_api_and_basic_auth_url_tokens() {
        let result = redact(
            "ghp_abcdefghijklmnopqrstuvwxyz https://user:pass@example.test/a?access_token=abcdefghi",
        );
        assert_eq!(
            result,
            "[REDACTED:GITHUB_TOKEN] https://[REDACTED:URL_CREDENTIAL]@example.test/a?access_token=[REDACTED:URL_CREDENTIAL]"
        );
    }

    #[test]
    fn is_idempotent_and_preserves_ordinary_text() {
        let once = redact("ordinary output: cargo test passed");
        assert_eq!(once, "ordinary output: cargo test passed");
        assert_eq!(redact(&once), once);
    }

    #[test]
    fn redacts_strings_recursively_in_json_values() {
        let mut value = serde_json::json!({
            "details": ["Authorization: Bearer abcdefghijklmnopqrstuvwxyz", {"nested": "s3cr3t-value-123"}]
        });
        redact_json_value_with_known_secrets(
            &mut value,
            &[("DEMO_TOKEN".to_string(), "s3cr3t-value-123".to_string())],
        );
        assert_eq!(
            value["details"][0],
            "Authorization: Bearer [REDACTED:TOKEN]"
        );
        assert_eq!(value["details"][1]["nested"], "[REDACTED:DEMO_TOKEN]");
    }
}
