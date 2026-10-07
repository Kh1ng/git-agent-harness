//! A source check is separate from billing identity. Keys are never hashed into
//! pools; only a provider-verified account supplies a shared pool identifier.
use super::{CredentialInfo, CredentialKind};
use crate::quota_store::{self, QuotaObservationRecord};
use anyhow::Result;
use std::path::Path;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(crate) fn backend(info: &CredentialInfo) -> &str {
    match (info.kind, info.provider.as_str(), info.env_var.as_deref()) {
        (CredentialKind::MistralDashboard | CredentialKind::MistralLogin, _, _) => {
            "mistral-dashboard"
        }
        (CredentialKind::ClaudeSubscriptionToken, _, _) => "claude",
        (CredentialKind::ApiKey, "nous", _) => "opencode",
        (CredentialKind::ApiKey, "mistral", Some("MISTRAL_ADMIN_API_KEY")) => "vibe",
        _ => &info.provider,
    }
}

fn unknown(info: &CredentialInfo, now: OffsetDateTime) -> QuotaObservationRecord {
    QuotaObservationRecord {
        backend: backend(info).into(),
        backend_instance: Some(format!("credential:{}", info.id)),
        credential_id: Some(info.id.clone()),
        model: None,
        quota_pool: None,
        quota_window: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        observed_at: None,
        checked_at: now.format(&Rfc3339).ok(),
        check_error: None,
        usage_source: Some(if info.kind == CredentialKind::ClaudeSubscriptionToken {
            "claude_oauth_usage".into()
        } else {
            format!("credential_api:{}", info.provider)
        }),
        mistral_admin: None,
        account_usage: None,
    }
}

pub(crate) fn refresh(id: &str, path: &Path) -> Result<QuotaObservationRecord> {
    let root = super::root()?;
    refresh_selected_at(&root, id, path, |selected| {
        let (info, secret) = (&selected.info, selected.secret.as_str());
        let one = |record: Result<QuotaObservationRecord>| record.map(|record| vec![record]);
        match (info.kind, info.provider.as_str(), info.env_var.as_deref()) {
            // One check returns the 5-hour and weekly windows together.
            (CredentialKind::ClaudeSubscriptionToken, _, _) => {
                crate::usage::claude::refresh_token(secret)
            }
            (CredentialKind::MistralLogin, _, _) => one(refresh_mistral_login(&root, selected)),
            (CredentialKind::MistralDashboard, _, _) => {
                one(crate::usage::mistral_dashboard::refresh_cookie(secret))
            }
            (CredentialKind::ApiKey, "nous", _) => one(crate::usage::nous::refresh_key(secret)),
            (CredentialKind::ApiKey, "mistral", Some("MISTRAL_ADMIN_API_KEY")) => {
                quota_store::refresh_vibe_admin_record(secret, None)
                    .map(|record| record.into_iter().collect())
            }
            _ => Ok(Vec::new()),
        }
    })
}

fn refresh_selected_at(
    root: &Path,
    id: &str,
    path: &Path,
    collect: impl FnOnce(&super::StoredCredential) -> Result<Vec<QuotaObservationRecord>>,
) -> Result<QuotaObservationRecord> {
    // Secret and generation come from one atomic record. Network calls hold no
    // source lock; rotation/deletion and publication share only a short lock.
    let selected = super::read_at(root, id)?;
    let result = collect(&selected);
    let _guard = super::source_lock(root, id)?;
    let current = super::read_at(root, id)
        .map_err(|_| anyhow::anyhow!("credential changed during quota check"))?;
    if current.revision != selected.revision {
        anyhow::bail!("credential changed during quota check");
    }
    publish(&selected.info, path, OffsetDateTime::now_utc(), result)
}

/// Reuse the cached dashboard session; sign in again only when Mistral rejects
/// it, so central does not log in every refresh. The new session is cached for
/// this exact revision, so a rotated password never inherits it.
fn refresh_mistral_login(
    root: &Path,
    selected: &super::StoredCredential,
) -> Result<QuotaObservationRecord> {
    use crate::usage::mistral_dashboard::{refresh_cookie, sign_in};
    refresh_mistral_login_with(root, selected, refresh_cookie, sign_in)
}

fn refresh_mistral_login_with(
    root: &Path,
    selected: &super::StoredCredential,
    refresh_cookie: impl Fn(&str) -> Result<QuotaObservationRecord>,
    sign_in: impl FnOnce(&str, &str) -> Result<String>,
) -> Result<QuotaObservationRecord> {
    let auth_required = |error: &anyhow::Error| error.to_string().starts_with("auth_required:");
    // A refused login is not retried every refresh tick: repeated password
    // attempts against the owner's account risk a lockout or sign-in alerts.
    if let Some(reason) = &selected.login_rejected {
        anyhow::bail!("{reason}; save the Mistral login again to retry");
    }
    if let Some(session) = &selected.session {
        match refresh_cookie(session) {
            Err(error) if auth_required(&error) => {}
            other => return other,
        }
    }
    let (id, revision) = (&selected.info.id, &selected.revision);
    let reject = |error: anyhow::Error| {
        super::reject_login_at(root, id, revision, &error.to_string())?;
        Err(error)
    };
    let login = super::mistral_login(&selected.secret)?;
    let session = match sign_in(&login.email, &login.password) {
        Err(error) if auth_required(&error) => return reject(error),
        other => other?,
    };
    super::cache_session_at(root, id, revision, &session)?;
    match refresh_cookie(&session) {
        Err(error) if auth_required(&error) => reject(anyhow::anyhow!("auth_required: Mistral accepted the saved password but rejected the new dashboard session; the account may require a second factor")),
        other => other,
    }
}

// Replacing a key can change its billing account. Clear that source before the
// replacement is acknowledged; other sources and ambient accounts stay active.
pub(crate) fn updated(info: &CredentialInfo, path: &Path) -> Result<()> {
    let mut record = unknown(info, OffsetDateTime::now_utc());
    record.usage_source = Some("credential_updated".into());
    quota_store::append(path, &record)
}

pub(crate) fn removed(info: &CredentialInfo, path: &Path) -> Result<()> {
    let mut record = unknown(info, OffsetDateTime::now_utc());
    record.usage_source = Some("credential_removed".into());
    quota_store::append(path, &record)
}

/// Store one check's outcome and return its first record. Every window of
/// the check is written together; no windows means the source reported no
/// quota, and an error becomes a failed observation for the source.
fn publish(
    info: &CredentialInfo,
    path: &Path,
    now: OffsetDateTime,
    result: Result<Vec<QuotaObservationRecord>>,
) -> Result<QuotaObservationRecord> {
    let outcomes = match result {
        Ok(records) if records.is_empty() => vec![Ok(None)],
        Ok(records) => records.into_iter().map(|record| Ok(Some(record))).collect(),
        Err(error) => vec![Err(error)],
    };
    let mut records = outcomes
        .into_iter()
        .map(|outcome| observation(info, path, now, outcome))
        .collect::<Result<Vec<_>>>()?;
    quota_store::append_all(path, &records)?;
    Ok(records.swap_remove(0))
}

fn observation(
    info: &CredentialInfo,
    path: &Path,
    now: OffsetDateTime,
    outcome: Result<Option<QuotaObservationRecord>>,
) -> Result<QuotaObservationRecord> {
    let mut record = match outcome {
        Ok(Some(mut record)) => {
            // Mistral dashboard returns a verified customer/workspace pool.
            // Legacy collectors with a fixed ambient pool cannot identify a
            // named account, so keep them source-scoped until verified.
            if !matches!(
                info.kind,
                CredentialKind::MistralDashboard | CredentialKind::MistralLogin
            ) && record
                .quota_pool
                .as_deref()
                .is_none_or(|pool| !pool.starts_with("nous:"))
            {
                record.backend_instance = Some(format!("credential:{}", info.id));
                record.quota_pool = None;
            }
            if record.backend_instance.is_none() {
                record.backend_instance = Some(format!("credential:{}", info.id));
            }
            record
        }
        Ok(None) => unknown(info, now),
        Err(error) => {
            let mut failed = quota_store::load(path)?
                .into_iter()
                .rev()
                .find(|record| {
                    record.credential_id.as_deref() == Some(&info.id)
                        && record.backend == backend(info)
                })
                .unwrap_or_else(|| unknown(info, now));
            failed.quota_window = None;
            failed.quota_remaining_percent = None;
            failed.quota_reset_at = None;
            failed.observed_at = None;
            failed.account_usage = None;
            failed.mistral_admin = None;
            failed.checked_at = now.format(&Rfc3339).ok();
            failed.check_error = Some(crate::redact::redact(&error.to_string()));
            failed
        }
    };
    record.credential_id = Some(info.id.clone());
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_identity::ExecutionIdentity;
    fn refresh_with(
        info: &CredentialInfo,
        path: &Path,
        now: OffsetDateTime,
        collect: impl FnOnce() -> Result<Option<QuotaObservationRecord>>,
    ) -> Result<QuotaObservationRecord> {
        let result = collect().map(|record| record.into_iter().collect());
        publish(info, path, now, result)
    }
    #[test]
    fn claude_subscription_windows_are_source_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let source = CredentialInfo {
            id: "claude-sub".into(),
            provider: "anthropic".into(),
            kind: CredentialKind::ClaudeSubscriptionToken,
            account_label: "Subscription".into(),
            env_var: None,
        };
        let records = crate::usage::claude::parse(
            br#"{"five_hour":{"utilization":20},"seven_day":{"utilization":40}}"#,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let first = records[0].quota_window.clone();
        let saved = publish(&source, &path, OffsetDateTime::now_utc(), Ok(records)).unwrap();
        assert_eq!(saved.quota_window, first);
        let stored = quota_store::load(&path).unwrap();
        assert_eq!(stored.len(), 2);
        let mut identity =
            ExecutionIdentity::legacy_candidate("claude", None::<String>, None::<String>);
        identity.credential_id = Some(source.id);
        let windows = quota_store::latest_windows_for_identity(&stored, &identity);
        assert_eq!(windows.len(), 2);
        assert!(windows.iter().all(|record| record.backend == "claude"
            && record.credential_id.as_deref() == Some("claude-sub")
            && record.quota_pool.is_none()));
    }
    fn info(id: &str) -> CredentialInfo {
        CredentialInfo {
            id: id.into(),
            provider: "mistral".into(),
            kind: CredentialKind::MistralDashboard,
            account_label: id.into(),
            env_var: None,
        }
    }
    fn reading(info: &CredentialInfo, now: OffsetDateTime) -> QuotaObservationRecord {
        let mut record = unknown(info, now);
        record.backend_instance = Some("mistral-dashboard:verified-customer".into());
        record.quota_pool = Some("mistral-dashboard:verified-customer".into());
        record.quota_window = Some("vibe-code-included-monthly".into());
        record.quota_remaining_percent = Some(40.0);
        record.observed_at = record.checked_at.clone();
        record
    }
    #[test]
    #[cfg(unix)]
    fn replacing_saved_source_invalidates_cached_quota_without_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = dir.path().join("credentials");
        let path = dir.path().join("quota.jsonl");
        let source = info("primary");
        let now = OffsetDateTime::now_utc() - time::Duration::minutes(1);
        super::super::save_with_quota_at(&credentials, source.clone(), "session=old", Some(&path))
            .unwrap();
        refresh_with(&source, &path, now, || Ok(Some(reading(&source, now)))).unwrap();
        let sibling = info("second");
        refresh_with(&sibling, &path, now, || Ok(Some(reading(&sibling, now)))).unwrap();
        super::super::save_with_quota_at(&credentials, source.clone(), "session=new", Some(&path))
            .unwrap();
        let records = quota_store::load(&path).unwrap();
        let current = quota_store::current_source_records(&records);
        let selected: Vec<_> = current
            .iter()
            .filter(|r| r.credential_id.as_deref() == Some("primary"))
            .collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].quota_pool, None);
        assert_eq!(selected[0].quota_remaining_percent, None);
        assert!(selected[0].account_usage.is_none());
        assert!(current
            .iter()
            .any(|r| r.credential_id.as_deref() == Some("second")
                && r.quota_remaining_percent == Some(40.0)));
        let identity = ExecutionIdentity::legacy_candidate(
            "mistral-dashboard",
            None::<String>,
            None::<String>,
        );
        assert!(quota_store::latest_windows_for_identity_and_credential(
            &records,
            &identity,
            Some("primary")
        )
        .is_empty());
    }
    #[test]
    #[cfg(unix)]
    fn replacement_and_removal_retire_future_dated_source_history() {
        for remove in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let credentials = dir.path().join("credentials");
            let path = dir.path().join("quota.jsonl");
            let source = info("primary");
            super::super::save_with_quota_at(
                &credentials,
                source.clone(),
                "session=old",
                Some(&path),
            )
            .unwrap();
            let future = OffsetDateTime::now_utc() + time::Duration::days(7);
            refresh_with(&source, &path, future, || {
                Ok(Some(reading(&source, future)))
            })
            .unwrap();
            let sibling = info("second");
            refresh_with(&sibling, &path, future, || {
                Ok(Some(reading(&sibling, future)))
            })
            .unwrap();
            if remove {
                super::super::remove_at(&credentials, &source.id, &path).unwrap();
            } else {
                super::super::save_with_quota_at(
                    &credentials,
                    source.clone(),
                    "session=new",
                    Some(&path),
                )
                .unwrap();
            }
            let records = quota_store::load(&path).unwrap();
            let current = quota_store::current_source_records(&records);
            assert!(current
                .iter()
                .filter(|r| r.credential_id.as_deref() == Some("primary"))
                .all(|r| r.quota_remaining_percent.is_none() && r.account_usage.is_none()));
            assert!(current
                .iter()
                .any(|r| r.credential_id.as_deref() == Some("second")
                    && r.quota_remaining_percent == Some(40.0)));
            let identity = ExecutionIdentity::legacy_candidate(
                "mistral-dashboard",
                None::<String>,
                None::<String>,
            );
            assert!(quota_store::latest_windows_for_identity_and_credential(
                &records,
                &identity,
                Some("primary")
            )
            .is_empty());
            assert_eq!(
                records.len(),
                3,
                "retired data stays in append-only history"
            );
        }
    }
    #[test]
    #[cfg(unix)]
    fn replacement_same_verified_pool_cannot_reactivate_previous_generation() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = dir.path().join("credentials");
        let path = dir.path().join("quota.jsonl");
        let source = info("primary");
        super::super::save_with_quota_at(&credentials, source.clone(), "session=old", Some(&path))
            .unwrap();
        let now = OffsetDateTime::now_utc();
        let future = now + time::Duration::days(7);
        refresh_with(&source, &path, future, || {
            Ok(Some(reading(&source, future)))
        })
        .unwrap();
        let sibling = info("second");
        refresh_with(&sibling, &path, future, || {
            Ok(Some(reading(&sibling, future)))
        })
        .unwrap();
        super::super::save_with_quota_at(&credentials, source.clone(), "session=new", Some(&path))
            .unwrap();
        let mut fresh = reading(&source, now);
        fresh.quota_remaining_percent = Some(70.0);
        refresh_with(&source, &path, now, || Ok(Some(fresh))).unwrap();
        let mut independent_window = reading(&source, now);
        independent_window.quota_window = Some("daily".into());
        independent_window.quota_remaining_percent = Some(90.0);
        refresh_with(&source, &path, now, || Ok(Some(independent_window))).unwrap();
        let records = quota_store::load(&path).unwrap();
        let identity = ExecutionIdentity::legacy_candidate(
            "mistral-dashboard",
            None::<String>,
            None::<String>,
        );
        let readings = quota_store::latest_windows_for_identity_and_credential(
            &records,
            &identity,
            Some("primary"),
        );
        assert_eq!(readings.len(), 2);
        assert!(readings.iter().any(|r| r.quota_window.as_deref()
            == Some("vibe-code-included-monthly")
            && r.quota_remaining_percent == Some(70.0)));
        assert!(readings
            .iter()
            .any(|r| r.quota_window.as_deref() == Some("daily")
                && r.quota_remaining_percent == Some(90.0)));
        assert!(quota_store::current_source_records(&records)
            .iter()
            .any(|r| r.credential_id.as_deref() == Some("second")
                && r.quota_remaining_percent == Some(40.0)));
    }
    #[test]
    #[cfg(unix)]
    fn in_flight_old_account_cannot_publish_after_rotation_or_removal() {
        for remove in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let credentials = dir.path().join("credentials");
            let path = dir.path().join("quota.jsonl");
            let source = info("primary");
            super::super::save_with_quota_at(
                &credentials,
                source.clone(),
                "session=old",
                Some(&path),
            )
            .unwrap();
            let result = refresh_selected_at(&credentials, &source.id, &path, |stored| {
                let selected = &stored.info;
                assert_eq!(stored.secret, "session=old");
                if remove {
                    super::super::remove_at(&credentials, &source.id, &path).unwrap();
                } else {
                    super::super::save_with_quota_at(
                        &credentials,
                        source.clone(),
                        "session=new",
                        Some(&path),
                    )
                    .unwrap();
                }
                Ok(vec![reading(selected, OffsetDateTime::now_utc())])
            });
            assert!(result.is_err());
            let records = quota_store::load(&path).unwrap();
            assert!(quota_store::current_source_records(&records)
                .iter()
                .all(|r| r.quota_remaining_percent.is_none() && r.account_usage.is_none()));
            assert_eq!(
                records.len(),
                1,
                "the old collector must not append after source mutation"
            );
        }
    }
    #[test]
    fn rotating_a_source_cannot_leave_its_previous_account_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let source = info("primary");
        let original = reading(&source, now);
        refresh_with(&source, &path, now, || Ok(Some(original.clone()))).unwrap();
        let mut rotated = reading(&source, now + time::Duration::minutes(1));
        rotated.backend_instance = Some("mistral-dashboard:other-customer".into());
        rotated.quota_pool = rotated.backend_instance.clone();
        refresh_with(&source, &path, now, || Ok(Some(rotated))).unwrap();
        let records = quota_store::load(&path).unwrap();
        let current = quota_store::current_source_records(&records);
        assert_eq!(current.len(), 1);
        assert_ne!(current[0].quota_pool, original.quota_pool);
    }
    #[test]
    fn removed_sources_hide_history_and_changed_providers_do_not_reuse_old_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let now = OffsetDateTime::now_utc();
        let source = info("primary");
        refresh_with(&source, &path, now, || Ok(Some(reading(&source, now)))).unwrap();
        removed(&source, &path).unwrap();
        assert!(quota_store::current_source_records(&quota_store::load(&path).unwrap()).is_empty());
        let mut changed = source.clone();
        changed.provider = "nous".into();
        changed.kind = CredentialKind::ApiKey;
        changed.env_var = Some("NOUS_API_KEY".into());
        let failed = refresh_with(&changed, &path, OffsetDateTime::now_utc(), || {
            anyhow::bail!("auth_required: synthetic expiration")
        })
        .unwrap();
        assert_eq!(failed.backend, "opencode");
        assert_eq!(failed.quota_pool, None);
        assert_eq!(
            failed.backend_instance.as_deref(),
            Some("credential:primary")
        );
    }
    #[test]
    fn a_bound_source_cannot_inherit_ambient_or_sibling_quota() {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let mut ambient = reading(&info("ambient"), now);
        ambient.credential_id = None;
        let mut sibling = reading(&info("sibling"), now);
        sibling.credential_id = Some("sibling".into());
        let identity = ExecutionIdentity::legacy_candidate(
            "mistral-dashboard",
            None::<String>,
            None::<String>,
        );
        assert!(quota_store::latest_windows_for_identity_and_credential(
            &[ambient, sibling],
            &identity,
            Some("bound")
        )
        .is_empty());
    }
    #[test]
    fn same_account_sources_share_one_pool_and_failure_invalidates_only_its_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let a = info("primary");
        let b = info("second");
        let first = refresh_with(&a, &path, now, || Ok(Some(reading(&a, now)))).unwrap();
        let second = refresh_with(&b, &path, now, || Ok(Some(reading(&b, now)))).unwrap();
        assert_eq!(first.quota_pool, second.quota_pool);
        let failed = refresh_with(&a, &path, now + time::Duration::minutes(1), || {
            anyhow::bail!("auth_required: synthetic expiration")
        })
        .unwrap();
        assert_eq!(failed.credential_id.as_deref(), Some("primary"));
        assert_eq!(failed.quota_pool, first.quota_pool);
        assert!(failed.quota_remaining_percent.is_none());
        let mut identity = ExecutionIdentity::legacy_candidate(
            "mistral-dashboard",
            None::<String>,
            first.quota_pool,
        );
        identity.backend_instance = first.backend_instance.unwrap();
        let records = quota_store::load(&path).unwrap();
        let windows = quota_store::latest_windows_for_identity(&records, &identity);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].credential_id.as_deref(), Some("second"));
    }

    #[test]
    #[cfg(unix)]
    fn mistral_login_signs_in_only_when_the_cached_session_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("credentials");
        let source = CredentialInfo {
            id: "console".into(),
            provider: "mistral".into(),
            kind: CredentialKind::MistralLogin,
            account_label: "Mistral console".into(),
            env_var: None,
        };
        let login = r#"{"email":"owner@example.com","password":"synthetic-pass"}"#;
        super::super::save_at(&root, source.clone(), login).unwrap();
        let now = OffsetDateTime::now_utc();
        let dashboard = |accepted: &'static str| {
            let source = source.clone();
            move |cookie: &str| {
                if cookie == accepted {
                    Ok(reading(&source, now))
                } else {
                    anyhow::bail!("auth_required: Mistral dashboard session expired")
                }
            }
        };
        let never = |_: &str, _: &str| -> Result<String> { panic!("must not sign in") };

        // No cached session: sign in once with the stored login, then cache it.
        let stored = super::super::read_at(&root, "console").unwrap();
        let signed_in = |email: &str, password: &str| {
            assert_eq!((email, password), ("owner@example.com", "synthetic-pass"));
            Ok("ory_session_a=1".to_string())
        };
        refresh_mistral_login_with(&root, &stored, dashboard("ory_session_a=1"), signed_in)
            .unwrap();
        let stored = super::super::read_at(&root, "console").unwrap();
        assert_eq!(stored.session.as_deref(), Some("ory_session_a=1"));

        // A valid cached session is reused without a login.
        refresh_mistral_login_with(&root, &stored, dashboard("ory_session_a=1"), never).unwrap();

        // An expired session triggers exactly one re-login.
        refresh_mistral_login_with(&root, &stored, dashboard("ory_session_b=2"), |_, _| {
            Ok("ory_session_b=2".to_string())
        })
        .unwrap();
        let stored = super::super::read_at(&root, "console").unwrap();
        assert_eq!(stored.session.as_deref(), Some("ory_session_b=2"));

        // A non-auth failure is reported, not retried with the password.
        let outage = |_: &str| -> Result<QuotaObservationRecord> {
            anyhow::bail!("Mistral dashboard HTTP request failed")
        };
        assert!(refresh_mistral_login_with(&root, &stored, outage, never).is_err());

        // A fresh session the dashboard still rejects points at a second factor.
        let error = refresh_mistral_login_with(&root, &stored, dashboard("none"), |_, _| {
            Ok("ory_session_c=3".to_string())
        })
        .unwrap_err();
        assert!(error.to_string().contains("second factor"));

        // That refusal is remembered for this revision: no repeat sign-in.
        let stored = super::super::read_at(&root, "console").unwrap();
        assert_eq!(stored.session, None);
        let error = refresh_mistral_login_with(&root, &stored, dashboard("ory_session_c=3"), never)
            .unwrap_err();
        assert!(error.to_string().starts_with("auth_required:"));

        // A wrong password is remembered the same way until the login is saved again.
        super::super::save_at(&root, source.clone(), login).unwrap();
        let stored = super::super::read_at(&root, "console").unwrap();
        let wrong = |_: &str, _: &str| -> Result<String> {
            anyhow::bail!("auth_required: Mistral rejected the saved email or password")
        };
        assert!(refresh_mistral_login_with(&root, &stored, dashboard("none"), wrong).is_err());
        let stored = super::super::read_at(&root, "console").unwrap();
        assert!(refresh_mistral_login_with(&root, &stored, dashboard("none"), never).is_err());

        // A non-auth sign-in failure (outage) is not remembered.
        super::super::save_at(&root, source.clone(), login).unwrap();
        let stored = super::super::read_at(&root, "console").unwrap();
        let down = |_: &str, _: &str| -> Result<String> {
            anyhow::bail!("Mistral sign-in failed (HTTP 500)")
        };
        assert!(refresh_mistral_login_with(&root, &stored, dashboard("none"), down).is_err());
        let stored = super::super::read_at(&root, "console").unwrap();
        assert_eq!(stored.login_rejected, None);
    }
}
