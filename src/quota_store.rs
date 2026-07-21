//! #166 (within #151): durable store for *account-level* quota observations.
//!
//! Per-attempt usage (tokens, per-attempt cost) is already recorded in the
//! ledger via `usage.rs`'s structured parsers. This module holds the separate
//! *account-level* quota picture that only surfaces through a backend's own
//! status/quota endpoint — e.g. `codex status --json` — which is not part
//! of any single attempt's log.
//!
//! The store mirrors `availability.rs`'s design philosophy: append-only JSONL
//! (not an in-place keyed map), so concurrent GAH processes can never erase
//! each other's writes, and "current state" for a (backend, model) scope is
//! derived by reading the latest record in its scope. A missing file is a
//! clean empty state, never an error.

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaObservationRecord {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_used_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_remaining_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_reset_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_source: Option<String>,
}

/// Global, not per-profile (like `availability.rs`): Codex/Claude/AGY
/// subscription limits are shared across every repo GAH touches.
pub fn store_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        Path::new(&dir).join("gah").join("quota_observations.jsonl")
    } else {
        Path::new(&std::env::var("HOME").unwrap_or_default())
            .join(".local")
            .join("state")
            .join("gah")
            .join("quota_observations.jsonl")
    }
}

/// Load all records. A missing file is an empty list; any read/parse error
/// is swallowed to an empty list so callers can always safely enrich a report
/// without a quota store present (e.g. in hermetic tests).
pub fn load(state_path: &Path) -> Result<Vec<QuotaObservationRecord>> {
    if !state_path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(state_path).context("read quota store")?;
    let mut records = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Skip only the malformed line, not the whole file: one corrupt JSONL
        // record (e.g. a partial write) must not discard every valid
        // observation before/after it. Mirrors availability.rs's resilience.
        match serde_json::from_str::<QuotaObservationRecord>(line) {
            Ok(mut rec) => {
                rec.backend = crate::config::canonical_backend_name(&rec.backend).to_string();
                records.push(rec);
            }
            Err(_) => continue,
        }
    }
    Ok(records)
}

/// Load from the canonical global path, swallowing any error to an empty list.
pub fn load_account_observations() -> Vec<QuotaObservationRecord> {
    load(&store_path()).unwrap_or_default()
}

/// Append one record under an exclusive lock. Missing parent dirs are created.
pub fn append(state_path: &Path, rec: &QuotaObservationRecord) -> Result<()> {
    for (field, value) in [
        ("backend instance", rec.backend_instance.as_deref()),
        ("quota pool", rec.quota_pool.as_deref()),
    ] {
        if let Some(value) = value {
            let normalized = crate::execution_identity::validate_secret_safe_label(field, value)?;
            if normalized != value {
                anyhow::bail!("{field} must not contain surrounding whitespace");
            }
        }
    }
    if let Some(parent) = state_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_path)
        .context("open quota store")?;
    file.lock_exclusive()
        .with_context(|| format!("locking {}", state_path.display()))?;
    let line = serde_json::to_string(rec).context("serialize quota observation")?;
    writeln!(file, "{line}").context("write quota observation")?;
    let _ = file.unlock();
    Ok(())
}

/// Most-recent observation for a (backend, model) scope that actually carries
/// quota data (used/remaining percent, window, or reset). `model` of `None`
/// matches account-level records that have no model qualifier.
pub fn latest_for<'a>(
    records: &'a [QuotaObservationRecord],
    backend: &str,
    model: Option<&'a str>,
) -> Option<&'a QuotaObservationRecord> {
    records
        .iter()
        .filter(|r| {
            r.backend == backend
                && r.model.as_deref() == model
                && r.backend_instance.is_none()
                && r.quota_pool.is_none()
                && has_quota_data(r)
        })
        .max_by(|a, b| a.observed_at.cmp(&b.observed_at))
}

/// Most-recent observation for an exact execution identity, with a
/// deterministic fallback to legacy instance-unknown rows. A row for a
/// different explicit instance never matches, even when backend/model agree.
pub fn latest_for_identity<'a>(
    records: &'a [QuotaObservationRecord],
    identity: &crate::execution_identity::ExecutionIdentity,
) -> Option<&'a QuotaObservationRecord> {
    records
        .iter()
        .filter(|record| {
            record.backend == identity.logical_backend
                && (record.model.is_none() || record.model == identity.effective_model)
                && record
                    .backend_instance
                    .as_deref()
                    .is_none_or(|instance| instance == identity.backend_instance)
                && record
                    .quota_pool
                    .as_deref()
                    .is_none_or(|pool| Some(pool) == identity.quota_pool.as_deref())
                && has_quota_data(record)
        })
        .max_by(|left, right| left.observed_at.cmp(&right.observed_at))
}

fn has_quota_data(record: &QuotaObservationRecord) -> bool {
    record.quota_used_percent.is_some()
        || record.quota_remaining_percent.is_some()
        || record.quota_window.is_some()
        || record.quota_reset_at.is_some()
}

/// #166: run `codex status --json`, parse its account-level quota, and append
/// the result to the durable store. Returns the stored record when Codex
/// reported quota data, `Ok(None)` when it reported nothing (or `codex` is
/// unavailable), and an error only when the subprocess itself failed to spawn.
pub fn refresh_codex_and_store(
    codex_cmd: &str,
    model: Option<&str>,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let obs = crate::usage::refresh_codex_quota(codex_cmd, model)
        .map_err(|e| anyhow::anyhow!("`codex status --json` failed: {e}"))?;
    match obs {
        Some(obs) => {
            let rec = QuotaObservationRecord {
                backend: obs.backend.clone(),
                backend_instance: None,
                model: obs.model.clone(),
                quota_pool: None,
                quota_window: obs.quota_window.clone(),
                quota_used_percent: obs.quota_used_percent,
                quota_remaining_percent: obs.quota_remaining_percent,
                quota_reset_at: obs.quota_reset_at.clone(),
                observed_at: obs.observed_at.clone(),
                usage_source: obs.usage_source.clone(),
            };
            append(state_path, &rec)?;
            Ok(Some(rec))
        }
        None => Ok(None),
    }
}

/// Refresh and persist account quota for one explicit execution identity.
pub fn refresh_codex_and_store_for_identity(
    codex_cmd: &str,
    identity: &crate::execution_identity::ExecutionIdentity,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let observation =
        crate::usage::refresh_codex_quota(codex_cmd, identity.effective_model.as_deref())
            .map_err(|error| anyhow::anyhow!("`codex status --json` failed: {error}"))?;
    let Some(observation) = observation else {
        return Ok(None);
    };
    let record = QuotaObservationRecord {
        backend: identity.logical_backend.clone(),
        backend_instance: Some(identity.backend_instance.clone()),
        model: identity.effective_model.clone(),
        quota_pool: identity.quota_pool.clone(),
        quota_window: observation.quota_window,
        quota_used_percent: observation.quota_used_percent,
        quota_remaining_percent: observation.quota_remaining_percent,
        quota_reset_at: observation.quota_reset_at,
        observed_at: observation.observed_at,
        usage_source: observation.usage_source,
    };
    append(state_path, &record)?;
    Ok(Some(record))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota_observations.jsonl");
        (dir, path)
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
                quota_used_percent: Some(25.0),
                quota_remaining_percent: Some(75.0),
                quota_reset_at: Some("2026-04-29T12:00:00Z".into()),
                observed_at: Some("2026-04-28T10:00:00Z".into()),
                usage_source: Some("codex_status_json".into()),
            },
        )
        .unwrap();

        let records = load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].backend, "codex");
        assert_eq!(records[0].quota_used_percent, Some(25.0));
        assert_eq!(records[0].quota_remaining_percent, Some(75.0));
    }

    #[test]
    fn latest_for_picks_newest_and_scopes_by_model() {
        let (_dir, path) = tmp_store();
        for (ts, pct) in [
            ("2026-04-28T10:00:00Z", 10.0),
            ("2026-04-29T10:00:00Z", 40.0),
        ] {
            append(
                &path,
                &QuotaObservationRecord {
                    backend: "codex".into(),
                    backend_instance: None,
                    model: None,
                    quota_pool: None,
                    quota_window: Some("300m".into()),
                    quota_used_percent: Some(pct),
                    quota_remaining_percent: Some(100.0 - pct),
                    quota_reset_at: None,
                    observed_at: Some(ts.into()),
                    usage_source: Some("codex_status_json".into()),
                },
            )
            .unwrap();
        }
        // A different backend must not be selected.
        append(
            &path,
            &QuotaObservationRecord {
                backend: "agy".into(),
                backend_instance: None,
                model: None,
                quota_pool: None,
                quota_window: Some("AGY individual quota".into()),
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: Some("in 16m44s".into()),
                observed_at: Some("2026-04-29T11:00:00Z".into()),
                usage_source: Some("agy_cli_log_delta".into()),
            },
        )
        .unwrap();

        let records = load(&path).unwrap();
        let latest = latest_for(&records, "codex", None).unwrap();
        assert_eq!(latest.observed_at.as_deref(), Some("2026-04-29T10:00:00Z"));
        assert_eq!(latest.quota_used_percent, Some(40.0));
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
            quota_used_percent: Some(10.0),
            quota_remaining_percent: Some(90.0),
            quota_reset_at: None,
            observed_at: Some("2026-04-28T10:00:00Z".into()),
            usage_source: Some("codex_status_json".into()),
        };
        let good2 = QuotaObservationRecord {
            quota_used_percent: Some(20.0),
            observed_at: Some("2026-04-29T10:00:00Z".into()),
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
        assert_eq!(records[0].quota_used_percent, Some(10.0));
        assert_eq!(records[1].quota_used_percent, Some(20.0));
    }

    #[test]
    fn latest_for_ignores_data_less_records() {
        let (_dir, path) = tmp_store();
        append(
            &path,
            &QuotaObservationRecord {
                backend: "codex".into(),
                backend_instance: None,
                model: None,
                quota_pool: None,
                quota_window: None,
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: None,
                observed_at: Some("2026-04-29T10:00:00Z".into()),
                usage_source: Some("codex_status_json".into()),
            },
        )
        .unwrap();
        assert!(latest_for(&load(&path).unwrap(), "codex", None).is_none());
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
            quota_used_percent: Some(percent),
            quota_remaining_percent: Some(100.0 - percent),
            quota_reset_at: None,
            observed_at: Some(observed_at.into()),
            usage_source: Some("test".into()),
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
    fn latest_identity_observation_never_crosses_explicit_instances() {
        let records = vec![
            scoped_record(Some("account-a"), 10.0, "2026-07-20T10:00:00Z"),
            scoped_record(Some("account-b"), 70.0, "2026-07-20T11:00:00Z"),
        ];

        let first = latest_for_identity(&records, &identity("account-a")).unwrap();
        let second = latest_for_identity(&records, &identity("account-b")).unwrap();

        assert_eq!(first.quota_used_percent, Some(10.0));
        assert_eq!(second.quota_used_percent, Some(70.0));
        assert!(
            latest_for(&records, "opencode", Some("shared-model")).is_none(),
            "legacy aggregation must not erase explicit instance identity"
        );
    }

    #[test]
    fn identity_observation_reads_legacy_unknown_without_assigning_it() {
        let records = vec![scoped_record(None, 40.0, "2026-07-20T10:00:00Z")];

        let observed = latest_for_identity(&records, &identity("account-a")).unwrap();

        assert_eq!(observed.backend_instance, None);
        assert_eq!(observed.quota_used_percent, Some(40.0));
    }

    #[test]
    fn account_level_observation_applies_to_models_on_the_same_instance() {
        let mut account = scoped_record(Some("account-a"), 55.0, "2026-07-20T10:00:00Z");
        account.model = None;
        let records = [account];

        let observed = latest_for_identity(&records, &identity("account-a")).unwrap();

        assert_eq!(observed.model, None);
        assert_eq!(observed.quota_used_percent, Some(55.0));
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
}
