//! Persist native Antigravity readings under their independent model pools.
use super::{append, append_scoped, QuotaObservationRecord};
use anyhow::Result;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// The store instance that records when this probe last ran for an account.
/// It is not a routing scope, so a failed or empty check filed under it
/// throttles the next probe without invalidating any pool's readings,
/// including ones imported from another source.
pub(super) fn probe_instance(account: &str) -> String {
    format!("{account}:usage-probe")
}

/// The accounts the scheduled refresh samples for this profile, with the
/// HOME each one runs under. `agy-second` is sampled only with its own home.
pub(super) fn accounts(profile: &crate::config::Profile) -> Vec<(&'static str, Option<PathBuf>)> {
    let second = profile
        .agy_second_home
        .as_deref()
        .filter(|home| !home.trim().is_empty())
        .map(|home| ("agy-second", Some(PathBuf::from(home))));
    std::iter::once(("agy", None)).chain(second).collect()
}

/// Executable and HOME for an explicit `gah quota refresh --backend <account>`,
/// taken from the first profile that configures them.
pub fn launch(
    config: &crate::config::GahConfig,
    account: &str,
) -> Result<(String, Option<PathBuf>)> {
    let executable = config
        .profiles
        .values()
        .find_map(|profile| profile.configured_backend_path(account))
        .unwrap_or("agy")
        .to_string();
    let home = config
        .profiles
        .values()
        .flat_map(accounts)
        .find(|(name, _)| *name == account)
        .map(|(_, home)| home);
    match home {
        Some(home) => Ok((executable, home)),
        None => {
            anyhow::bail!("no profile sets agy_second_home, so {account} has no account to check")
        }
    }
}

pub fn refresh_and_store(
    executable: &str,
    account: &str,
    home: Option<&Path>,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let records = crate::usage::agy::refresh(executable, account, home)?;
    let first = append_scoped(state_path, records, None)?;
    append(
        state_path,
        &probe_marker(account, OffsetDateTime::now_utc()),
    )?;
    Ok(first)
}

fn probe_marker(account: &str, now: OffsetDateTime) -> QuotaObservationRecord {
    QuotaObservationRecord {
        backend: account.into(),
        backend_instance: Some(probe_instance(account)),
        credential_id: None,
        model: None,
        quota_pool: None,
        quota_window: None,
        quota_used_percent: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        observed_at: None,
        checked_at: now.format(&Rfc3339).ok(),
        check_error: None,
        usage_source: Some(crate::usage::agy::USAGE_SOURCE.into()),
        mistral_admin: None,
        account_usage: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_identity::ExecutionIdentity;

    fn reading(backend: &str, source: &str, now: OffsetDateTime) -> QuotaObservationRecord {
        serde_json::from_value(serde_json::json!({
            "backend": backend,
            "backend_instance": format!("{backend}:google-native"),
            "quota_pool": format!("{backend}:google-native"),
            "quota_window": "weekly",
            "quota_remaining_percent": 80.0,
            "usage_source": source,
            "checked_at": now.format(&Rfc3339).unwrap(),
            "observed_at": now.format(&Rfc3339).unwrap()
        }))
        .unwrap()
    }

    #[test]
    fn only_this_probes_own_check_throttles_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let now = OffsetDateTime::now_utc();
        // Account names no other test probes: in-flight checks are tracked
        // process-wide, so a shared name would race a sibling test.
        for account in ["agy-throttle-one", "agy-throttle-two"] {
            // A fresh reading imported from another source does not count as
            // this probe having run.
            append(&path, &reading(account, "cli_router", now)).unwrap();
            let due = super::super::maybe_refresh_backend_instance(
                &path,
                account,
                Some(&probe_instance(account)),
                now,
                || Ok(None),
            );
            due.expect("probe is due").join().unwrap();
            assert!(super::super::maybe_refresh_backend_instance(
                &path,
                account,
                Some(&probe_instance(account)),
                now,
                || panic!("not due")
            )
            .is_none());
        }
    }

    #[test]
    fn a_failed_probe_keeps_readings_from_other_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.jsonl");
        let now = OffsetDateTime::now_utc();
        append(&path, &reading("agy", "cli_router", now)).unwrap();
        super::super::maybe_refresh_backend_instance(
            &path,
            "agy",
            Some(&probe_instance("agy")),
            now + time::Duration::minutes(1),
            || anyhow::bail!("agy is not on PATH"),
        )
        .expect("probe is due")
        .join()
        .unwrap();

        let records = super::super::load(&path).unwrap();
        assert!(records.iter().any(|record| record.check_error.is_some()
            && record.backend_instance == Some(probe_instance("agy"))));
        let identity = ExecutionIdentity::legacy_candidate(
            "agy",
            Some("Gemini 3.5 Flash"),
            Some("agy:google-native"),
        );
        let windows = super::super::latest_windows_for_identity(&records, &identity);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].quota_remaining_percent, Some(80.0));
    }

    #[test]
    fn an_empty_second_home_is_not_an_account() {
        let mut profile = crate::config::tests::test_profile_for_notifications();
        profile.agy_second_home = Some("  ".into());
        assert_eq!(accounts(&profile), vec![("agy", None)]);
        profile.agy_second_home = Some("/accounts/second".into());
        assert_eq!(
            accounts(&profile)[1],
            ("agy-second", Some(PathBuf::from("/accounts/second")))
        );
    }
}
