use super::*;

#[test]
fn record_route_attempt_preserves_each_route_and_its_diagnostics() {
    let tmp = tempfile::tempdir().unwrap();
    let _quota_guard = crate::test_support::QuotaStoreEnvGuard::set(tmp.path().join("quota.jsonl"));

    let now = OffsetDateTime::now_utc();
    let reading = serde_json::from_value(serde_json::json!({
        "backend": "agy", "backend_instance": "agy", "model": "gemini",
        "quota_window": "weekly", "quota_remaining_percent": 80,
        "observed_at": now.format(&Rfc3339).unwrap(),
        "quota_reset_at": (now + time::Duration::hours(1)).format(&Rfc3339).unwrap()
    }))
    .unwrap();
    crate::quota_store::append(&tmp.path().join("quota.jsonl"), &reading).unwrap();

    let mut entry = LedgerEntry::new(
        "test",
        &profile(tmp.path()),
        "codex",
        "improve",
        "target",
        Some("session-1".into()),
        None,
    );
    let first = RouteDecision::from_identity(
        crate::execution_identity::ExecutionIdentity::legacy_route(
            "auto",
            None::<String>,
            "agy",
            Some("gemini"),
            None::<String>,
        ),
        "profile routing policy".into(),
        false,
        None,
        false,
        Some(crate::ledger::RoutingDiagnostics {
            selected_backend: Some("agy".into()),
            selected_model: Some("gemini".into()),
            human_summary: Some("selected agy/gemini".into()),
            ..Default::default()
        }),
    );
    let second = RouteDecision::from_identity(
        crate::execution_identity::ExecutionIdentity::legacy_route(
            "auto",
            None::<String>,
            "codex",
            Some("gpt-5.4-mini"),
            None::<String>,
        ),
        "profile routing policy".into(),
        true,
        None,
        false,
        Some(crate::ledger::RoutingDiagnostics {
            selected_backend: Some("codex".into()),
            selected_model: Some("gpt-5.4-mini".into()),
            human_summary: Some("agy skipped: quota_exhausted".into()),
            ..Default::default()
        }),
    );

    record_route_attempt(&mut entry, &first).unwrap();
    record_route_attempt(&mut entry, &second).unwrap();

    assert!(entry
        .routing_runtime
        .dispatch_attempted
        .contains(&CandidateIdentity::new("agy", Some("gemini"))));
    assert_eq!(entry.attempt_routing.len(), 2);
    let capacity = entry.attempt_routing[0]
        .routing_diagnostics
        .as_ref()
        .unwrap()
        .selected_subscription_capacity
        .as_ref()
        .unwrap();
    assert!(capacity.known_capacity);
    assert!(!capacity.exhausted);
    assert!(capacity.reset_pressure.unwrap() > 100.0);
    assert_eq!(entry.attempt_routing[0].backend_instance, "agy");
    assert_eq!(
        entry.attempt_routing[0]
            .identity
            .as_ref()
            .map(|identity| identity.runner_kind.as_str()),
        Some("agy")
    );
    assert_eq!(
        entry.attempt_routing[1]
            .identity
            .as_ref()
            .map(|identity| identity.logical_backend.as_str()),
        Some("codex")
    );
    assert_eq!(
        entry.attempt_routing[1]
            .routing_diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.human_summary.as_deref()),
        Some("agy skipped: quota_exhausted")
    );

    let serialized = serde_json::to_string(&entry).unwrap();
    let parsed: LedgerEntry = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed.attempt_routing, entry.attempt_routing);
    assert!(parsed.routing_runtime.dispatch_attempted.is_empty());

    let mut unsafe_route = first.clone();
    unsafe_route.identity.auth_source_label = Some("/credential/home".into());
    let mut rejected = LedgerEntry::new(
        "test",
        &profile(tmp.path()),
        "auto",
        "improve",
        "target",
        None,
        None,
    );
    assert!(record_route_attempt(&mut rejected, &unsafe_route).is_err());
    assert!(rejected.attempt_routing.is_empty());
}
