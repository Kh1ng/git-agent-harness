//! `NotifyEvent::DispatchFailed` was defined, formatted, and tested but
//! never actually constructed anywhere in production (confirmed 2026-08-08
//! while dogfooding: only HumanRequired/MrCreated/ReviewVerdict/MrMerged/
//! BackendStalled/HandoffCreated ever fired) -- despite the module doc
//! comment at the top of `notifications.rs` explicitly listing "Dispatch
//! failed terminally" as one of the events it covers. This is the fix:
//! `jsonl::append` (the single chokepoint ~50 call sites across the crate
//! funnel every ledger write through) calls `notify_if_genuine_failure`
//! after every successful write, rather than scattering a notify call
//! across each of the ~10 individual failure-construction sites. Fires per
//! genuine failure (`agent_no_progress`/`agent_failure`/
//! `context_limit_exceeded`, the same `is_genuine_agent_failure` classifier
//! the controller already uses to decide retry/escalate), deliberately
//! per-attempt rather than waiting for full retry exhaustion, and
//! deliberately excludes routine backpressure (`harness_error`/
//! `backend_error`: quota exhaustion, node admission deferred, no eligible
//! backend) to avoid alert spam on expected conditions.
//!
//! Split out of `jsonl.rs` into its own file (2026-08-08) once that file
//! hit the untracked-file line-count ratchet in `tests/source_structure.rs`
//! -- a one-line call from `append` is a smaller footprint there than the
//! full notification-construction logic.

use super::entry::LedgerEntry;
use crate::config::GahConfig;

pub(super) fn notify_if_genuine_failure(cfg: &GahConfig, entry: &LedgerEntry) {
    let Some(failure_class) = entry.failure_class.as_deref() else {
        return;
    };
    if !crate::controller::is_genuine_agent_failure(failure_class) {
        return;
    }
    let Ok(profile) = crate::config::get_profile(cfg, &entry.profile) else {
        return;
    };
    let Some(work_id) = entry.work_id.as_deref() else {
        return;
    };
    crate::notifications::notify_event(
        cfg,
        profile,
        crate::notifications::NotifyEvent::DispatchFailed {
            timestamp: &entry.timestamp,
            profile: &entry.profile,
            failure_class,
            failure_stage: entry.failure_stage.as_deref(),
            run_id: entry.session_id.as_deref().unwrap_or("unknown"),
            work_id,
            attempt_count: entry.attempts_completed,
            error_summary: entry.error_summary.as_deref(),
            mr_url: entry.mr_url.as_deref(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::notify_if_genuine_failure;
    use crate::ledger::test_util as ledger_tests;

    #[test]
    fn fires_dispatch_failed_notification_only_for_genuine_failures() {
        let (tmp, mut cfg) = ledger_tests::test_config();
        let genuine_out = tmp.path().join("genuine-notify.txt");
        let mut profile = ledger_tests::profile();
        profile.notify_command = Some(format!("cat > {}", genuine_out.display()));
        cfg.profiles.insert("test".into(), profile);

        let mut entry = crate::ledger::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "vibe",
            "improve",
            "hello",
            Some("session-1".into()),
            None,
        );
        entry.work_id = Some("W1".into());
        entry.failure_class = Some("agent_no_progress".into());
        entry.error_summary =
            Some("backend exited 0 on attempt 3 but produced no worktree changes".into());
        notify_if_genuine_failure(&cfg, &entry);

        let notified = std::fs::read_to_string(&genuine_out).unwrap();
        assert!(notified.contains("dispatch terminal failure"));
        assert!(notified.contains("class=agent_no_progress"));
        assert!(notified.contains("work_id=W1"));

        // Backpressure (e.g. quota exhaustion, no eligible backend) is a
        // real ledger entry too, but must NOT trigger the same alert --
        // only is_genuine_agent_failure classes do.
        let backpressure_out = tmp.path().join("backpressure-notify.txt");
        let mut backpressure_profile = ledger_tests::profile();
        backpressure_profile.notify_command = Some(format!("cat > {}", backpressure_out.display()));
        cfg.profiles.insert("test".into(), backpressure_profile);

        let mut entry2 = crate::ledger::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "vibe",
            "improve",
            "hello",
            Some("session-2".into()),
            None,
        );
        entry2.work_id = Some("W2".into());
        entry2.failure_class = Some("harness_error".into());
        notify_if_genuine_failure(&cfg, &entry2);

        assert!(!backpressure_out.exists());
    }
}
