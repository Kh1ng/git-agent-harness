use serde::{Deserialize, Serialize};

/// Issue #125: delivery mode for work results.
/// * `Pr` (default): standard behavior -- open/merge PRs/MRs, close issues on satisfaction.
/// * `Handoff`: read-only profile mode -- performs work & local validation, writes diff/patch + summary report to `artifact_root/handoffs/<ticket>/`, notifies operator, records in ledger (mode = "handoff", no mr_url), and NEVER calls remote write operations (gh pr create, gh pr merge, gh issue close).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryMode {
    #[default]
    Pr,
    Handoff,
}

impl DeliveryMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeliveryMode::Pr => "pr",
            DeliveryMode::Handoff => "handoff",
        }
    }
}

impl std::fmt::Display for DeliveryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
