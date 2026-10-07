use super::*;

fn tmp_store() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota_observations.jsonl");
    (dir, path)
}

// Issue #761: maybe_refresh_backend is the throttle gate for the new
// periodic quota refresh -- these test it directly rather than through
// refresh_stale_quota_observations, which needs a real codex/vibe
// subprocess to actually observe anything. The refresh itself now runs
// on a detached thread (see IN_FLIGHT's doc comment for why), so these
// wait on the one externally-observable effect -- a marker record
// landing in the store -- instead of asserting immediately. Each test
// uses its own synthetic backend name so parallel test threads can't
// collide on the process-wide IN_FLIGHT set.
fn wait_for_backend_record(path: &Path, backend: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if load(path)
            .unwrap_or_default()
            .iter()
            .any(|record| record.backend == backend)
        {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn maybe_refresh_backend_calls_the_refresher_when_nothing_is_recorded_yet() {
    let (_dir, path) = tmp_store();
    let now = OffsetDateTime::now_utc();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_in_thread = calls.clone();

    maybe_refresh_backend(&path, "test-calls-when-empty", now, move || {
        calls_in_thread.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(None)
    });

    assert!(wait_for_backend_record(&path, "test-calls-when-empty"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn maybe_refresh_backend_skips_a_backend_checked_within_the_interval() {
    let (_dir, path) = tmp_store();
    let now = OffsetDateTime::now_utc();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let first = calls.clone();
    maybe_refresh_backend(&path, "test-skip-within-interval", now, move || {
        first.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(None)
    });
    assert!(wait_for_backend_record(&path, "test-skip-within-interval"));
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "first call must actually refresh"
    );

    // Ten minutes later, inside QUOTA_REFRESH_INTERVAL.
    let second = calls.clone();
    maybe_refresh_backend(
        &path,
        "test-skip-within-interval",
        now + time::Duration::minutes(10),
        move || {
            second.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        },
    );
    // Nothing async to wait for here -- a throttled call never spawns a
    // thread, so the skip is synchronous. A brief settle is still fair
    // in case it wrongly did spawn one.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "must not re-refresh before the interval elapses"
    );
}

#[test]
fn maybe_refresh_backend_refreshes_again_once_the_interval_elapses() {
    let (_dir, path) = tmp_store();
    let now = OffsetDateTime::now_utc();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let first = calls.clone();
    maybe_refresh_backend(&path, "test-refresh-again", now, move || {
        first.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(None)
    })
    .unwrap()
    .join()
    .unwrap();
    assert!(wait_for_backend_record(&path, "test-refresh-again"));

    let second = calls.clone();
    maybe_refresh_backend(
        &path,
        "test-refresh-again",
        now + QUOTA_REFRESH_INTERVAL + time::Duration::minutes(1),
        move || {
            second.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        },
    )
    .unwrap()
    .join()
    .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn every_fifteen_minute_tick_refreshes_before_routing_distrusts_the_reading() {
    // #1331: a 30-minute throttle against a 30-minute freshness window
    // skipped every other server tick, leaving routing blind half the hour.
    let tick = time::Duration::minutes(15);
    assert!(QUOTA_REFRESH_INTERVAL < tick);
    assert!(tick < QUOTA_FRESHNESS);
    let (_dir, path) = tmp_store();
    let start = OffsetDateTime::parse("2026-10-03T18:00:00Z", &Rfc3339).unwrap();
    let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "test-tick-cadence",
        None::<String>,
        None::<String>,
    );
    for n in 0..4 {
        let now = start + tick * n;
        if n > 0 {
            assert!(
                crate::routing::subscription::capacity(&identity, &load(&path).unwrap(), now)
                    .known_capacity,
                "capacity must stay known before tick {n} refreshes"
            );
        }
        let refresh_path = path.clone();
        maybe_refresh_backend(&path, "test-tick-cadence", now, move || {
            let reading: QuotaObservationRecord = serde_json::from_value(serde_json::json!({
                "backend": "test-tick-cadence", "quota_window": "weekly",
                "quota_remaining_percent": 50, "observed_at": now.format(&Rfc3339).unwrap(),
                "checked_at": now.format(&Rfc3339).unwrap(),
                "quota_reset_at": (start + time::Duration::days(7)).format(&Rfc3339).unwrap()
            }))
            .unwrap();
            append(&refresh_path, &reading)?;
            Ok(Some(reading))
        })
        .unwrap_or_else(|| panic!("tick {n} was throttled"))
        .join()
        .unwrap();
        assert!(
            crate::routing::subscription::capacity(&identity, &load(&path).unwrap(), now)
                .known_capacity,
            "capacity must be known after tick {n} refreshes"
        );
    }
}

#[test]
fn maybe_refresh_backend_throttles_on_a_failed_attempt_too() {
    // A backend that's simply not installed here (or a network hiccup)
    // must not be retried every single loop tick forever -- that's a
    // real "don't DDoS it" failure mode, not just the happy path.
    let (_dir, path) = tmp_store();
    let now = OffsetDateTime::now_utc();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let first = calls.clone();
    maybe_refresh_backend(&path, "test-throttle-on-failure", now, move || {
        first.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        anyhow::bail!("vibe not installed")
    });
    assert!(wait_for_backend_record(&path, "test-throttle-on-failure"));

    let second = calls.clone();
    maybe_refresh_backend(
        &path,
        "test-throttle-on-failure",
        now + time::Duration::minutes(5),
        move || {
            second.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("vibe not installed")
        },
    );
    std::thread::sleep(std::time::Duration::from_millis(50));

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a failed attempt must still throttle retries"
    );
}

#[test]
fn one_backend_failure_is_recorded_without_blocking_another_backend() {
    let (_dir, path) = tmp_store();
    let now = OffsetDateTime::now_utc();

    maybe_refresh_backend(&path, "test-independent-codex", now, || {
        anyhow::bail!("codex status failed with ghp_abcdefghijklmnopqrstuvwxyz")
    });
    maybe_refresh_backend(&path, "test-independent-vibe", now, || Ok(None));

    assert!(wait_for_backend_record(&path, "test-independent-codex"));
    assert!(wait_for_backend_record(&path, "test-independent-vibe"));
    let records = load(&path).unwrap();
    let codex = records
        .iter()
        .find(|record| record.backend == "test-independent-codex")
        .unwrap();
    let vibe = records
        .iter()
        .find(|record| record.backend == "test-independent-vibe")
        .unwrap();
    assert_eq!(
        codex.check_error.as_deref(),
        Some("codex status failed with [REDACTED:GITHUB_TOKEN]")
    );
    assert_eq!(vibe.check_error, None);
}

#[test]
fn refresh_vibe_admin_and_store_reports_auth_required_without_api_key() {
    let _key_guard = crate::test_support::MistralAdminKeyEnvGuard::unset();
    let (_dir, path) = tmp_store();

    assert_eq!(
        refresh_vibe_admin_and_store(None, &path)
            .unwrap_err()
            .to_string(),
        "auth_required: MISTRAL_ADMIN_API_KEY is not configured"
    );
    assert!(load(&path).unwrap().is_empty());
}

#[test]
fn refresh_vibe_admin_and_store_persists_spend_limit_observation() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let _key_guard = crate::test_support::MistralAdminKeyEnvGuard::set("sk-test");
    let (_dir, path) = tmp_store();
    let bin_dir = tempfile::tempdir().unwrap();
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mistral-admin");
    let rate_limit_path = bin_dir.path().join("rate_limit.json");
    let spend_limit_path = bin_dir.path().join("spend_limit.json");
    std::fs::write(
        &rate_limit_path,
        r#"{
  "requests_per_second": 5,
  "tokens_limits_by_model": {
    "mistral-vibe-cli-latest": {
      "tokens_per_minute": 500000,
      "tokens_per_month": 200000000
    },
    "mistral-medium-3.5": {
      "tokens_per_minute": 250000,
      "tokens_per_month": 100000000
    }
  }
}"#,
    )
    .unwrap();
    std::fs::write(
        &spend_limit_path,
        r#"{
  "limits": {
    "completion": {
      "no_monthly_limit": false,
      "monthly_limit_reached": false,
      "usage": 128.42,
      "vibe_usage": 41.1,
      "total_usage": 169.52,
      "usage_limit": 500.0,
      "usage_limit_organization": 500.0
    },
    "last_payment_failure": false,
    "last_payment_failure_protection": null,
    "currency": "USD"
  }
}"#,
    )
    .unwrap();
    let script_path = bin_dir.path().join("curl");
    std::fs::write(
            &script_path,
            format!(
                "#!/bin/sh\ncfg=$(cat)\ncase \"$cfg\" in\n  *analytics/vibe/code/usage/by_workspace*) cat '{fixtures}/vibe_workspace_usage.json' ;;\n  *v1/admin/usage*) cat '{fixtures}/usage.json' ;;\n  *v1/admin/rate-limit*) cat '{rate_limit}' ;;\n  *v1/admin/spend-limit*) cat '{spend_limit}' ;;\n  *) exit 1 ;;\nesac\n",
                rate_limit = rate_limit_path.display(),
                spend_limit = spend_limit_path.display(),
            ),
        )
        .unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    let _path_guard = crate::test_support::PathGuard::set(bin_dir.path());

    let rec = refresh_vibe_admin_and_store(None, &path)
        .unwrap()
        .expect("spend limit observation persisted");
    assert_eq!(rec.backend, "vibe");
    assert_eq!(rec.quota_remaining_percent, Some(100.0 - 33.904));
    assert_eq!(
        rec.usage_source.as_deref(),
        Some("mistral_admin_spend_limit")
    );
    assert!(rec.mistral_admin.is_some());
    let admin = rec.mistral_admin.as_ref().unwrap();
    assert!(admin.workspace_usage.is_some());
    assert!(admin.billing.is_some());
    assert!(admin.rate_limits.is_some());

    let records = load(&path).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].backend, "vibe");
    assert!(records[0].mistral_admin.is_some());
}

#[test]
fn refresh_vibe_admin_and_store_persists_admin_refresh_without_spend_limit() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let _key_guard = crate::test_support::MistralAdminKeyEnvGuard::set("sk-test");
    let (_dir, path) = tmp_store();
    let bin_dir = tempfile::tempdir().unwrap();
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mistral-admin");
    let rate_limit_path = bin_dir.path().join("rate_limit.json");
    let spend_limit_path = bin_dir.path().join("spend_limit.json");
    std::fs::write(
        &rate_limit_path,
        r#"{
  "requests_per_second": 5,
  "tokens_limits_by_model": {
    "mistral-vibe-cli-latest": {
      "tokens_per_minute": 500000,
      "tokens_per_month": 200000000
    },
    "mistral-medium-3.5": {
      "tokens_per_minute": 250000,
      "tokens_per_month": 100000000
    }
  }
}"#,
    )
    .unwrap();
    std::fs::write(
        &spend_limit_path,
        r#"{
  "limits": {
    "completion": {
      "no_monthly_limit": false,
      "monthly_limit_reached": false
    },
    "currency": "USD",
    "last_payment_failure": false,
    "last_payment_failure_protection": null
  }
}"#,
    )
    .unwrap();
    let script_path = bin_dir.path().join("curl");
    std::fs::write(
            &script_path,
            format!(
                "#!/bin/sh\ncfg=$(cat)\ncase \"$cfg\" in\n  *analytics/vibe/code/usage/by_workspace*) cat '{fixtures}/vibe_workspace_usage.json' ;;\n  *v1/admin/usage*) cat '{fixtures}/usage.json' ;;\n  *v1/admin/rate-limit*) cat '{rate_limit}' ;;\n  *v1/admin/spend-limit*) cat '{spend_limit}' ;;\n  *) exit 1 ;;\nesac\n",
                rate_limit = rate_limit_path.display(),
                spend_limit = spend_limit_path.display(),
            ),
        )
        .unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    let _path_guard = crate::test_support::PathGuard::set(bin_dir.path());

    let rec = refresh_vibe_admin_and_store(None, &path)
        .unwrap()
        .expect("admin refresh observation persisted");

    let records = load(&path).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(rec.backend, "vibe");
    assert!(rec.quota_remaining_percent.is_none());
    assert_eq!(rec.usage_source.as_deref(), Some("mistral_admin_refresh"));
    let admin = rec.mistral_admin.as_ref().expect("admin payload persisted");
    assert!(admin.workspace_usage.is_some());
    assert!(admin.billing.is_some());
    assert!(admin.rate_limits.is_some());
    assert_eq!(records[0].backend, "vibe");
    assert!(records[0].mistral_admin.is_some());
}

#[test]
fn load_missing_file_is_empty() {
    let (_dir, path) = tmp_store();
    assert!(!path.exists());
    assert!(load(&path).unwrap().is_empty());
}

#[test]
fn append_then_load_round_trips() {
    let (_dir, path) = tmp_store();
    append(
        &path,
        &QuotaObservationRecord {
            backend: "codex".into(),
            backend_instance: None,
            model: Some("gpt-5".into()),
            quota_pool: None,
            quota_window: Some("300m".into()),
            quota_remaining_percent: Some(75.0),
            quota_reset_at: Some("2026-04-29T12:00:00Z".into()),
            observed_at: Some("2026-04-28T10:00:00Z".into()),
            checked_at: None,
            check_error: None,
            usage_source: Some("codex_status_json".into()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        },
    )
    .unwrap();

    let records = load(&path).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].backend, "codex");
    assert_eq!(records[0].quota_remaining_percent, Some(100.0 - 25.0));
    assert_eq!(records[0].quota_remaining_percent, Some(75.0));
}

// Issue #206: a single malformed JSONL line must be skipped, not cause the
// whole store (every valid record before/after it) to be discarded.
#[test]
fn load_skips_malformed_line_and_keeps_valid_records() {
    let (_dir, path) = tmp_store();
    let good1 = QuotaObservationRecord {
        backend: "codex".into(),
        backend_instance: None,
        model: None,
        quota_pool: None,
        quota_window: Some("weekly".into()),
        quota_remaining_percent: Some(90.0),
        quota_reset_at: None,
        observed_at: Some("2026-04-28T10:00:00Z".into()),
        checked_at: None,
        check_error: None,
        usage_source: Some("codex_status_json".into()),
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
    };
    let good2 = QuotaObservationRecord {
        quota_remaining_percent: Some(100.0 - 20.0),
        observed_at: Some("2026-04-29T10:00:00Z".into()),
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
        ..good1.clone()
    };
    let mut contents = serde_json::to_string(&good1).unwrap();
    contents.push('\n');
    contents.push_str("{ this is not valid json ]\n");
    contents.push_str(&serde_json::to_string(&good2).unwrap());
    contents.push('\n');
    std::fs::write(&path, contents).unwrap();

    let records = load(&path).unwrap();
    assert_eq!(
        records.len(),
        2,
        "the bad line should be skipped, not fatal"
    );
    assert_eq!(records[0].quota_remaining_percent, Some(100.0 - 10.0));
    assert_eq!(records[1].quota_remaining_percent, Some(100.0 - 20.0));
}

fn scoped_record(
    instance: Option<&str>,
    percent: f64,
    observed_at: &str,
) -> QuotaObservationRecord {
    QuotaObservationRecord {
        backend: "opencode".into(),
        backend_instance: instance.map(str::to_string),
        model: Some("shared-model".into()),
        quota_pool: Some("shared-pool".into()),
        quota_window: Some("daily".into()),
        quota_remaining_percent: Some(100.0 - percent),
        quota_reset_at: None,
        observed_at: Some(observed_at.into()),
        checked_at: None,
        check_error: None,
        usage_source: Some("test".into()),
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
    }
}

fn identity(instance: &str) -> crate::execution_identity::ExecutionIdentity {
    let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
        "opencode",
        Some("shared-model"),
        Some("shared-pool"),
    );
    identity.backend_instance = instance.into();
    identity
}

#[test]
fn latest_windows_keep_short_and_weekly_limits_on_the_right_account() {
    let mut weekly = scoped_record(Some("account-a"), 20.0, "2026-07-20T10:00:00Z");
    weekly.quota_window = Some("weekly".into());
    let mut short = scoped_record(Some("account-a"), 90.0, "2026-07-20T11:00:00Z");
    short.quota_window = Some("5h".into());
    let mut old = weekly.clone();
    old.observed_at = Some("2026-07-19T10:00:00Z".into());
    let mut sibling = weekly.clone();
    sibling.backend_instance = Some("account-b".into());
    sibling.quota_remaining_percent = Some(1.0);
    let records = [weekly, short, old, sibling];
    let windows = latest_windows_for_identity(&records, &identity("account-a"));
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].quota_remaining_percent, Some(100.0 - 90.0));
    assert_eq!(windows[1].quota_remaining_percent, Some(100.0 - 20.0));
}

/// #1384 review: a newer account-wide reading must not hide an exhausted
/// model-scoped reading of the same window from routing.
#[test]
fn latest_windows_keep_model_scoped_reading_beside_account_reading() {
    let mut account = scoped_record(Some("account-a"), 50.0, "2026-07-20T11:00:00Z");
    account.model = None;
    let exhausted = scoped_record(Some("account-a"), 100.0, "2026-07-20T10:00:00Z");
    let records = [account, exhausted];
    let windows = latest_windows_for_identity(&records, &identity("account-a"));
    assert_eq!(windows.len(), 2);
    assert!(windows
        .iter()
        .any(|record| record.model.is_some() && record.quota_remaining_percent == Some(0.0)));
}

#[test]
fn newer_failed_or_empty_check_invalidates_only_its_account() {
    let success = scoped_record(Some("account-a"), 20.0, "2026-07-20T10:00:00Z");
    let sibling = scoped_record(Some("account-b"), 70.0, "2026-07-20T10:00:00Z");
    let mut failure = success.clone();
    failure.quota_window = None;
    failure.quota_remaining_percent = None;
    failure.observed_at = None;
    failure.checked_at = Some("2026-07-20T11:00:00Z".into());
    for error in [None, Some("failed".into())] {
        failure.check_error = error;
        let records = [success.clone(), sibling.clone(), failure.clone()];
        assert!(latest_for_identity(&records, &identity("account-a")).is_none());
        assert_eq!(
            latest_for_identity(&records, &identity("account-b"))
                .unwrap()
                .quota_remaining_percent,
            Some(30.0)
        );
    }
}

#[test]
fn account_usage_import_is_scoped_and_does_not_require_a_quota_cap() {
    let value = serde_json::json!({"backend":"mistral-dashboard", "backend_instance":"mistral-dashboard:account", "quota_pool":"mistral-dashboard:account", "checked_at":"2026-10-03T00:00:00Z", "observed_at":"2026-10-03T00:00:00Z", "usage_source":"mistral_dashboard", "account_usage":{"account_id":"customer-1", "workspace_id":null, "period_start":"2026-10-01T00:00:00Z", "period_end":"2026-10-03T00:00:00Z", "currency":"USD", "requests":4, "cost":0.03079, "cost_source":"dashboard_prices", "models":[]}});
    let record = parse_external_observation(&value.to_string()).unwrap();
    assert!(record.quota_remaining_percent.is_none());
    assert!(record.quota_reset_at.is_none());
}

#[test]
fn external_observation_rejects_invalid_or_unscoped_provider_data() {
    let valid = serde_json::json!({"backend":"agy", "backend_instance":"agy-primary", "quota_pool":"agy:external", "quota_window":"weekly", "quota_remaining_percent":42.0, "checked_at":"2026-10-02T23:00:00Z", "observed_at":"2026-10-02T23:00:00Z", "usage_source":"cli_router"});
    assert_eq!(
        parse_external_observation(&valid.to_string())
            .unwrap()
            .quota_remaining_percent,
        Some(42.0)
    );
    for (field, value) in [
        ("backend_instance", serde_json::Value::Null),
        ("checked_at", serde_json::json!("bad time")),
        ("quota_remaining_percent", serde_json::json!(101)),
        ("access_token", serde_json::json!("private")),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(
            parse_external_observation(&invalid.to_string()).is_err(),
            "accepted invalid {field}"
        );
    }
}

#[test]
fn latest_identity_observation_never_crosses_explicit_instances() {
    let records = vec![
        scoped_record(Some("account-a"), 10.0, "2026-07-20T10:00:00Z"),
        scoped_record(Some("account-b"), 70.0, "2026-07-20T11:00:00Z"),
    ];

    let first = latest_for_identity(&records, &identity("account-a")).unwrap();
    let second = latest_for_identity(&records, &identity("account-b")).unwrap();

    assert_eq!(first.quota_remaining_percent, Some(100.0 - 10.0));
    assert_eq!(second.quota_remaining_percent, Some(100.0 - 70.0));
    assert!(
        latest_for_identity(&records, &identity("account-c")).is_none(),
        "legacy aggregation must not erase explicit instance identity"
    );
}

/// #1339 acceptance: a backend-scoped view surfaces every source
/// identity's latest windows — instance-scoped rows and router rows
/// alike — instead of only legacy unscoped rows.
#[test]
fn latest_windows_for_backend_returns_every_identity() {
    let claude_five_hour = QuotaObservationRecord {
        backend: "claude".into(),
        backend_instance: Some("claude".into()),
        model: None,
        quota_pool: None,
        quota_window: Some("5h".into()),
        quota_remaining_percent: Some(40.0),
        quota_reset_at: None,
        observed_at: Some("2026-10-04T08:00:00Z".into()),
        checked_at: Some("2026-10-04T08:00:00Z".into()),
        check_error: None,
        usage_source: Some("claude_native".into()),
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
    };
    let mut claude_weekly = claude_five_hour.clone();
    claude_weekly.quota_window = Some("weekly".into());
    claude_weekly.quota_remaining_percent = Some(90.0);
    let router = QuotaObservationRecord {
        backend: "agy".into(),
        backend_instance: Some("agy-primary".into()),
        model: None,
        quota_pool: Some("agy:external".into()),
        quota_window: Some("weekly".into()),
        quota_remaining_percent: Some(63.0),
        quota_reset_at: Some("in 16m44s".into()),
        observed_at: Some("2026-10-04T08:30:00Z".into()),
        checked_at: Some("2026-10-04T08:30:00Z".into()),
        check_error: None,
        usage_source: Some("cli_router".into()),
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
    };
    let records = vec![claude_five_hour, claude_weekly, router];

    let claude = latest_windows_for_backend(&records, "claude");
    assert_eq!(claude.len(), 2, "both Claude windows, newest per window");
    assert!(claude
        .iter()
        .all(|r| r.backend_instance.as_deref() == Some("claude")));

    let agy = latest_windows_for_backend(&records, "agy");
    assert_eq!(agy.len(), 1);
    assert_eq!(agy[0].usage_source.as_deref(), Some("cli_router"));

    assert!(latest_windows_for_backend(&records, "codex").is_empty());
}

/// #1339: a newer failed check invalidates only its own source's
/// windows in the backend-scoped view, never a sibling account's.
#[test]
fn latest_windows_for_backend_failed_check_scopes_to_its_source() {
    let good = QuotaObservationRecord {
        backend: "claude".into(),
        backend_instance: Some("claude".into()),
        model: None,
        quota_pool: None,
        quota_window: Some("5h".into()),
        quota_remaining_percent: Some(40.0),
        quota_reset_at: None,
        observed_at: Some("2026-10-04T08:00:00Z".into()),
        checked_at: Some("2026-10-04T08:00:00Z".into()),
        check_error: None,
        usage_source: Some("claude_native".into()),
        mistral_admin: None,
        account_usage: None,
        credential_id: None,
    };
    let mut failed_check = good.clone();
    failed_check.backend_instance = Some("claude-work".into());
    failed_check.quota_window = None;
    failed_check.quota_remaining_percent = None;
    failed_check.checked_at = Some("2026-10-04T09:00:00Z".into());
    failed_check.check_error = Some("auth_required: login expired".into());
    let records = vec![good, failed_check];

    let windows = latest_windows_for_backend(&records, "claude");
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].backend_instance.as_deref(), Some("claude"));
    assert_eq!(windows[0].quota_remaining_percent, Some(40.0));
}

#[test]
fn identity_observation_reads_legacy_unknown_without_assigning_it() {
    let records = vec![scoped_record(None, 40.0, "2026-07-20T10:00:00Z")];

    let observed = latest_for_identity(&records, &identity("account-a")).unwrap();

    assert_eq!(observed.backend_instance, None);
    assert_eq!(observed.quota_remaining_percent, Some(100.0 - 40.0));
}

#[test]
fn account_level_observation_applies_to_models_on_the_same_instance() {
    let mut account = scoped_record(Some("account-a"), 55.0, "2026-07-20T10:00:00Z");
    account.model = None;
    let records = [account];

    let observed = latest_for_identity(&records, &identity("account-a")).unwrap();

    assert_eq!(observed.model, None);
    assert_eq!(observed.quota_remaining_percent, Some(100.0 - 55.0));
}

#[test]
fn loading_legacy_jsonl_is_idempotent_and_keeps_instance_unknown() {
    let (_dir, path) = tmp_store();
    let legacy =
        r#"{"backend":"codex","model":null,"quota_window":"weekly","quota_used_percent":30.0}"#;
    std::fs::write(&path, format!("{legacy}\n")).unwrap();
    let original = std::fs::read(&path).unwrap();

    for _ in 0..2 {
        let records = load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].backend_instance, None);
        assert_eq!(records[0].quota_pool, None);
    }

    assert_eq!(std::fs::read(&path).unwrap(), original);
}
