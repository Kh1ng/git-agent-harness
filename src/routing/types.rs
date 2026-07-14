use std::collections::{HashMap, HashSet};
use std::fmt;

/// Request input to the routing decision engine.
#[derive(Debug, Clone)]
pub struct RouteRequest<'a> {
    pub mode: &'a str,
    pub requested_backend: &'a str,
    pub requested_model: Option<&'a str>,
    pub recommended_backend: Option<&'a str>,
    pub recommended_model: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub usage_summary: Option<crate::ledger::BackendUsageSummary>,
    /// TICKET-089 AC7/8: the `FailureClass::as_str()` of the immediately
    /// preceding attempt, when this route decision is a same-invocation
    /// retry. Only `agent_failure`/`agent_no_progress`/`validation_failure`
    /// (genuine agent-capability failures) may escalate candidate ordering
    /// toward a stronger model; harness/environment/backend (auth/quota)
    /// failures must not, since a stronger model doesn't fix those.
    pub last_failure_class: Option<&'a str>,
}

/// Dynamic, per-dispatch facts that must not be baked into static routing
/// configuration. Equal-priority candidates are balanced by recent execution
/// count; genuine capability escalation excludes backend/model pairs already
/// tried for this work item; paid routes remain ineligible until an operator
/// grants that exact pair.
#[derive(Debug, Clone, Default)]
pub struct RoutingRuntimeState {
    pub recent_runs: HashMap<CandidateIdentity, u64>,
    pub attempted: HashSet<CandidateIdentity>,
    pub approved: HashSet<CandidateIdentity>,
}

/// Unique identifier for a backend+model combination used for tracking
/// availability, recent usage, and approvals.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateIdentity {
    pub backend: String,
    pub model: Option<String>,
}

impl CandidateIdentity {
    pub fn new(backend: impl Into<String>, model: Option<impl Into<String>>) -> Self {
        Self {
            backend: backend.into(),
            model: model.map(Into::into),
        }
    }
}

/// Trusted ticket metadata used only to choose an operator-configured
/// implementation candidate list. This is intentionally separate from
/// `RouteRequest`: ordinary routing callers (reviews, PM, CLI overrides) keep
/// their current behavior unless they explicitly opt into task routing.
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskRoutingContext<'a> {
    pub task_class: Option<&'a str>,
    pub difficulty: Option<&'a str>,
    pub risk: Option<&'a str>,
}

/// The final routing decision including backend, model, and metadata about
/// how the decision was made.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub requested_backend: String,
    pub effective_backend: String,
    pub requested_model: Option<String>,
    pub effective_model: Option<String>,
    pub effective_quota_pool: Option<String>,
    pub routing_reason: String,
    pub fallback_used: bool,
    pub confidence_impact: Option<String>,
    pub human_required: bool,
    pub routing_diagnostics: Option<crate::ledger::RoutingDiagnostics>,
}

/// A backend+model combination that was considered but not selected, along
/// with the reason it was skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedBackend {
    pub backend: String,
    pub model: Option<String>,
    pub reason: String,
    pub unavailable_until: Option<String>,
}

/// Error returned when routing cannot find an eligible backend+model combination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    NoEligibleBackend {
        preferred_backend: String,
        preferred_model: Option<String>,
        skipped: Vec<SkippedBackend>,
        earliest_reset: Option<String>,
    },
    ApprovalRequired {
        backend: String,
        model: Option<String>,
        skipped: Vec<SkippedBackend>,
    },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::NoEligibleBackend {
                preferred_backend,
                preferred_model,
                skipped,
                earliest_reset,
            } => {
                write!(
                    f,
                    "no eligible backend available for preferred {}",
                    candidate_label(preferred_backend, preferred_model.as_deref())
                )?;
                if !skipped.is_empty() {
                    write!(f, "; skipped: {}", render_skips(skipped))?;
                }
                if let Some(reset) = earliest_reset {
                    write!(f, "; earliest reset: {}", reset)?;
                }
                Ok(())
            }
            RouteError::ApprovalRequired {
                backend,
                model,
                skipped,
            } => {
                write!(
                    f,
                    "operator approval required before using paid route {}",
                    candidate_label(backend, model.as_deref())
                )?;
                if !skipped.is_empty() {
                    write!(f, "; skipped: {}", render_skips(skipped))?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for RouteError {}

/// Helper function to create a human-readable label for a backend+model combination.
pub(crate) fn candidate_label(backend: &str, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{backend}/{model}"),
        None => backend.to_string(),
    }
}

/// Render a list of skipped backends for error messages.
pub(crate) fn render_skips(skipped: &[SkippedBackend]) -> String {
    skipped
        .iter()
        .map(|skip| {
            let mut summary = format!(
                "{}: {}",
                candidate_label(&skip.backend, skip.model.as_deref()),
                skip.reason
            );
            if let Some(until) = &skip.unavailable_until {
                summary.push_str(" until ");
                summary.push_str(until);
            }
            summary
        })
        .collect::<Vec<_>>()
        .join(", ")
}
