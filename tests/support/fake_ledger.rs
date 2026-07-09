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
