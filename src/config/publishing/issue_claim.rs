use serde::{Deserialize, Serialize};

/// Where a profile records that it is working on a provider issue.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IssueClaimMode {
    /// Claims stay on this machine. Nothing is written to the provider.
    #[default]
    Local,
    /// The GitHub assignee plus a claim comment is the claim, so every loop
    /// working the repository sees it. The machine-local claim still applies.
    GithubAssignee,
}

/// Per-profile settings for provider-visible issue claims.
///
/// Loops that share a repository must agree on `ttl_minutes` and
/// `priority_logins`: each loop decides a contested claim from its own copy,
/// and two different copies can both decide they won.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct IssueClaimPolicy {
    #[serde(default)]
    pub mode: IssueClaimMode,
    /// Minutes a claim holds an issue, counted from the claim comment. The
    /// limit is hard: work in progress does not extend it. Only an open pull
    /// request for the issue keeps it held afterwards.
    #[serde(default = "default_ttl_minutes")]
    pub ttl_minutes: u32,
    /// Seconds to wait between posting a claim and re-reading the issue to
    /// see whether another loop claimed it at the same moment. Keep it longer
    /// than the slowest loop's poll interval.
    #[serde(default = "default_verify_seconds")]
    pub verify_seconds: u32,
    /// Logins that win a contested claim, most preferred first. Without a
    /// listed contender the earliest claim comment wins.
    #[serde(default)]
    pub priority_logins: Vec<String>,
}

impl Default for IssueClaimPolicy {
    fn default() -> Self {
        Self {
            mode: IssueClaimMode::default(),
            ttl_minutes: default_ttl_minutes(),
            verify_seconds: default_verify_seconds(),
            priority_logins: Vec::new(),
        }
    }
}

fn default_ttl_minutes() -> u32 {
    60
}

fn default_verify_seconds() -> u32 {
    60
}

#[cfg(test)]
mod tests {
    use super::{IssueClaimMode, IssueClaimPolicy};

    #[test]
    fn an_unconfigured_profile_keeps_claims_local() {
        let policy: IssueClaimPolicy = toml::from_str("").unwrap();
        assert_eq!(policy, IssueClaimPolicy::default());
        assert_eq!(policy.mode, IssueClaimMode::Local);
        assert_eq!((policy.ttl_minutes, policy.verify_seconds), (60, 60));
    }

    #[test]
    fn an_unknown_claim_mode_is_refused() {
        assert!(toml::from_str::<IssueClaimPolicy>("mode = \"assignee\"").is_err());
    }
}
