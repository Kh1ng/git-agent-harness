use super::{issue_intake, IssueIntakeMode};
use serde::{Deserialize, Serialize};

/// Per-profile policy for human-facing repository messaging and safe
/// publication boundaries. This remains independent from reviewer routing
/// and merge authorization.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct PublishingPolicy {
    #[serde(default = "default_true")]
    pub allow_pull_request_creation: bool,
    #[serde(default = "default_true")]
    pub allow_commit_message_generation: bool,
    #[serde(default = "default_true")]
    pub allow_issue_comments: bool,
    #[serde(default)]
    pub allow_source_issue_closure: bool,
    #[serde(default)]
    pub github_issue_author_allowlist: Option<Vec<String>>,
    #[serde(default)]
    pub trusted_issue_human_authors: Option<Vec<String>>,
    #[serde(default)]
    pub trusted_issue_bot_authors: Option<Vec<String>>,
    #[serde(default = "issue_intake::default_issue_intake_mode")]
    pub issue_intake_mode: IssueIntakeMode,
    #[serde(default = "issue_intake::default_canonical_autonomous_label")]
    pub canonical_autonomous_label: String,
    /// Gitignore-style path patterns that newly tracked files must not match
    /// before GAH creates a commit or pushes a backend-authored commit.
    /// Explicit `[]` disables the guard for a profile.
    #[serde(default = "crate::generated_artifacts::default_deny_patterns")]
    pub generated_artifact_deny_patterns: Vec<String>,
}

impl Default for PublishingPolicy {
    fn default() -> Self {
        Self {
            allow_pull_request_creation: true,
            allow_commit_message_generation: true,
            allow_issue_comments: true,
            allow_source_issue_closure: false,
            github_issue_author_allowlist: None,
            trusted_issue_human_authors: None,
            trusted_issue_bot_authors: None,
            issue_intake_mode: issue_intake::default_issue_intake_mode(),
            canonical_autonomous_label: issue_intake::default_canonical_autonomous_label(),
            generated_artifact_deny_patterns: crate::generated_artifacts::default_deny_patterns(),
        }
    }
}

fn default_true() -> bool {
    true
}
