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
/// current attempt records carry a reason for absent usage. Historical rows
/// retain unknown telemetry rather than acquiring inferred facts during import.
pub(super) fn annotate_unknown_usage(entry: &mut LedgerEntry) {
    if entry.schema_version < super::LEDGER_SCHEMA_VERSION {
        return;
    }
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
            .or_else(|| {
                // Some workflows run the backend without writing attempt records.
                // Improve can also fail after completion but before recording usage.
                if entry.backend_exit_code.is_some()
                    || entry.attempts_completed.is_some_and(|count| count > 0)
                    || !entry.attempt_routing.is_empty()
                {
                    return Some(UsageUnknownReason::UsageArtifactMissing);
                }
                match (entry.attempts_started, entry.attempts.is_empty()) {
                    (None, true) => None,
                    (Some(0), true) => Some(UsageUnknownReason::NoAttemptStarted),
                    (Some(_), true) => Some(UsageUnknownReason::BackendNotInvoked),
                    (_, false) => Some(UsageUnknownReason::UsageArtifactMissing),
                }
            });
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

    #[test]
    fn persistence_records_backend_not_invoked_without_attempt_records() {
        for attempts_started in [Some(1), None] {
            let mut entry = LedgerEntry::new(
                "test",
                &crate::ledger::test_util::profile(),
                "vibe",
                "fix",
                "task",
                None,
                None,
            );
            entry.attempts_started = attempts_started;
            assert!(entry.attempts.is_empty());

            let normalized = entry.normalized_for_persistence();
            assert_eq!(
                normalized.usage.usage_unknown_reason,
                attempts_started.map(|_| UsageUnknownReason::BackendNotInvoked)
            );
            assert_eq!(normalized.usage.usage_source, None);
            assert_eq!(normalized.usage.total_tokens, None);
            assert_eq!(normalized.usage.requests_count, None);
        }
    }
    #[test]
    fn persistence_records_missing_usage_with_invocation_evidence_without_attempt_records() {
        for attempts_started in [Some(0), Some(1), None] {
            for evidence in ["exit_success", "exit_failure", "completed", "routing"] {
                let mut entry = LedgerEntry::new(
                    "test",
                    &crate::ledger::test_util::profile(),
                    "vibe",
                    "fix",
                    "task",
                    None,
                    None,
                );
                entry.attempts_started = attempts_started;
                match evidence {
                    "exit_success" => entry.backend_exit_code = Some(0),
                    "exit_failure" => entry.backend_exit_code = Some(1),
                    "completed" => entry.attempts_completed = Some(1),
                    "routing" => entry
                        .attempt_routing
                        .push(crate::ledger::AttemptRoutingRecord {
                            attempt_number: 1,
                            backend_instance: "vibe".into(),
                            effective_model: None,
                            identity: None,
                            routing_diagnostics: None,
                        }),
                    _ => unreachable!(),
                }
                assert!(entry.attempts.is_empty());
                let normalized = entry.normalized_for_persistence();
                assert_eq!(
                    normalized.usage.usage_unknown_reason,
                    Some(UsageUnknownReason::UsageArtifactMissing),
                    "{evidence}, attempts_started={attempts_started:?}",
                );
                assert_eq!(normalized.usage.usage_source, None);
                assert_eq!(normalized.usage.total_tokens, None);
                assert_eq!(normalized.usage.requests_count, None);
            }
        }
    }

    #[test]
    fn persistence_preserves_backend_launch_failure_reason_with_routing_evidence() {
        let mut entry = LedgerEntry::new(
            "test",
            &crate::ledger::test_util::profile(),
            "vibe",
            "fix",
            "task",
            None,
            None,
        );
        entry
            .attempt_routing
            .push(crate::ledger::AttemptRoutingRecord {
                attempt_number: 1,
                backend_instance: "vibe".into(),
                effective_model: None,
                identity: None,
                routing_diagnostics: None,
            });
        entry.attempts.push(crate::ledger::AttemptRecord {
            failure_stage: Some("backend_launch".into()),
            ..Default::default()
        });
        let normalized = entry.normalized_for_persistence();
        assert_eq!(
            normalized.usage.usage_unknown_reason,
            Some(UsageUnknownReason::BackendNotInvoked)
        );
        assert_eq!(
            normalized.attempts[0].usage.usage_unknown_reason,
            Some(UsageUnknownReason::BackendNotInvoked)
        );
    }

    #[test]
    fn persistence_preserves_legacy_unknown_usage() {
        let mut entry = LedgerEntry::new(
            "test",
            &crate::ledger::test_util::profile(),
            "vibe",
            "fix",
            "task",
            None,
            None,
        );
        entry.schema_version = super::super::LEDGER_SCHEMA_VERSION - 1;
        entry.attempts_started = None;
        entry.backend_exit_code = Some(0);
        entry.attempts_completed = Some(1);
        assert_eq!(
            entry
                .normalized_for_persistence()
                .usage
                .usage_unknown_reason,
            None
        );
        entry.attempts.push(crate::ledger::AttemptRecord::default());
        let normalized = entry.normalized_for_persistence();
        assert_eq!(normalized.usage.usage_unknown_reason, None);
        assert_eq!(normalized.attempts[0].usage.usage_unknown_reason, None);
    }
}
