use super::LedgerEntry;
use serde::{Deserialize, Serialize};

/// Why execution usage could not be observed. Separate from model, token,
/// cost and quota reasons: those can be unknown even in a valid artifact.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum UsageUnknownReason {
    NoAttemptStarted,
    BackendNotInvoked,
    UsageArtifactMissing,
    UsageArtifactUnparsed,
}

/// Durable sinks share this boundary so early dispatch/control rows and
/// attempt records cannot be persisted without a reason for absent usage.
pub(super) fn annotate_unknown_usage(entry: &mut LedgerEntry) {
    for attempt in &mut entry.attempts {
        if attempt.usage.usage_source.is_none() && attempt.usage.usage_unknown_reason.is_none() {
            attempt.usage.usage_unknown_reason = Some(
                if attempt.failure_stage.as_deref() == Some("backend_launch") {
                    UsageUnknownReason::BackendNotInvoked
                } else {
                    UsageUnknownReason::UsageArtifactMissing
                },
            );
        }
    }
    if entry.usage.usage_source.is_none() && entry.usage.usage_unknown_reason.is_none() {
        entry.usage.usage_unknown_reason = entry
            .attempts
            .iter()
            .filter_map(|attempt| attempt.usage.usage_unknown_reason)
            .max()
            .or(Some(
                if entry.attempts_started == Some(0) && entry.attempts.is_empty() {
                    UsageUnknownReason::NoAttemptStarted
                } else {
                    UsageUnknownReason::UsageArtifactMissing
                },
            ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_records_no_attempt_started() {
        let entry = LedgerEntry::new(
            "test",
            &crate::ledger::test_util::profile(),
            "vibe",
            "fix",
            "task",
            None,
            None,
        );
        let normalized = entry.normalized_for_persistence();
        assert_eq!(normalized.usage.usage_source, None);
        assert_eq!(
            normalized.usage.usage_unknown_reason,
            Some(UsageUnknownReason::NoAttemptStarted)
        );
        assert_eq!(normalized.usage.total_tokens, None);
    }
}
