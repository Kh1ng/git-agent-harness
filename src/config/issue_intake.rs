use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueIntakeMode {
    Legacy,
    CanonicalAutonomousOnly,
}

impl IssueIntakeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueIntakeMode::Legacy => "legacy",
            IssueIntakeMode::CanonicalAutonomousOnly => "canonical_autonomous_only",
        }
    }
}

pub(crate) fn default_issue_intake_mode() -> IssueIntakeMode {
    IssueIntakeMode::CanonicalAutonomousOnly
}

pub(crate) fn default_canonical_autonomous_label() -> String {
    "exec:autonomous".to_string()
}
