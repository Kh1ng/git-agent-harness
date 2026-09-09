use super::*;

fn candidate(instance: &str, reason: &str, reset: Option<&str>) -> RoutingCandidateDiagnostic {
    RoutingCandidateDiagnostic {
        backend: "opencode".into(),
        backend_instance: Some(instance.into()),
        model: Some("provider/model".into()),
        skip_reason: Some(reason.into()),
        unavailable_until: reset.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn notices_include_decision_context_and_are_durable_per_exact_candidate() {
    let (tmp, mut cfg) = crate::ledger::test_util::test_config();
    let output = tmp.path().join("notices.txt");
    let mut profile = crate::ledger::test_util::profile();
    profile.notify_command = Some(format!("cat >> '{}'", output.display()));
    cfg.profiles.insert("test".into(), profile.clone());
    let mut entry = LedgerEntry::new("test", &profile, "auto", "fix", "#762", None, None);
    entry.work_id = Some("#762".into());
    let mut diagnostics = RoutingDiagnostics {
        candidates: vec![
            candidate("paid-a", "operator_approval_required", None),
            candidate(
                "included-slow",
                "quota_exhausted",
                Some("2100-01-02T00:00:00Z"),
            ),
            candidate(
                "included-fast",
                "quota_exhausted",
                Some("2100-01-01T00:00:00Z"),
            ),
            candidate(
                "broken-auth",
                "authentication_error",
                Some("2099-01-01T00:00:00Z"),
            ),
            candidate("bad-reset", "quota_exhausted", Some("invalid")),
            candidate(
                "stale-reset",
                "quota_exhausted",
                Some("2000-01-01T00:00:00Z"),
            ),
        ],
        ..Default::default()
    };
    notify_paid_route_skips(&cfg, &profile, &entry, Some(&diagnostics));
    // No in-memory cache: a fresh call reads the durable journal and suppresses
    // a repeated tick, even when the work ID uses the legacy ticket alias.
    entry.work_id = Some("TICKET-762".into());
    notify_paid_route_skips(&cfg, &profile, &entry, Some(&diagnostics));
    let messages = std::fs::read_to_string(&output).unwrap();
    assert_eq!(messages.lines().count(), 1);
    assert!(messages.contains("work_id=#762"));
    assert!(messages.contains("route=opencode/provider/model"));
    assert!(messages.contains("instance=paid-a"));
    assert!(messages.contains("policy requires operator approval"));
    assert!(messages.contains("2100-01-01T00:00:00Z"));
    assert!(messages.contains("alternative instance=included-fast"));
    assert!(!messages.contains("2100-01-02"));
    assert!(!messages.contains("2099-01-01"));
    assert!(crate::ledger::read_entries(&cfg).unwrap().is_empty());

    diagnostics
        .candidates
        .push(candidate("paid-b", "operator_approval_required", None));
    notify_paid_route_skips(&cfg, &profile, &entry, Some(&diagnostics));
    assert_eq!(std::fs::read_to_string(&output).unwrap().lines().count(), 2);

    let mut grant = LedgerEntry::new_paid_route_approval(
        "test",
        &profile,
        "#762",
        "opencode",
        Some("provider/model"),
        true,
    );
    grant.usage.backend_instance = Some("paid-a".into());
    crate::ledger::append(&cfg, &grant).unwrap();
    grant.mode = "paid_route_approval_revoke".into();
    crate::ledger::append(&cfg, &grant).unwrap();
    notify_paid_route_skips(&cfg, &profile, &entry, Some(&diagnostics));
    let messages = std::fs::read_to_string(&output).unwrap();
    assert_eq!(messages.lines().count(), 3);
    assert!(messages.lines().last().unwrap().contains("instance=paid-a"));
    // Notices do not invent work-item gates or change approval state.
    let entries = crate::ledger::read_entries(&cfg).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(crate::ledger::effective_human_gate_from_entries(
        &entries,
        "test",
        &profile.repo_id,
        "#762"
    )
    .is_none());
    assert!(
        crate::ledger::active_paid_route_approval_destinations_from_entries(
            &entries, "test", "#762"
        )
        .is_empty()
    );
}

#[test]
fn routine_skips_and_missing_work_id_never_notify() {
    let (tmp, cfg) = crate::ledger::test_util::test_config();
    let output = tmp.path().join("notices.txt");
    let mut profile = crate::ledger::test_util::profile();
    profile.notify_command = Some(format!("cat >> '{}'", output.display()));
    let mut entry = LedgerEntry::new("test", &profile, "auto", "fix", "#762", None, None);
    entry.work_id = Some("#762".into());
    let mut diagnostics = RoutingDiagnostics {
        candidates: vec![
            candidate("quota", "quota_exhausted", None),
            candidate("auth", "authentication_error", None),
        ],
        ..Default::default()
    };
    notify_paid_route_skips(&cfg, &profile, &entry, Some(&diagnostics));
    diagnostics
        .candidates
        .push(candidate("paid", "operator_approval_required", None));
    entry.work_id = None;
    notify_paid_route_skips(&cfg, &profile, &entry, Some(&diagnostics));
    assert!(!output.exists());
}

#[test]
fn concurrent_dispatches_claim_one_notice_and_corruption_fails_closed() {
    let (_tmp, cfg) = crate::ledger::test_util::test_config();
    let profile = crate::ledger::test_util::profile();
    let entry = LedgerEntry::new("test", &profile, "auto", "fix", "#762", None, None);
    let paid = candidate("paid", "operator_approval_required", None);
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| claim_notice(&cfg, &entry, "#762", &paid).unwrap()))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|claimed| *claimed)
            .count()
    });
    assert_eq!(results, 1);
    let mut other_scope = entry.clone();
    other_scope.repo_id = "another-repo".into();
    assert!(claim_notice(&cfg, &other_scope, "#762", &paid).unwrap());
    assert!(claim_notice(&cfg, &entry, "#763", &paid).unwrap());
    let mut clear = entry.clone();
    clear.work_id = Some("#762".into());
    clear.mode = "clear_attempts".into();
    crate::ledger::append(&cfg, &clear).unwrap();
    assert!(claim_notice(&cfg, &entry, "#762", &paid).unwrap());
    assert!(!claim_notice(&cfg, &entry, "#762", &paid).unwrap());
    let path = cfg
        .defaults
        .ledger_path()
        .with_extension("paid-route-notices.json");
    std::fs::write(path, "invalid").unwrap();
    assert!(claim_notice(&cfg, &entry, "#762", &paid).is_err());
}
