//! Standalone ticket difficulty/cost/duration estimator (issue #916).
//!
//! Extracted as a narrow, single-ticket prompt/parse pair so it can run
//! against any target -- not just tickets already going through
//! `workflows::pm`'s multi-ticket decomposition call, which only ever
//! produces a difficulty label as one field of a much larger "draft N new
//! child tickets" plan (see `workflows::pm`'s plan-building prompt). Reuses
//! that same `difficulty: easy|medium|hard` vocabulary/validation so the two
//! call sites can never drift into incompatible labels.

use super::super::text::extract_first_json_object;
use anyhow::{Context, Result};
use serde::Deserialize;

const ESTIMATE_JSON_MAX_BYTES: usize = 4_000;

/// A single ticket's predicted difficulty/cost/duration, produced before
/// dispatch so it can be compared against the actual outcome recorded on
/// the same ledger entry afterward.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TicketEstimate {
    pub(super) difficulty: String,
    pub(super) predicted_cost_usd: Option<f64>,
    pub(super) predicted_duration_seconds: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RawEstimate {
    difficulty: String,
    #[serde(default)]
    predicted_cost_usd: Option<f64>,
    #[serde(default)]
    predicted_duration_seconds: Option<f64>,
}

/// Build the task string for a single-ticket estimate call.
pub(super) fn build_estimate_task(title: &str, objective: &str) -> String {
    format!(
        "You are estimating effort for a single engineering ticket before it is \
         dispatched to an autonomous coding backend.\n\n\
         Return only valid JSON matching this schema:\n\
         {{\"difficulty\": \"easy|medium|hard\", \
         \"predicted_cost_usd\": number, \
         \"predicted_duration_seconds\": number}}\n\n\
         Rules:\n\
         - difficulty must be exactly one of easy, medium, hard.\n\
         - predicted_cost_usd and predicted_duration_seconds are your best-effort \
           estimate of the LLM backend cost (USD) and wall-clock duration (seconds) \
           to complete this ticket end to end, including retries.\n\
         - Keep this JSON strictly machine-consumable; no prose outside JSON.\n\n\
         ## Ticket title\n{}\n\n\
         ## Ticket body\n{}\n",
        title, objective,
    )
}

/// Parse a single-ticket estimate response produced from
/// `build_estimate_task`'s prompt.
pub(super) fn parse_estimate_response(text: &str) -> Result<TicketEstimate> {
    let json = extract_first_json_object(text)
        .ok_or_else(|| anyhow::anyhow!("estimator did not return valid JSON"))?;
    if json.len() > ESTIMATE_JSON_MAX_BYTES {
        anyhow::bail!(
            "estimate response exceeded {} bytes; reject malformed/bounded-overflow response",
            ESTIMATE_JSON_MAX_BYTES
        );
    }
    let raw: RawEstimate =
        serde_json::from_str(&json).context("estimate response did not match expected schema")?;
    if !matches!(raw.difficulty.as_str(), "easy" | "medium" | "hard") {
        anyhow::bail!(
            "estimate response had invalid difficulty '{}'",
            raw.difficulty
        );
    }
    Ok(TicketEstimate {
        difficulty: raw.difficulty,
        predicted_cost_usd: raw.predicted_cost_usd,
        predicted_duration_seconds: raw.predicted_duration_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_estimate_task_includes_title_and_body() {
        let task = build_estimate_task("Fix flaky test", "The retry test flakes under load.");
        assert!(task.contains("Fix flaky test"));
        assert!(task.contains("flakes under load"));
        assert!(task.contains("easy|medium|hard"));
    }

    #[test]
    fn parse_estimate_response_extracts_full_estimate() {
        let text = "noise before\n\
            {\"difficulty\": \"medium\", \"predicted_cost_usd\": 1.25, \"predicted_duration_seconds\": 900}\n\
            noise after";
        let estimate = parse_estimate_response(text).unwrap();
        assert_eq!(estimate.difficulty, "medium");
        assert_eq!(estimate.predicted_cost_usd, Some(1.25));
        assert_eq!(estimate.predicted_duration_seconds, Some(900.0));
    }

    #[test]
    fn parse_estimate_response_allows_missing_predictions() {
        let text = "{\"difficulty\": \"easy\"}";
        let estimate = parse_estimate_response(text).unwrap();
        assert_eq!(estimate.difficulty, "easy");
        assert_eq!(estimate.predicted_cost_usd, None);
        assert_eq!(estimate.predicted_duration_seconds, None);
    }

    #[test]
    fn parse_estimate_response_rejects_invalid_difficulty() {
        let text = "{\"difficulty\": \"impossible\"}";
        assert!(parse_estimate_response(text).is_err());
    }

    #[test]
    fn parse_estimate_response_rejects_missing_json() {
        assert!(parse_estimate_response("no json here").is_err());
    }

    #[test]
    fn parse_estimate_response_rejects_oversized_json() {
        let padding = "x".repeat(ESTIMATE_JSON_MAX_BYTES);
        let text = format!("{{\"difficulty\": \"easy\", \"note\": \"{padding}\"}}");
        assert!(parse_estimate_response(&text).is_err());
    }
}
