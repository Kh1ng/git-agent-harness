use serde::{Deserialize, Serialize};

/// How much automatic effort one issue may consume before the controller
/// holds it for a human (`[profiles.<name>.routing.issue_budget]`, with
/// `[defaults.routing.issue_budget]` as the fallback for every profile).
///
/// Three independent axes, each unlimited when unset:
///
/// * `max_attempts`: worker attempts on the issue, counted across every
///   recorded dispatch including ones that failed in setup (preflight,
///   backend launch, environment), not only genuine agent failures. This is
///   deliberately stricter than `max_implementation_failures_per_ticket`.
/// * `max_elapsed_minutes`: summed worker and review time recorded for the
///   issue.
/// * `max_manager_rounds`: review rounds (each review verdict is one round).
///
/// `escalation_reserve_attempts` keeps the last attempts of `max_attempts`
/// for escalation to a stronger backend/model only: once the remaining
/// attempts are inside the reserve, a plain retry of the same tier is
/// refused. Defaults to one attempt when `max_attempts` is set.
///
/// A `gah clear-attempts` tombstone resets every counter, the same way it
/// resets the retry cap.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct IssueBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_elapsed_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_manager_rounds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_reserve_attempts: Option<u32>,
}

impl IssueBudget {
    /// True when at least one axis is limited; an unlimited budget is never
    /// evaluated, so profiles without the section keep today's behaviour.
    pub fn is_enforced(&self) -> bool {
        self.max_attempts.is_some()
            || self.max_elapsed_minutes.is_some()
            || self.max_manager_rounds.is_some()
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Attempts held back for escalation. Never more than `max_attempts`.
    pub fn escalation_reserve_attempts(&self) -> u32 {
        let reserve = self.escalation_reserve_attempts.unwrap_or(1);
        match self.max_attempts {
            Some(max) => reserve.min(max),
            None => reserve,
        }
    }

    /// Profile values win; unset profile axes inherit the canonical ones.
    pub(crate) fn merged_with(self, canonical: IssueBudget) -> IssueBudget {
        IssueBudget {
            max_attempts: self.max_attempts.or(canonical.max_attempts),
            max_elapsed_minutes: self.max_elapsed_minutes.or(canonical.max_elapsed_minutes),
            max_manager_rounds: self.max_manager_rounds.or(canonical.max_manager_rounds),
            escalation_reserve_attempts: self
                .escalation_reserve_attempts
                .or(canonical.escalation_reserve_attempts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::IssueBudget;

    #[test]
    fn unset_budget_is_not_enforced() {
        assert!(!IssueBudget::default().is_enforced());
        assert!(IssueBudget {
            max_elapsed_minutes: Some(90),
            ..IssueBudget::default()
        }
        .is_enforced());
    }

    #[test]
    fn reserve_defaults_to_one_and_never_exceeds_max_attempts() {
        let budget = IssueBudget {
            max_attempts: Some(3),
            ..IssueBudget::default()
        };
        assert_eq!(budget.escalation_reserve_attempts(), 1);
        let budget = IssueBudget {
            max_attempts: Some(1),
            escalation_reserve_attempts: Some(4),
            ..IssueBudget::default()
        };
        assert_eq!(budget.escalation_reserve_attempts(), 1);
    }

    #[test]
    fn profile_axes_override_canonical_axes_individually() {
        let canonical = IssueBudget {
            max_attempts: Some(4),
            max_elapsed_minutes: Some(120),
            max_manager_rounds: Some(3),
            escalation_reserve_attempts: Some(2),
        };
        let profile = IssueBudget {
            max_attempts: Some(2),
            ..IssueBudget::default()
        };
        let merged = profile.merged_with(canonical);
        assert_eq!(merged.max_attempts, Some(2));
        assert_eq!(merged.max_elapsed_minutes, Some(120));
        assert_eq!(merged.max_manager_rounds, Some(3));
        assert_eq!(merged.escalation_reserve_attempts, Some(2));
    }

    #[test]
    fn parses_from_toml_section() {
        let budget: IssueBudget =
            toml::from_str("max_attempts = 2\nmax_elapsed_minutes = 90\nmax_manager_rounds = 3\n")
                .unwrap();
        assert_eq!(budget.max_attempts, Some(2));
        assert_eq!(budget.max_elapsed_minutes, Some(90));
        assert_eq!(budget.max_manager_rounds, Some(3));
        assert_eq!(budget.escalation_reserve_attempts(), 1);
    }
}
