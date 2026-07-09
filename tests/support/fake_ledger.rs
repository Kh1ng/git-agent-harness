//! Temp-ledger fixture builder for hermetic test harness.
//!
//! Writes real JSONL ledger entries and reads them back as
//! `serde_json::Value`. Preserves the production JSONL format without
//! importing `LedgerEntry` (integration tests can't access crate-internal
//! types).

use std::fs;
use std::path::{Path, PathBuf};

/// A single ledger entry value, represented as `serde_json::Value`.
pub type LedgerEntry = serde_json::Value;

/// Build a production-compatible full-schema ledger fixture entry.
///
/// Partial `json!()` entries missing required fields are silently dropped
/// by `ledger::read_entries` → empty count map → false confidence.
/// This function produces every field the production `LedgerEntry` struct
/// requires (fields without `#[serde(default)]`).
///
/// Keep this in sync with `src/ledger.rs:LedgerEntry` required fields and
/// `REQUIRED` array in `validate_production_schema()`.
pub fn ledger_entry_full(
    mode: &str,
    branch: &str,
    reason: Option<&str>,
    work_id: &str,
    ts: &str,
) -> LedgerEntry {
    let mut m = serde_json::Map::new();
    m.insert("timestamp".into(), serde_json::Value::String(ts.into()));
    m.insert("session_id".into(), serde_json::Value::Null);
    m.insert("profile".into(), serde_json::Value::String("test".into()));
    m.insert(
        "display_name".into(),
        serde_json::Value::String("Test Repo".into()),
    );
    m.insert("repo_id".into(), serde_json::Value::String("test".into()));
    m.insert(
        "repo".into(),
        serde_json::Value::String("owner/repo".into()),
    );
    m.insert(
        "local_path".into(),
        serde_json::Value::String("/tmp/repo".into()),
    );
    m.insert(
        "provider".into(),
        serde_json::Value::String("github".into()),
    );
    m.insert(
        "backend".into(),
        serde_json::Value::String("openhands".into()),
    );
    m.insert(
        "requested_backend".into(),
        serde_json::Value::String("openhands".into()),
    );
    m.insert(
        "effective_backend".into(),
        serde_json::Value::String("openhands".into()),
    );
    m.insert("requested_model".into(), serde_json::Value::Null);
    m.insert("effective_model".into(), serde_json::Value::Null);
    m.insert("routing_reason".into(), serde_json::Value::Null);
    m.insert("fallback_used".into(), serde_json::Value::Bool(false));
    m.insert("confidence_impact".into(), serde_json::Value::Null);
    m.insert("human_required".into(), serde_json::Value::Bool(false));
    m.insert("mode".into(), serde_json::Value::String(mode.into()));
    m.insert(
        "target_summary".into(),
        serde_json::Value::String(branch.into()),
    );
    m.insert("work_id".into(), serde_json::Value::String(work_id.into()));
    m.insert("work_title".into(), serde_json::Value::Null);
    m.insert("branch".into(), serde_json::Value::String(branch.into()));
    m.insert("session_dir".into(), serde_json::Value::Null);
    m.insert("duration_seconds".into(), serde_json::Value::Null);
    m.insert("backend_exit_code".into(), serde_json::Value::Null);
    m.insert("validation_result".into(), serde_json::Value::Null);
    m.insert("review_verdict".into(), serde_json::Value::Null);
    m.insert("review_confidence".into(), serde_json::Value::Null);
    m.insert("reviewer_backend".into(), serde_json::Value::Null);
    m.insert("reviewer_model".into(), serde_json::Value::Null);
    m.insert("commit_attempted".into(), serde_json::Value::Bool(false));
    m.insert("commit_created".into(), serde_json::Value::Bool(false));
    m.insert("push_attempted".into(), serde_json::Value::Bool(false));
    m.insert("push_succeeded".into(), serde_json::Value::Bool(false));
    m.insert("mr_attempted".into(), serde_json::Value::Bool(false));
    m.insert("mr_created".into(), serde_json::Value::Bool(false));
    m.insert("mr_url".into(), serde_json::Value::Null);
    m.insert("files_changed".into(), serde_json::Value::Null);
    m.insert("insertions".into(), serde_json::Value::Null);
    m.insert("deletions".into(), serde_json::Value::Null);
    m.insert("error_summary".into(), serde_json::Value::Null);
    m.insert("failure_class".into(), serde_json::Value::Null);
    m.insert("failure_stage".into(), serde_json::Value::Null);
    m.insert(
        "attempts_started".into(),
        serde_json::Value::Number(1.into()),
    );
    m.insert(
        "attempts_completed".into(),
        serde_json::Value::Number(1.into()),
    );
    m.insert("attempts".into(), serde_json::Value::Array(vec![]));
    m.insert(
        "dispatch_reason".into(),
        match reason {
            Some(r) => serde_json::Value::String(r.into()),
            None => serde_json::Value::Null,
        },
    );
    m.insert("usage".into(), serde_json::json!({}));
    serde_json::Value::Object(m)
}

/// A builder that collects JSON values and writes them into a temporary
/// JSONL file using the production JSONL format (one JSON object per line).
pub struct TestLedger {
    entries: Vec<LedgerEntry>,
}

impl TestLedger {
    pub fn new() -> Self {
        Self { entries: vec![] }
    }

    pub fn with_entry(mut self, entry: LedgerEntry) -> Self {
        self.entries.push(entry);
        self
    }

    /// Write all entries to `path` as JSONL.
    pub fn write_to(&self, path: &Path) -> std::io::Result<PathBuf> {
        let mut content = String::new();
        for entry in &self.entries {
            content.push_str(&serde_json::to_string(entry)?);
            content.push('\n');
        }
        fs::write(path, &content)?;
        Ok(path.to_path_buf())
    }

    /// Convenience: write into `dir/ledger.jsonl`.
    pub fn write_into(&self, dir: &Path) -> std::io::Result<PathBuf> {
        self.write_to(&dir.join("ledger.jsonl"))
    }

    /// Read entries back from a JSONL file as `serde_json::Value`.
    pub fn read_from(path: &Path) -> std::io::Result<Vec<LedgerEntry>> {
        let text = fs::read_to_string(path)?;
        let entries: Vec<LedgerEntry> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        Ok(entries)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Validate that every fixture entry has all fields required by the
    /// production `LedgerEntry` struct (fields WITHOUT `#[serde(default)]`).
    ///
    /// A partial JSON fixture missing a required field will be silently
    /// dropped by `ledger::read_entries` — `serde_json` returns an error
    /// and the `filter_map(|l| serde_json::from_str(l).ok())` skips it.
    /// This guard catches the problem at harness setup rather than letting
    /// the test pass with an empty count map and false confidence.
    pub fn validate_production_schema(&self) -> Result<(), String> {
        // Required field names from production LedgerEntry — every field
        // that does NOT carry `#[serde(default)]`.  Keep in sync with
        // `src/ledger.rs:LedgerEntry`.
        const REQUIRED: &[&str] = &[
            "timestamp",
            "session_id",
            "profile",
            "display_name",
            "repo_id",
            "repo",
            "local_path",
            "provider",
            "backend",
            "requested_backend",
            "effective_backend",
            "requested_model",
            "effective_model",
            "routing_reason",
            "fallback_used",
            "confidence_impact",
            "human_required",
            "mode",
            "target_summary",
            "branch",
            "session_dir",
            "duration_seconds",
            "backend_exit_code",
            "validation_result",
            "commit_attempted",
            "commit_created",
            "push_attempted",
            "push_succeeded",
            "mr_attempted",
            "mr_created",
            "mr_url",
            "files_changed",
            "insertions",
            "deletions",
            "error_summary",
            "usage",
        ];

        for (i, entry) in self.entries.iter().enumerate() {
            let obj = entry
                .as_object()
                .ok_or_else(|| format!("fixture entry {i} is not a JSON object"))?;
            for &field in REQUIRED {
                if !obj.contains_key(field) {
                    return Err(format!(
                        "fixture entry {i} missing required field \"{field}\": \
                         {entry}\n\
                         HINT: use ledger_entry_full() instead of partial json!() \
                         for production-path tests."
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Default for TestLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ledger_writes_readable() {
        let tmp = tempfile::tempdir().unwrap();
        let path = TestLedger::new().write_into(tmp.path()).unwrap();
        assert!(path.exists());
        let entries = TestLedger::read_from(&path).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn round_trips_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = serde_json::json!({"work_id": "TICKET-001", "profile": "test"});
        let path = TestLedger::new()
            .with_entry(entry)
            .write_into(tmp.path())
            .unwrap();
        let entries = TestLedger::read_from(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["work_id"], "TICKET-001");
    }
}
