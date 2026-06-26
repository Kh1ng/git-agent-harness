use serde::{Deserialize, Serialize};

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

#[derive(Debug, Serialize)]
pub struct CandidateArtifact {
    pub counts: CandidateCounts,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Serialize)]
pub struct CandidateCounts {
    pub seen: usize,
    pub converted: usize,
    pub skipped_warning: usize,
}

#[derive(Debug, Serialize)]
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
