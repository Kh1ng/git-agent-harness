use super::{issue_intake, IssueIntakeMode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// Existing provider labels to apply to PM-published children. Values are
    /// label names, keyed by the planner's normalized value (for example
    /// `easy = "difficulty:easy"`). GAH never creates missing labels.
    #[serde(default)]
    pub pm_difficulty_labels: BTreeMap<String, String>,
    #[serde(default)]
    pub pm_risk_labels: BTreeMap<String, String>,
    #[serde(default)]
    pub pm_execution_labels: BTreeMap<String, String>,
    /// Provider labels that select controller-driven PM decomposition instead
    /// of direct implementation. Matching is case-insensitive.
    #[serde(default = "default_pm_decomposition_labels")]
    pub pm_decomposition_labels: Vec<String>,
    /// Maximum children one controller-published plan may contain.
    #[serde(default = "default_pm_max_children")]
    pub pm_max_children: u32,
    /// Maximum generated-plan ancestry depth. A normal provider issue starts
    /// at depth zero; its generated children are depth one.
    #[serde(default = "default_pm_max_depth")]
    pub pm_max_depth: u32,
    /// Maximum failed PM planning/publication attempts before the source item
    /// is surfaced for human attention.
    #[serde(default = "default_pm_max_attempts")]
    pub pm_max_attempts: u32,
    /// Hard wall-clock ceiling shared by all backend attempts in one PM plan.
    #[serde(default = "default_pm_timeout_seconds")]
    pub pm_timeout_seconds: u64,
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
            pm_difficulty_labels: BTreeMap::new(),
            pm_risk_labels: BTreeMap::new(),
            pm_execution_labels: BTreeMap::new(),
            pm_decomposition_labels: default_pm_decomposition_labels(),
            pm_max_children: default_pm_max_children(),
            pm_max_depth: default_pm_max_depth(),
            pm_max_attempts: default_pm_max_attempts(),
            pm_timeout_seconds: default_pm_timeout_seconds(),
            generated_artifact_deny_patterns: crate::generated_artifacts::default_deny_patterns(),
        }
    }
}

fn default_pm_decomposition_labels() -> Vec<String> {
    vec!["planning".to_string(), "plan".to_string()]
}

fn default_pm_max_children() -> u32 {
    12
}

fn default_pm_max_depth() -> u32 {
    1
}

fn default_pm_max_attempts() -> u32 {
    2
}

fn default_pm_timeout_seconds() -> u64 {
    900
}

impl PublishingPolicy {
    pub fn pm_decomposition_labels(&self) -> Vec<String> {
        let labels = self
            .pm_decomposition_labels
            .iter()
            .map(|label| label.trim())
            .filter(|label| !label.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if labels.is_empty() {
            default_pm_decomposition_labels()
        } else {
            labels
        }
    }

    pub fn pm_max_children(&self) -> usize {
        self.pm_max_children.clamp(1, 24) as usize
    }

    pub fn pm_max_depth(&self) -> u32 {
        self.pm_max_depth.clamp(1, 8)
    }

    pub fn pm_max_attempts(&self) -> usize {
        self.pm_max_attempts.clamp(1, 10) as usize
    }

    pub fn pm_timeout_seconds(&self) -> u64 {
        self.pm_timeout_seconds.clamp(30, 7_200)
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::PublishingPolicy;

    #[test]
    fn pm_orchestration_defaults_are_bounded() {
        let policy: PublishingPolicy = toml::from_str("").unwrap();
        assert_eq!(policy.pm_decomposition_labels, ["planning", "plan"]);
        assert_eq!(policy.pm_max_children(), 12);
        assert_eq!(policy.pm_max_depth(), 1);
        assert_eq!(policy.pm_max_attempts(), 2);
        assert_eq!(policy.pm_timeout_seconds(), 900);
    }

    #[test]
    fn empty_decomposition_label_list_falls_back_fail_safe() {
        let policy: PublishingPolicy = toml::from_str("pm_decomposition_labels=[]").unwrap();
        assert_eq!(policy.pm_decomposition_labels(), ["planning", "plan"]);
    }

    #[test]
    fn pm_orchestration_limits_are_safely_clamped() {
        let policy: PublishingPolicy = toml::from_str(
            "pm_max_children=100\npm_max_depth=99\npm_max_attempts=0\npm_timeout_seconds=1",
        )
        .unwrap();
        assert_eq!(policy.pm_max_children(), 24);
        assert_eq!(policy.pm_max_depth(), 8);
        assert_eq!(policy.pm_max_attempts(), 1);
        assert_eq!(policy.pm_timeout_seconds(), 30);
    }
}
