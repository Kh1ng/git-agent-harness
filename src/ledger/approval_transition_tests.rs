use super::*;
use crate::ledger::{append_external_approval, read_entries};

fn transition(profile: &Profile, work: &str, mode: &str) -> LedgerEntry {
    let state = mode.strip_prefix("external_approval_").unwrap();
    LedgerEntry::new_external_approval(
        "test",
        profile,
        work,
        mode,
        ExternalApprovalRecord {
            state: Some(
                match state {
                    "request" => "requested",
                    "grant" => "approved",
                    "revoke" => "revoked",
                    _ => state,
                }
                .into(),
            ),
            credential_label: Some("odds".into()),
            operation_kind: Some("external_api".into()),
            allowed_env_vars: vec!["ODDS_API_KEY".into()],
            ..Default::default()
        },
    )
}

fn request(profile: &Profile, work: &str) -> LedgerEntry {
    let mut entry = transition(profile, work, "external_approval_request");
    let scope = entry.external_approval.as_mut().unwrap();
    scope.max_requests = Some(3);
    scope.max_dollars = Some(5.0);
    scope.expires_at = Some(
        (OffsetDateTime::now_utc() + time::Duration::hours(1))
            .format(&Rfc3339)
            .unwrap(),
    );
    scope.purpose = Some("Check the fixture service".into());
    entry
}

#[test]
fn grants_inherit_pending_bounds_and_cannot_expand_or_replay_them() {
    let (_tmp, cfg) = crate::ledger::test_util::test_config();
    let profile = crate::ledger::test_util::profile();
    // Work identifiers remain exact and provider-independent, including custom GitLab URLs.
    for work in ["#42", "https://gitlab.example.test/team/repo/-/issues/42"] {
        let grant = transition(&profile, work, "external_approval_grant");
        assert!(append_external_approval(&cfg, grant.clone()).is_err());
        let requested = request(&profile, work);
        append_external_approval(&cfg, requested.clone()).unwrap();
        let baseline = read_entries(&cfg).unwrap().len();
        for field in [
            "requests",
            "dollars",
            "expiry",
            "purpose",
            "credentials",
            "operation",
            "work",
            "repo",
            "profile",
        ] {
            let mut broader = grant.clone();
            let scope = broader.external_approval.as_mut().unwrap();
            match field {
                "requests" => scope.max_requests = Some(4),
                "dollars" => scope.max_dollars = Some(6.0),
                "expiry" => {
                    scope.expires_at = Some(
                        (OffsetDateTime::now_utc() + time::Duration::hours(2))
                            .format(&Rfc3339)
                            .unwrap(),
                    )
                }
                "purpose" => scope.purpose = Some("Unrelated operation".into()),
                "credentials" => scope.allowed_env_vars.push("OTHER_API_KEY".into()),
                "operation" => scope.operation_kind = Some("backfill".into()),
                "work" => broader.work_id = Some("#43".into()),
                "repo" => broader.repo_id = "other/repo".into(),
                "profile" => broader.profile = "other".into(),
                _ => unreachable!(),
            }
            assert!(
                append_external_approval(&cfg, broader).is_err(),
                "accepted {field}"
            );
            assert_eq!(read_entries(&cfg).unwrap().len(), baseline);
        }
        let (approved, _) = append_external_approval(&cfg, grant.clone()).unwrap();
        let approved = approved.external_approval.unwrap();
        let requested = requested.external_approval.unwrap();
        assert_eq!(approved.max_requests, requested.max_requests);
        assert_eq!(approved.max_dollars, requested.max_dollars);
        assert_eq!(approved.expires_at, requested.expires_at);
        assert_eq!(approved.purpose, requested.purpose);
        assert!(append_external_approval(&cfg, grant).is_err());
    }
    append_external_approval(&cfg, request(&profile, "#44")).unwrap();
    let mut narrowed = transition(&profile, "#44", "external_approval_grant");
    let scope = narrowed.external_approval.as_mut().unwrap();
    scope.max_requests = Some(1);
    scope.max_dollars = Some(1.0);
    scope.expires_at = Some(
        (OffsetDateTime::now_utc() + time::Duration::minutes(5))
            .format(&Rfc3339)
            .unwrap(),
    );
    let expected = scope.clone();
    let (granted, _) = append_external_approval(&cfg, narrowed).unwrap();
    let actual = granted.external_approval.unwrap();
    assert_eq!(actual.max_requests, expected.max_requests);
    assert_eq!(actual.max_dollars, expected.max_dollars);
    assert_eq!(actual.expires_at, expected.expires_at);
}

#[test]
fn invalid_or_expired_requests_fail_without_a_grant() {
    let (_tmp, cfg) = crate::ledger::test_util::test_config();
    let profile = crate::ledger::test_util::profile();
    for cap in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0] {
        let mut invalid = request(&profile, "#42");
        invalid.external_approval.as_mut().unwrap().max_dollars = Some(cap);
        assert!(append_external_approval(&cfg, invalid).is_err());
    }
    for field in ["requests", "expiry", "credentials", "label", "purpose"] {
        let mut invalid = request(&profile, "#42");
        let scope = invalid.external_approval.as_mut().unwrap();
        match field {
            "requests" => scope.max_requests = Some(0),
            "expiry" => scope.expires_at = Some("2020-01-01T00:00:00Z".into()),
            "credentials" => scope.allowed_env_vars.clear(),
            "label" => scope.credential_label = Some("odds\nforged message".into()),
            "purpose" => scope.purpose = Some("test\nforged message".into()),
            _ => unreachable!(),
        }
        assert!(append_external_approval(&cfg, invalid).is_err());
    }
    assert!(read_entries(&cfg).unwrap().is_empty());
    let mut expired = request(&profile, "#42");
    expired.external_approval.as_mut().unwrap().expires_at = Some("2020-01-01T00:00:00Z".into());
    // An old persisted request must also fail, without rewriting its history.
    crate::ledger::append(&cfg, &expired).unwrap();
    assert!(
        append_external_approval(&cfg, transition(&profile, "#42", "external_approval_grant"))
            .is_err()
    );
    assert_eq!(read_entries(&cfg).unwrap().len(), 1);
}

#[test]
fn only_one_concurrent_grant_can_consume_a_request() {
    let (_tmp, cfg) = crate::ledger::test_util::test_config();
    let profile = crate::ledger::test_util::profile();
    append_external_approval(&cfg, request(&profile, "#42")).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|threads| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                threads.spawn(|| {
                    barrier.wait();
                    append_external_approval(
                        &cfg,
                        transition(&profile, "#42", "external_approval_grant"),
                    )
                    .is_ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.into_iter().filter(|success| *success).count(), 1);
    assert_eq!(read_entries(&cfg).unwrap().len(), 2);
}

#[test]
fn consumption_and_sparse_revoke_preserve_granted_scope_and_unrelated_holds() {
    let (_tmp, cfg) = crate::ledger::test_util::test_config();
    let mut profile = crate::ledger::test_util::profile();
    profile.external_credential_scopes.insert(
        "odds".into(),
        crate::config::ExternalCredentialScope {
            env_vars: vec!["ODDS_API_KEY".into(), "NEW_API_KEY".into()],
        },
    );
    let mut requested = request(&profile, "#42");
    requested.external_approval.as_mut().unwrap().max_dollars = None;
    append_external_approval(&cfg, requested).unwrap();
    let mut gate = LedgerEntry::new("test", &profile, "auto", "fix", "#42", None, None);
    gate.work_id = Some("#42".into());
    gate.human_required = true;
    gate.human_required_reason_code = Some("operator_review".into());
    crate::ledger::append(&cfg, &gate).unwrap();
    append_external_approval(&cfg, transition(&profile, "#42", "external_approval_grant")).unwrap();
    record_external_approval_consumption_for_work_item(
        &cfg,
        "test",
        &profile,
        Some("#42"),
        &Default::default(),
    )
    .unwrap();
    let entries = read_entries(&cfg).unwrap();
    let snapshot = external_approval_snapshot_from_entries(
        &entries,
        "test",
        &profile.repo_id,
        "#42",
        "odds",
        "external_api",
    )
    .unwrap();
    assert!(snapshot.active);
    assert_eq!(snapshot.consumed_requests, 1);
    assert_eq!(snapshot.max_requests, Some(3));
    assert_eq!(snapshot.allowed_env_vars, ["ODDS_API_KEY"]);
    assert_eq!(
        active_external_approval_env_vars_from_entries(&entries, "test", &profile.repo_id, "#42"),
        HashSet::from(["ODDS_API_KEY".into()])
    );
    assert!(crate::ledger::effective_human_gate_from_entries(
        &entries,
        "test",
        &profile.repo_id,
        "#42"
    )
    .is_some());
    let mut revoke = transition(&profile, "#42", "external_approval_revoke");
    revoke
        .external_approval
        .as_mut()
        .unwrap()
        .allowed_env_vars
        .clear();
    append_external_approval(&cfg, revoke).unwrap();
    let entries = read_entries(&cfg).unwrap();
    let revoked = external_approval_snapshot_from_entries(
        &entries,
        "test",
        &profile.repo_id,
        "#42",
        "odds",
        "external_api",
    )
    .unwrap();
    assert!(!revoked.active);
    assert_eq!(revoked.state, "revoked");
    assert_eq!(revoked.max_requests, snapshot.max_requests);
    assert_eq!(revoked.allowed_env_vars, snapshot.allowed_env_vars);
    assert_eq!(revoked.purpose, snapshot.purpose);
    assert_eq!(revoked.expires_at, snapshot.expires_at);
    assert_eq!(revoked.consumed_requests, snapshot.consumed_requests);
    assert!(active_external_approval_env_vars_from_entries(
        &entries,
        "test",
        &profile.repo_id,
        "#42"
    )
    .is_empty());
}
