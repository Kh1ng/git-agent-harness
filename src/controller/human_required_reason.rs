//! TICKET-505: Stable reason codes for HumanRequired transitions.
//!
//! Every HumanRequired transition emits a stable reason code plus its existing
//! redacted human-readable explanation. Reason codes distinguish at least:
//! - policy/approval
//! - retry-budget exhaustion
//! - review-evidence gate
//! - merge policy
//! - publishing restriction
//! - configuration/infra
//! - unknown legacy records
//!
//! The code is a plain lowercase string on the wire (not a serde-tagged enum),
//! matching this codebase's existing convention for enum-like ledger fields
//! (e.g. `FailureClass`, `FailureStage`). This ensures the wire format never
//! breaks if variants are renamed internally.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable reason codes for why autonomy stopped and requires human judgment.
///
/// These are deliberately broad categories that cover all current and known
/// future HumanRequired paths. Each code maps to exactly one semantic reason,
/// and historical records without a code deserialize as `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HumanRequiredReason {
    /// Policy or approval gate requires human judgment (e.g., StopForHuman merge
    /// policy, explicit profile-level human gate).
    PolicyApproval,
    /// Retry budget exhausted for a ticket (AUTO_RETRY_CAP / implementation
    /// failure cap reached).
    RetryBudgetExhausted,
    /// Review evidence gate: a reviewer's verdict explicitly marked the work as
    /// requiring human judgment (READY_FOR_HUMAN with human_required=true in ledger).
    ReviewEvidenceGate,
    /// Review hard-ceiling exhaustion: a healthy reviewer that kept making
    /// progress but exceeded the explicit wall-clock safety ceiling (issue #540).
    /// Deliberately NOT a backend failure, so it must not trigger retry/escalation.
    ReviewCeilingExhausted,
    /// Merge policy forbids auto-merge (StopForHuman) even with strong approval
    /// and green CI.
    MergePolicy,
    /// Publishing restriction: profile has PR/MR creation disabled
    /// (allow_pull_request_creation == false).
    PublishingRestriction,
    /// Configuration or infrastructure failure at the profile level (sync
    /// failure, invalid config, required infra unavailable, auth failure with no
    /// viable route).
    ConfigurationInfra,
    /// Fix retry cap exceeded for an MR (max_fix_attempts_per_mr reached).
    FixRetryCapExceeded,
    /// Merge retry cap exceeded for an MR (AUTO_RETRY_CAP merge attempts reached).
    MergeRetryCapExceeded,
    /// Unknown reason - for historical records without a code or genuinely
    /// unclassifiable cases. Missing data is never inferred as a different reason.
    #[default]
    Unknown,
}

impl HumanRequiredReason {
    /// Return the stable wire-format string for this reason code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PolicyApproval => "policy_approval",
            Self::RetryBudgetExhausted => "retry_budget_exhausted",
            Self::ReviewEvidenceGate => "review_evidence_gate",
            Self::ReviewCeilingExhausted => "review_ceiling_exhausted",
            Self::MergePolicy => "merge_policy",
            Self::PublishingRestriction => "publishing_restriction",
            Self::ConfigurationInfra => "configuration_infra",
            Self::FixRetryCapExceeded => "fix_retry_cap_exceeded",
            Self::MergeRetryCapExceeded => "merge_retry_cap_exceeded",
            Self::Unknown => "unknown",
        }
    }

    /// Return a human-readable description of this reason code.
    #[allow(dead_code)]
    pub fn description(self) -> &'static str {
        match self {
            Self::PolicyApproval => "Policy or approval requires human judgment",
            Self::RetryBudgetExhausted => "Retry budget exhausted",
            Self::ReviewEvidenceGate => "Review evidence gate requires human judgment",
            Self::ReviewCeilingExhausted => "Review hard-ceiling exhausted",
            Self::MergePolicy => "Merge policy forbids auto-merge",
            Self::PublishingRestriction => "Publishing policy forbids PR/MR creation",
            Self::ConfigurationInfra => "Configuration or infrastructure failure",
            Self::FixRetryCapExceeded => "Fix retry cap exceeded for MR",
            Self::MergeRetryCapExceeded => "Merge retry cap exceeded for MR",
            Self::Unknown => "Unknown reason",
        }
    }

    /// Parse a reason code from its wire-format string.
    /// Returns `Unknown` for unrecognized strings to ensure historical records
    /// without a code deserialize as unknown.
    pub fn from_code(s: &str) -> Self {
        match s {
            "policy_approval" => Self::PolicyApproval,
            "retry_budget_exhausted" => Self::RetryBudgetExhausted,
            "review_evidence_gate" => Self::ReviewEvidenceGate,
            "review_ceiling_exhausted" => Self::ReviewCeilingExhausted,
            "merge_policy" => Self::MergePolicy,
            "publishing_restriction" => Self::PublishingRestriction,
            "configuration_infra" => Self::ConfigurationInfra,
            "fix_retry_cap_exceeded" => Self::FixRetryCapExceeded,
            "merge_retry_cap_exceeded" => Self::MergeRetryCapExceeded,
            _ => Self::Unknown,
        }
    }

    /// Returns all known reason codes for table-driven testing.
    #[allow(dead_code)]
    pub fn all() -> &'static [Self] {
        &[
            Self::PolicyApproval,
            Self::RetryBudgetExhausted,
            Self::ReviewEvidenceGate,
            Self::ReviewCeilingExhausted,
            Self::MergePolicy,
            Self::PublishingRestriction,
            Self::ConfigurationInfra,
            Self::FixRetryCapExceeded,
            Self::MergeRetryCapExceeded,
            Self::Unknown,
        ]
    }
}

impl<'de> Deserialize<'de> for HumanRequiredReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(HumanRequiredReason::from_code(&s))
    }
}

impl fmt::Display for HumanRequiredReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for HumanRequiredReason {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(HumanRequiredReason::from_code(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_reason_codes_have_stable_string_representations() {
        for reason in HumanRequiredReason::all() {
            let s = reason.as_str();
            // All codes are lowercase snake_case
            assert!(s.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
            // Round-trip through from_str
            assert_eq!(HumanRequiredReason::from_code(s), *reason);
        }
    }

    #[test]
    fn unknown_string_parses_to_unknown() {
        assert_eq!(
            HumanRequiredReason::from_code("completely_unknown_code"),
            HumanRequiredReason::Unknown
        );
        assert_eq!(
            HumanRequiredReason::from_code(""),
            HumanRequiredReason::Unknown
        );
    }

    #[test]
    fn reason_code_serialization_is_plain_string() {
        use serde_json::json;

        // Serialize as plain string
        let reason = HumanRequiredReason::RetryBudgetExhausted;
        let serialized = serde_json::to_value(reason.as_str()).unwrap();
        assert_eq!(serialized, json!("retry_budget_exhausted"));

        // Deserialization from string
        let deserialized: HumanRequiredReason =
            serde_json::from_value(json!("merge_policy")).unwrap();
        assert_eq!(deserialized, HumanRequiredReason::MergePolicy);

        // Unknown code deserializes to Unknown
        let unknown: HumanRequiredReason = serde_json::from_value(json!("some_unknown")).unwrap();
        assert_eq!(unknown, HumanRequiredReason::Unknown);
    }

    #[test]
    fn display_format_matches_as_str() {
        let reason = HumanRequiredReason::PublishingRestriction;
        assert_eq!(format!("{}", reason), reason.as_str());
    }

    #[test]
    fn default_is_unknown() {
        assert_eq!(HumanRequiredReason::default(), HumanRequiredReason::Unknown);
    }

    // TICKET-505: Integration tests with NextAction::HumanRequired
    use super::super::action::NextAction;

    #[test]
    fn human_required_action_with_reason_code_serialization() {
        let action = NextAction::HumanRequired {
            reason: "Merge policy forbids auto-merge".into(),
            reference: Some("https://example.com/mr/1".into()),
            reason_code: Some("merge_policy".into()),
        };

        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("\"reason_code\":\"merge_policy\""));
        assert!(json.contains("\"reason\":\"Merge policy forbids auto-merge\""));

        // Verify deserialization
        let parsed: NextAction = serde_json::from_str(&json).unwrap();
        match parsed {
            NextAction::HumanRequired { reason_code, .. } => {
                assert_eq!(reason_code, Some("merge_policy".into()));
            }
            _ => panic!("Expected HumanRequired action"),
        }
    }

    #[test]
    fn human_required_action_without_reason_code_deserializes_as_none() {
        let json = r#"{"type":"HumanRequired","reason":"test","reference":null}"#;
        let parsed: NextAction = serde_json::from_str(json).unwrap();
        match parsed {
            NextAction::HumanRequired { reason_code, .. } => {
                assert_eq!(reason_code, None);
            }
            _ => panic!("Expected HumanRequired action"),
        }
    }

    #[test]
    fn all_reason_codes_are_valid_for_human_required() {
        for reason in HumanRequiredReason::all() {
            let code = reason.as_str();
            let action = NextAction::HumanRequired {
                reason: format!("Test reason for {code}"),
                reference: None,
                reason_code: Some(code.to_string()),
            };

            // Verify serialization
            let json = serde_json::to_string(&action).unwrap();
            assert!(json.contains(&format!("\"reason_code\":\"{code}\"")));

            // Verify deserialization
            let parsed: NextAction = serde_json::from_str(&json).unwrap();
            match parsed {
                NextAction::HumanRequired {
                    reason_code: Some(ref parsed_code),
                    ..
                } => {
                    assert_eq!(parsed_code, code);
                }
                _ => panic!("Expected HumanRequired with reason_code for {code}"),
            }
        }
    }
}
