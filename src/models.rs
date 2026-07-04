use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Deserialize)]
pub struct GateArtifact {
    #[serde(default)]
    pub source_scout_artifact: Option<String>,
    #[serde(default)]
    pub findings: Vec<GateFinding>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GateFinding {
    pub id: Option<String>,
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub finding_type: Option<String>,
    pub gate_status: String,
    #[serde(default)]
    pub source_finding_path: Option<String>,
    #[serde(default)]
    pub source_draft_issue_path: Option<String>,
    #[serde(default)]
    pub affected_files: Option<Vec<String>>,
    #[serde(default)]
    pub evidence: Option<Vec<String>>,
    #[serde(default)]
    pub commands: Option<Vec<String>>,
    #[serde(default)]
    pub suggested_acceptance_criteria: Option<Vec<String>>,
    #[serde(default)]
    pub suggested_verification: Option<Vec<String>>,
    #[serde(default)]
    pub risk_guess: Option<String>,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub likely_agent_safe: Option<bool>,
    #[serde(default)]
    pub finding_path: Option<String>,
    #[serde(default)]
    pub draft_issue_path: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScoutArtifact {
    #[serde(default)]
    pub findings: Vec<ScoutFinding>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScoutFinding {
    pub id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub affected_files: Option<Vec<String>>,
    #[serde(default)]
    pub evidence: Option<Vec<String>>,
    #[serde(default)]
    pub commands: Option<Vec<String>>,
    #[serde(default)]
    pub suggested_acceptance_criteria: Option<Vec<String>>,
    #[serde(default)]
    pub suggested_verification: Option<Vec<String>>,
    #[serde(default)]
    pub risk_guess: Option<String>,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub likely_agent_safe: Option<bool>,
    #[serde(default)]
    pub finding_path: Option<String>,
    #[serde(default)]
    pub draft_issue_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CandidateArtifact {
    pub counts: CandidateCounts,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CandidateCounts {
    pub seen: usize,
    pub converted: usize,
    pub skipped_warning: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub candidate_id: String,
    pub source_gate_status: String,
    pub suggested_blueprint_phase: String,
    pub provider_mutation_allowed: bool,
    pub suggested_labels: Vec<String>,
    pub affected_files: Vec<String>,
    pub evidence: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub verification: Vec<String>,
    pub hydration_used: bool,
    pub hydration_source: String,
    pub hydration_match_method: String,
    pub hydrated_fields: Vec<String>,
    pub debug_gate_keys: Vec<String>,
    pub debug_scout_keys: Vec<String>,
    pub debug_hydrated_keys: Vec<String>,
    pub debug_hydrated_finding_excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_finding_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_draft_issue_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Watchlist {
    pub models: Vec<WatchModel>,
}

#[derive(Debug, Deserialize)]
pub struct WatchModel {
    pub id: String,
    pub status: String,
    pub input_per_1m: f64,
    pub output_per_1m: f64,
    pub max_input_per_1m: f64,
    pub max_output_per_1m: f64,
}

#[derive(Debug, Deserialize)]
pub struct PolicyConfig {
    pub repo: RepoPolicy,
}

#[derive(Debug, Deserialize)]
pub struct RepoPolicy {
    pub trust_mode: String,
    pub allow_provider_mutation: bool,
    pub allow_push: bool,
    pub allow_draft_pr: bool,
    pub allow_issue_write: bool,
    pub allow_project_write: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PmPlan {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub tickets: Vec<PmPlanTicket>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PmPlanTicket {
    pub title: String,
    pub summary: String,
    pub difficulty: String,
    pub risk: String,
    #[serde(default)]
    pub recommended_backend: Option<String>,
    #[serde(default)]
    pub duplicate_evidence: Vec<String>,
    #[serde(default)]
    pub affected_files: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub verification_commands: Vec<String>,
    pub uncovered_reason: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReviewVerdict {
    pub verdict: String,
    pub confidence: String,
    pub human_required: bool,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub blocking_findings: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub non_blocking_findings: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub risk_notes: Vec<String>,
    #[serde(default)]
    pub reviewer_backend: Option<String>,
    #[serde(default)]
    pub reviewer_model: Option<String>,
    #[serde(default)]
    pub requested_backend: Option<String>,
    #[serde(default)]
    pub effective_backend: Option<String>,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub effective_model: Option<String>,
    #[serde(default)]
    pub fallback_used: Option<bool>,
    #[serde(default)]
    pub usage_source: Option<String>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub estimated_cost_usd: Option<f64>,
    #[serde(default)]
    pub actual_cost_usd: Option<f64>,
}

fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(vec![]),
        Some(serde_json::Value::String(item)) => Ok(vec![item]),
        Some(serde_json::Value::Array(items)) => items
            .into_iter()
            .map(|item| match item {
                serde_json::Value::String(value) => Ok(value),
                other => Err(serde::de::Error::custom(format!(
                    "expected string in array, got {other}"
                ))),
            })
            .collect(),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected string, array, or null, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::ReviewVerdict;

    #[test]
    fn review_verdict_accepts_string_arrays() {
        let verdict: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false,"blocking_findings":["a"],"non_blocking_findings":["b"],"risk_notes":["c"]}"#,
        )
        .unwrap();
        assert_eq!(verdict.blocking_findings, vec!["a"]);
        assert_eq!(verdict.non_blocking_findings, vec!["b"]);
        assert_eq!(verdict.risk_notes, vec!["c"]);
    }

    #[test]
    fn review_verdict_normalizes_single_strings() {
        let verdict: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false,"blocking_findings":"a","non_blocking_findings":"b","risk_notes":"c"}"#,
        )
        .unwrap();
        assert_eq!(verdict.blocking_findings, vec!["a"]);
        assert_eq!(verdict.non_blocking_findings, vec!["b"]);
        assert_eq!(verdict.risk_notes, vec!["c"]);
    }

    #[test]
    fn review_verdict_normalizes_null_and_missing_lists() {
        let with_null: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false,"blocking_findings":null,"non_blocking_findings":null,"risk_notes":null}"#,
        )
        .unwrap();
        assert!(with_null.blocking_findings.is_empty());
        assert!(with_null.non_blocking_findings.is_empty());
        assert!(with_null.risk_notes.is_empty());

        let missing: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false}"#,
        )
        .unwrap();
        assert!(missing.blocking_findings.is_empty());
        assert!(missing.non_blocking_findings.is_empty());
        assert!(missing.risk_notes.is_empty());
    }

    #[test]
    fn review_verdict_still_rejects_malformed_json() {
        serde_json::from_str::<ReviewVerdict>(
            r#"{"verdict":"APPROVE_STRONG","confidence":"high","human_required":false"#,
        )
        .unwrap_err();
    }
}
