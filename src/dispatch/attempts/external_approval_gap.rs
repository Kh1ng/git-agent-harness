//! Issue #653: declared external credential scopes without an active grant.
//! Split from `attempts.rs` to stay under the source-size guard. The gap
//! pauses the dispatch before backend launch; the typed error is handled in
//! the workflow layer.

use super::*;
use crate::config::{GahConfig, Profile};
use crate::runner;

/// Issue #653: one declared external credential scope whose env vars exist
/// in the operator's environment file but are not covered by an active grant
/// for this work item. Each gap pauses the dispatch before backend launch.
#[derive(Debug, Clone)]
pub(crate) struct ExternalCredentialGap {
    pub label: String,
    pub env_vars: Vec<String>,
    pub max_requests: Option<u64>,
    pub max_dollars: Option<f64>,
    pub purpose: Option<String>,
}

/// Issue #653: typed pause signal. Never a backend failure and never a
/// generic human_required — the dispatch stopped before launch because a
/// declared external credential scope lacks an active grant.
#[derive(Debug)]
pub(crate) struct ExternalApprovalRequiredError {
    pub profile: String,
    pub work_id: String,
    pub gaps: Vec<ExternalCredentialGap>,
}

impl std::fmt::Display for ExternalApprovalRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let labels = self
            .gaps
            .iter()
            .map(|gap| gap.label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            f,
            "work {} on profile {} needs external API approval (credential scope(s): {labels})",
            self.work_id, self.profile
        )
    }
}

impl std::error::Error for ExternalApprovalRequiredError {}

/// Issue #653: declared external credential scopes whose env vars exist in
/// the operator's environment file but were not granted for this work item.
/// Requires a resolvable work_id — without one there is no approval scope
/// and the existing fail-closed stripping applies.
pub(crate) fn external_approval_gaps_for_work_item(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    work_id: Option<&str>,
    env_path: Option<&str>,
) -> Vec<ExternalCredentialGap> {
    let Some(work_id) = work_id else {
        return Vec::new();
    };
    if profile.external_credential_scopes.is_empty() {
        return Vec::new();
    }
    let Ok(entries) = ledger::read_entries(cfg) else {
        return Vec::new();
    };
    let allowed = ledger::active_external_approval_env_vars_from_entries(
        &entries,
        profile_name,
        &profile.repo_id,
        work_id,
    );
    let env_vars = env_path.map(runner::load_env_file).unwrap_or_default();
    let present: std::collections::HashSet<String> =
        env_vars.iter().map(|(key, _)| key.clone()).collect();
    let mut gaps: Vec<ExternalCredentialGap> = profile
        .external_credential_scopes
        .iter()
        .map(|(label, scope)| ExternalCredentialGap {
            label: label.clone(),
            env_vars: scope
                .env_vars
                .iter()
                .filter(|var| present.contains(*var) && !allowed.contains(*var))
                .cloned()
                .collect(),
            max_requests: scope.max_requests,
            max_dollars: scope.max_dollars,
            purpose: scope.purpose.clone(),
        })
        .filter(|gap| !gap.env_vars.is_empty())
        .collect();
    gaps.sort_by(|left, right| left.label.cmp(&right.label));
    gaps
}
