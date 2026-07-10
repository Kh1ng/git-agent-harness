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

use crate::ledger::summary::GroupQuotaObservation;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaObservationRecord {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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

impl QuotaObservationRecord {
    /// Convert a structured `GroupQuotaObservation` (the shape `usage.rs`
    /// produces) into a durable store record.
    pub fn from_observation(obs: &GroupQuotaObservation) -> Self {
        QuotaObservationRecord {
            backend: obs.backend.clone(),
            model: obs.model.clone(),
            quota_window: obs.quota_window.clone(),
            quota_used_percent: obs.quota_used_percent,
            quota_remaining_percent: obs.quota_remaining_percent,
            quota_reset_at: obs.quota_reset_at.clone(),
            observed_at: obs.observed_at.clone(),
            usage_source: obs.usage_source.clone(),
        }
    }

    /// Convert a stored record back into the report-facing
    /// `GroupQuotaObservation` shape (the "Quota page" consumes these).
    pub fn to_observation(&self) -> GroupQuotaObservation {
        GroupQuotaObservation {
            backend: self.backend.clone(),
            model: self.model.clone(),
            quota_window: self.quota_window.clone(),
            quota_used_percent: self.quota_used_percent,
            quota_remaining_percent: self.quota_remaining_percent,
            quota_reset_at: self.quota_reset_at.clone(),
            observed_at: self.observed_at.clone(),
            usage_source: self.usage_source.clone(),
        }
    }
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
        records.push(
            serde_json::from_str::<QuotaObservationRecord>(line)
                .context("parse quota observation record")?,
        );
    }
    Ok(records)
}

/// Load from the canonical global path, swallowing any error to an empty list.
pub fn load_account_observations() -> Vec<QuotaObservationRecord> {
    load(&store_path()).unwrap_or_default()
}

/// Append one record under an exclusive lock. Missing parent dirs are created.
pub fn append(state_path: &Path, rec: &QuotaObservationRecord) -> Result<()> {
    if let Some(parent) = state_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_path)
        .context("open quota store")?;
    let _ = file.lock_exclusive();
    let line = serde_json::to_string(rec).context("serialize quota observation")?;
    writeln!(file, "{line}").context("write quota observation")?;
    let _ = file.unlock();
    Ok(())
}

/// #166: run `codex status --json`, parse its account-level quota, and append
/// the result to the durable store. Returns the stored record when Codex
/// reported quota data, `Ok(None)` when it reported nothing (or `codex` is
/// unavailable), and an error only when the subprocess itself failed to spawn.
pub fn refresh_codex_and_store(
    codex_cmd: &Path,
    model: Option<&str>,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let obs = crate::usage::refresh_codex_quota(codex_cmd, model)
        .map_err(|e| anyhow::anyhow!("`codex status --json` failed: {e}"))?;
    match obs {
        Some(obs) => {
            let rec = QuotaObservationRecord::from_observation(&obs);
            append(state_path, &rec)?;
            Ok(Some(rec))
        }
        None => Ok(None),
    }
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
                model: Some("gpt-5".into()),
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

        let obs = records[0].to_observation();
        assert_eq!(obs.backend, "codex");
        assert_eq!(obs.quota_used_percent, Some(25.0));
    }

    #[test]
    fn refresh_codex_and_store_writes_when_quota_present() {
        // Use a fake executable that prints the fixture payload and exits 0.
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("codex");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\ncat <<'EOF'\n{}\nEOF\n",
                include_str!("../tests/fixtures/codex-status-json.json")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake, perms).unwrap();
        }

        let store_dir = tempfile::tempdir().unwrap();
        let store_path = store_dir.path().join("quota_observations.jsonl");

        let rec = refresh_codex_and_store(&fake, Some("gpt-5"), &store_path)
            .unwrap()
            .expect("must store an observation when codex reports quota");
        assert_eq!(rec.backend, "codex");
        assert_eq!(rec.quota_used_percent, Some(25.0));
        assert_eq!(rec.quota_remaining_percent, Some(75.0));

        let loaded = load(&store_path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].backend, "codex");
    }

    #[test]
    fn refresh_codex_and_store_is_none_when_codex_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("codex");
        std::fs::write(&fake, "#!/bin/sh\necho '{\"some\":\"data\"}'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake, perms).unwrap();
        }

        let store_dir = tempfile::tempdir().unwrap();
        let store_path = store_dir.path().join("quota_observations.jsonl");

        let rec = refresh_codex_and_store(&fake, None, &store_path).unwrap();
        assert!(rec.is_none());
        // Never fabricates a record.
        assert!(load(&store_path).unwrap().is_empty());
    }
}
