use serde::{Deserialize, Deserializer, Serialize};

/// TICKET-078: a ticket in `docs/tickets/` observed as a dispatch candidate.
/// `has_active_mr` tickets are excluded from consideration entirely --
/// their work_id is already covered by the MR-classification rules in
/// `decide_next_action` (ReviewMr/FixMr take precedence). Everything else
/// is either never-dispatched (`prior_attempt_count == 0`, a
/// `DispatchTicket` candidate) or has failed history with no active MR
/// (a `Retry`/`Escalate` candidate, gated by `last_failure_class` and the
/// retry cap on `prior_attempt_count`).
#[derive(Debug, Serialize, Clone)]
pub struct AvailableTicket {
    pub ticket_path: String,
    pub work_id: Option<String>,
    pub title: Option<String>,
    pub recommended_backend: Option<String>,
    pub recommended_model: Option<String>,
    pub prior_attempt_count: usize,
    /// Issue #95: count of attempts whose failure_class is a genuine agent
    /// failure (agent_no_progress | agent_failure). Infra-class failures
    /// (backend_error, environment_error, etc.) are still recorded in
    /// `prior_attempt_count` for history purposes, but only genuine agent
    /// failures consume the AUTO_RETRY_CAP.
    pub genuine_agent_failure_count: usize,
    pub last_failure_class: Option<String>,
    pub has_active_mr: bool,
    /// TICKET-human-required-scoping: effective `human_required` for this
    /// work item, derived from its own ledger history (a review verdict with
    /// `human_required`). Scoped to this ticket only; it does NOT block the
    /// profile. `None` work items (no work_id) are treated as not blocked.
    pub human_required: bool,
    /// Work-item-scoped reason for `human_required`, when available.
    /// `review_evidence_gate` covers review verdict gates, while
    /// `policy_approval` covers paid-route approval and similar policy
    /// blocks. Unknown or synthetic reasons are represented as
    /// `human_required` for backward compatibility.
    #[serde(default)]
    pub human_required_reason_code: Option<String>,
    /// Parallel workers: another concurrent `gah loop`/`gah dispatch`
    /// process claimed this work_id recently and hasn't finished (or been
    /// abandoned long enough to ignore) it yet. Excluded from selection
    /// the same way `has_active_mr` tickets are -- there's no point
    /// picking a ticket a sibling worker is already mid-flight on.
    pub has_active_claim: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct IssueIntakePolicy {
    pub mode: String,
    pub canonical_autonomous_label: String,
    pub trusted_human_authors: Vec<String>,
    pub trusted_bot_authors: Vec<String>,
    pub github_issue_author_allowlist: Vec<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct IssueIntakeRejection {
    pub ticket_path: String,
    pub work_id: Option<String>,
    pub title: Option<String>,
    pub provider: String,
    pub author_login: Option<String>,
    pub author_kind: Option<String>,
    pub reason_code: String,
    pub reason: String,
    pub labels: Vec<String>,
}

/// One same-project prerequisite observed while deciding whether an issue is
/// safe to dispatch. Provider state is retained verbatim for auditability;
/// normalized state drives the fail-closed controller policy.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct DependencyObservation {
    pub identity: String,
    pub provider: String,
    pub provider_state: Option<String>,
    pub normalized_state: String,
}

/// A native issue excluded from autonomous intake by its declared
/// prerequisites. This is status data, not a provider-label mutation.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct DependencyBlocker {
    pub ticket_path: String,
    pub work_id: String,
    pub title: String,
    pub reason_code: String,
    pub reason: String,
    pub dependencies: Vec<DependencyObservation>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct WorkMetadata {
    #[serde(default)]
    pub ticket_id: Option<String>,
    #[serde(default)]
    pub work_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub issue_number: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub problem: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub suggested_mr_title: Option<String>,
    #[serde(default)]
    pub difficulty: Option<String>,
    #[serde(default)]
    pub task_class: Option<String>,
    #[serde(default)]
    pub risk: Option<String>,
    #[serde(default)]
    pub recommended_backend: Option<String>,
    #[serde(default)]
    pub recommended_model: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub verification_commands: Vec<String>,
    #[serde(default)]
    pub affected_files: Vec<String>,
    #[serde(default)]
    pub is_authoritative: bool,
    /// PM-plan-mode only: prior evidence the PM cited for why this isn't a
    /// duplicate of existing work.
    #[serde(default)]
    pub duplicate_evidence: Vec<String>,
    /// PM-plan-mode only: why this gap isn't already covered.
    #[serde(default)]
    pub uncovered_reason: Option<String>,
}

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

/// TICKET-544: bounded, provider-neutral planner work packet.
///
/// Replaces the underspecified PM ticket payload (`WorkMetadata`) with a
/// schema where every field the controller/routing layer consumes is
/// explicit and validated. Recommended routing expresses capability and
/// difficulty tier rather than a hard-coded (temporary) model name, so the
/// dispatcher stays free to map the request onto whatever backend instance
/// currently satisfies the capability.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlannerWorkPacket {
    /// Plan-local stable key, unique within the plan. Used to express
    /// dependencies between packets via `depends_on`.
    pub key: String,
    pub title: String,
    /// Concise provider-facing summary, distinct from the full objective.
    #[serde(default)]
    pub summary: String,
    /// The objective the packet should achieve (the "why"), distinct from the
    /// machine summary used for de-duplication.
    pub objective: String,
    /// Provider-neutral task classification: fix | feature | refactor |
    /// docs | test | chore.
    pub task_class: String,
    /// easy | medium | hard.
    pub difficulty: String,
    /// low | medium | high.
    pub risk: String,
    /// How the packet should be executed: autonomous | supervised |
    /// human_required.
    pub execution_disposition: String,
    /// Capability/difficulty-based routing hint. Does NOT name a model.
    pub recommended_routing: RecommendedRouting,
    /// Repo areas touched (e.g. "area:controller", "auth").
    #[serde(default)]
    pub affected_areas: Vec<String>,
    /// Files expected to change.
    #[serde(default)]
    pub affected_files: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub verification_commands: Vec<String>,
    /// Plan-local keys of packets that must complete first.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Prior evidence the planner cited for why this isn't duplicate work.
    #[serde(default)]
    pub duplicate_evidence: Vec<String>,
    /// Why this gap isn't already covered.
    #[serde(default)]
    pub uncovered_reason: String,
}

/// TICKET-544: recommended routing expressed as a capability requirement and
/// a difficulty tier, intentionally provider/model-neutral.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecommendedRouting {
    /// Capability the backend must provide: edit | plan | review | research.
    pub capability: String,
    /// Minimum capability tier implied by difficulty/risk: standard | strong.
    #[serde(default = "default_routing_tier")]
    pub min_tier: String,
}

fn default_routing_tier() -> String {
    "standard".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PmPlan {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub tickets: Vec<PlannerWorkPacket>,
}

/// One repair-driving review finding. Free-form `blocking_findings` remain in
/// the wire model for historical ledger and provider-comment compatibility,
/// but new reviewer output must supply these typed facts before GAH may route
/// a FixMr. The controller, rather than the reviewer, validates `status`, the
/// changed-file identity, and the evidence grammar.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionableReviewFinding {
    pub summary: String,
    pub file: String,
    #[serde(default)]
    pub line: Option<String>,
    pub status: String,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub evidence: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReviewVerdict {
    #[serde(deserialize_with = "deserialize_review_verdict")]
    pub verdict: String,
    #[serde(deserialize_with = "deserialize_flexible_string")]
    pub confidence: String,
    pub human_required: bool,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub blocking_findings: Vec<String>,
    /// Machine-verifiable repair instructions. A NEEDS_FIX/REJECT result is
    /// review output, not repair context, until every item in this list is
    /// validated against the control-plane diff bundle.
    #[serde(default)]
    pub actionable_findings: Vec<ActionableReviewFinding>,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub non_blocking_findings: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub risk_notes: Vec<String>,
    /// Concrete, reviewable facts supporting an approval (for example a test
    /// name/result, a changed file/line, or an explicit compatibility check).
    /// This is supplied by the reviewer; an empty list makes an APPROVE
    /// non-mergeable in dispatch's evidence gate.
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub evidence: Vec<String>,
    /// Required when the review identifies a schema/API/persistence contract
    /// change but still recommends approval. It must state the versioned
    /// compatibility or migration evidence that makes the change safe.
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub compatibility_evidence: Vec<String>,
    /// Set only by GAH's deterministic evidence gate, never trusted from the
    /// reviewer's JSON. Persisted so operators can see why an apparent
    /// approval was made non-mergeable.
    #[serde(default)]
    pub safety_gate_reason: Option<String>,
    #[serde(default)]
    pub reviewer_backend: Option<String>,
    #[serde(default)]
    pub reviewer_model: Option<String>,
    /// TICKET-108 / Issue #123: reviewer authority tier
    /// ("strong"/"escalatory"/"standard"/"weak"), derived from routing (which
    /// config field selected this backend), not from anything the LLM reports.
    /// Populated by us, never parsed from the model's JSON response.
    #[serde(default)]
    pub reviewer_tier: Option<String>,
    /// TICKET-109: capabilities actually activated for this review turn
    /// (e.g. `["ponytail"]`). Populated by us -- see src/capability.rs --
    /// so review artifacts record the capability policy that was applied.
    #[serde(default)]
    pub applied_capabilities: Vec<String>,
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

/// Reviewers sometimes answer `confidence` with a raw number (e.g. `0.78`)
/// instead of the requested "high"/"medium"/"low" string -- same class of
/// prompt-adherence drift TICKET-102 already hardened for the findings
/// fields. Accept a number and preserve it as its string form rather than
/// crashing the whole verdict parse over one field.
fn deserialize_flexible_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        other => Err(serde::de::Error::custom(format!(
            "expected string or number, got {other}"
        ))),
    }
}

/// Verdicts are a closed protocol value. Normalize harmless model casing and
/// whitespace before dispatch applies its deterministic safety policy.
fn deserialize_review_verdict<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let verdict = deserialize_flexible_string(deserializer)?;
    Ok(verdict.trim().to_ascii_uppercase())
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
            r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":["a"],"non_blocking_findings":["b"],"risk_notes":["c"]}"#,
        )
        .unwrap();
        assert_eq!(verdict.blocking_findings, vec!["a"]);
        assert_eq!(verdict.non_blocking_findings, vec!["b"]);
        assert_eq!(verdict.risk_notes, vec!["c"]);
    }

    #[test]
    fn review_verdict_accepts_numeric_confidence() {
        // Regression: Claude returned a raw float confidence score (0.78)
        // instead of "high"/"medium"/"low", crashing the whole verdict
        // parse with "invalid type: floating point, expected a string".
        let verdict: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE","confidence":0.78,"human_required":false,"blocking_findings":[],"non_blocking_findings":[],"risk_notes":[]}"#,
        )
        .unwrap();
        assert_eq!(verdict.confidence, "0.78");
    }

    #[test]
    fn review_verdict_normalizes_case_and_whitespace() {
        let verdict: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":" Approve ","confidence":"high","human_required":false}"#,
        )
        .unwrap();
        assert_eq!(verdict.verdict, "APPROVE");
    }

    #[test]
    fn review_verdict_normalizes_single_strings() {
        let verdict: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":"a","non_blocking_findings":"b","risk_notes":"c"}"#,
        )
        .unwrap();
        assert_eq!(verdict.blocking_findings, vec!["a"]);
        assert_eq!(verdict.non_blocking_findings, vec!["b"]);
        assert_eq!(verdict.risk_notes, vec!["c"]);
    }

    #[test]
    fn review_verdict_normalizes_null_and_missing_lists() {
        let with_null: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE","confidence":"high","human_required":false,"blocking_findings":null,"non_blocking_findings":null,"risk_notes":null}"#,
        )
        .unwrap();
        assert!(with_null.blocking_findings.is_empty());
        assert!(with_null.non_blocking_findings.is_empty());
        assert!(with_null.risk_notes.is_empty());

        let missing: ReviewVerdict = serde_json::from_str(
            r#"{"verdict":"APPROVE","confidence":"high","human_required":false}"#,
        )
        .unwrap();
        assert!(missing.blocking_findings.is_empty());
        assert!(missing.non_blocking_findings.is_empty());
        assert!(missing.risk_notes.is_empty());
    }

    #[test]
    fn review_verdict_still_rejects_malformed_json() {
        serde_json::from_str::<ReviewVerdict>(
            r#"{"verdict":"APPROVE","confidence":"high","human_required":false"#,
        )
        .unwrap_err();
    }
}
