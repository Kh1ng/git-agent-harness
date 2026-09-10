//! Merge policy for work results (Issue #124 / TICKET-127). Split from
//! `config.rs` to stay under the source-size guard.

use serde::{Deserialize, Serialize};

/// TICKET-127/Issue #124: per-repo merge policy controlling what the
/// controller does once an MR is `READY_FOR_HUMAN` (strong reviewer approved)
/// and CI has been evaluated.
///
/// * `Auto` (default): current behavior -- strong review + green CI triggers
///   `MergeMr` (GAH merges itself).
/// * `StopForHuman`: strong review done + CI evaluated -> `HumanRequired`; GAH
///   never auto-merges, an operator clicks merge manually.
/// * `GitlabMwps`: after strong approval GAH sets GitLab's "merge when pipeline
///   succeeds" flag and does NOT merge itself; GitLab enforces the CI gate
///   natively. Only meaningful for GitLab; other providers fall back to `Auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MergePolicy {
    #[default]
    Auto,
    StopForHuman,
    GitlabMwps,
}

impl MergePolicy {
    /// Canonical config string for a merge policy (Issue #124 / TICKET-127).
    pub fn as_str(&self) -> &'static str {
        match self {
            MergePolicy::Auto => "auto",
            MergePolicy::StopForHuman => "stop_for_human",
            MergePolicy::GitlabMwps => "gitlab_mwps",
        }
    }
}
