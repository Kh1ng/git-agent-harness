use super::*;

#[test]
fn setup_failures_below_limit_or_with_different_signatures_retry() {
    let profile: crate::config::Profile = serde_json::from_value(serde_json::json!({
        "display_name": "Real",
        "repo_id": "real",
        "provider": "github",
        "repo": "owner/repo",
        "local_path": "/tmp/repo",
        "artifact_root": "/tmp/artifacts",
        "default_target_branch": "main"
    }))
    .unwrap();
    for summaries in [
        vec!["setup failed (/tmp/a): 9 GiB"; 2],
        vec![
            "disk space exhausted",
            "worktree already exists",
            "setup timeout",
        ],
    ] {
        let entries: Vec<_> = summaries
            .iter()
            .map(|summary| {
                let mut entry = crate::ledger::LedgerEntry::new(
                    "real", &profile, "auto", "fix", "#1462", None, None,
                );
                entry.work_id = Some("#1462".into());
                entry.failure_class = Some("harness_error".into());
                entry.failure_stage = Some("dispatch".into());
                entry.error_summary = Some((*summary).into());
                entry
            })
            .collect();
        let reason = AvailableTicket::setup_failure_reason_for_ticket(
            "#1462",
            &profile,
            &crate::ledger::index_entries_by_work_id(&entries),
        );
        assert!(reason.is_none(), "{summaries:?}");
        let mut snapshot = empty_snapshot();
        let mut candidate = ticket(
            "setup",
            Some("#1462"),
            entries.len(),
            Some("harness_error"),
            false,
            false,
        );
        candidate.human_required = reason.is_some();
        snapshot.available_tickets.push(candidate);
        snapshot.availability.push(ScopeStatusJson {
            backend_instance: None,
            backend: "codex".into(),
            model: None,
            quota_pool: None,
            eligible_now: true,
            reason: None,
            unavailable_until: None,
            source: None,
            last_error_summary: None,
            observed_at: None,
            scope: None,
        });
        assert!(matches!(
            decide_next_action(&snapshot),
            NextAction::Retry { .. }
        ));
    }
}

#[test]
fn repeated_setup_gate_prevents_dispatch_and_retry() {
    let mut snapshot = empty_snapshot();
    let mut candidate = ticket(
        "setup",
        Some("#1462"),
        3,
        Some("harness_error"),
        false,
        true,
    );
    candidate.human_required = true;
    candidate.human_required_reason_code = Some("repeated_setup_failure".into());
    snapshot.available_tickets.push(candidate);
    snapshot.availability.push(ScopeStatusJson {
        backend_instance: None,
        backend: "codex".into(),
        model: None,
        quota_pool: None,
        eligible_now: true,
        reason: None,
        unavailable_until: None,
        source: None,
        last_error_summary: None,
        observed_at: None,
        scope: None,
    });
    assert_eq!(decide_next_action(&snapshot).kind(), "no_op");
}
