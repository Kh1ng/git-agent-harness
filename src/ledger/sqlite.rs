use super::{jsonl::parse_jsonl_entries, jsonl::read_entries, LedgerEntry};
use crate::config::GahConfig;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

pub fn db_path(cfg: &GahConfig) -> PathBuf {
    cfg.defaults.ledger_path().with_extension("db")
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ledger_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            profile TEXT NOT NULL,
            work_id TEXT,
            mode TEXT NOT NULL,
            backend TEXT NOT NULL,
            effective_backend TEXT NOT NULL,
            effective_model TEXT,
            requested_model TEXT,
            validation_result TEXT,
            review_verdict TEXT,
            human_required INTEGER NOT NULL,
            duration_seconds REAL,
            failure_class TEXT,
            total_tokens INTEGER,
            human_required_reason_code TEXT,
            actual_cost_usd REAL,
            estimated_cost_usd REAL,
            raw_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS ledger_sync_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            synced_byte_len INTEGER NOT NULL,
            synced_entry_count INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_ledger_entries_profile_ts
            ON ledger_entries(profile, timestamp);
        CREATE INDEX IF NOT EXISTS idx_ledger_entries_work_id
            ON ledger_entries(work_id);",
    )?;
    ensure_human_required_reason_code_column(conn)?;
    Ok(())
}

fn ensure_human_required_reason_code_column(conn: &Connection) -> Result<()> {
    if !has_column(conn, "ledger_entries", "human_required_reason_code")? {
        conn.execute(
            "ALTER TABLE ledger_entries ADD COLUMN human_required_reason_code TEXT",
            [],
        )?;
    }
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&pragma)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn load_sync_state(conn: &Connection) -> Result<Option<(u64, u64)>> {
    let mut stmt = conn.prepare(
        "SELECT synced_byte_len, synced_entry_count FROM ledger_sync_state WHERE id = 1",
    )?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        Ok(Some((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)))
    } else {
        Ok(None)
    }
}

fn store_sync_state(
    tx: &rusqlite::Transaction,
    synced_byte_len: u64,
    synced_entry_count: u64,
) -> Result<()> {
    tx.execute(
        "INSERT INTO ledger_sync_state (id, synced_byte_len, synced_entry_count)
         VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET
             synced_byte_len=excluded.synced_byte_len,
             synced_entry_count=excluded.synced_entry_count",
        params![synced_byte_len as i64, synced_entry_count as i64],
    )?;
    Ok(())
}

fn sync_full_from_entries(cfg: &GahConfig, entries: &[LedgerEntry]) -> Result<()> {
    let db_path = db_path(cfg);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut conn = Connection::open(&db_path)
        .with_context(|| format!("opening sqlite ledger {}", db_path.display()))?;
    ensure_schema(&conn)?;
    let synced_byte_len = fs::metadata(cfg.defaults.ledger_path())
        .with_context(|| {
            format!(
                "reading metadata for {}",
                cfg.defaults.ledger_path().display()
            )
        })?
        .len();
    let tx = conn.transaction().context("opening sqlite transaction")?;
    tx.execute("DELETE FROM ledger_entries", [])
        .context("clearing sqlite ledger mirror before resync")?;
    for entry in entries {
        insert_entry(&tx, entry)?;
    }
    store_sync_state(&tx, synced_byte_len, entries.len() as u64)?;
    tx.commit().context("committing sqlite ledger mirror")?;
    Ok(())
}

fn sync_incremental_from_jsonl(cfg: &GahConfig) -> Result<()> {
    let ledger_path = cfg.defaults.ledger_path();
    let db_path = db_path(cfg);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut conn = Connection::open(&db_path)
        .with_context(|| format!("opening sqlite ledger {}", db_path.display()))?;
    ensure_schema(&conn)?;

    let ledger_meta = match fs::metadata(&ledger_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let tx = conn.transaction().context("opening sqlite transaction")?;
            tx.execute("DELETE FROM ledger_entries", [])
                .context("clearing sqlite ledger mirror before resync")?;
            store_sync_state(&tx, 0, 0)?;
            tx.commit().context("committing sqlite ledger mirror")?;
            return Ok(());
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", ledger_path.display())),
    };

    let ledger_len = ledger_meta.len();
    let sync_state = load_sync_state(&conn)?;
    let Some((synced_byte_len, synced_entry_count)) = sync_state else {
        let entries = read_entries(cfg)?;
        return sync_full_from_entries(cfg, &entries);
    };

    if ledger_len < synced_byte_len {
        let entries = read_entries(cfg)?;
        return sync_full_from_entries(cfg, &entries);
    }

    if ledger_len == synced_byte_len {
        return Ok(());
    }

    let mut file =
        File::open(&ledger_path).with_context(|| format!("opening {}", ledger_path.display()))?;
    file.seek(SeekFrom::Start(synced_byte_len))
        .with_context(|| format!("seeking {}", ledger_path.display()))?;
    let mut tail = String::new();
    file.read_to_string(&mut tail)
        .with_context(|| format!("reading {}", ledger_path.display()))?;
    if tail.trim().is_empty() {
        return Ok(());
    }

    let entries = parse_jsonl_entries(&tail, &ledger_path, synced_entry_count as usize)?;

    let mut conn = conn;
    let tx = conn.transaction().context("opening sqlite transaction")?;
    for entry in &entries {
        insert_entry(&tx, entry)?;
    }
    store_sync_state(&tx, ledger_len, synced_entry_count + entries.len() as u64)?;
    tx.commit().context("committing sqlite ledger mirror")?;
    Ok(())
}

fn insert_entry(tx: &rusqlite::Transaction, entry: &LedgerEntry) -> Result<()> {
    let raw_json = serde_json::to_string(entry).context("serializing ledger entry")?;
    tx.execute(
        "INSERT INTO ledger_entries (
            timestamp, profile, work_id, mode, backend, effective_backend,
            effective_model, requested_model, validation_result, review_verdict,
            human_required, duration_seconds, failure_class, total_tokens,
            human_required_reason_code, actual_cost_usd, estimated_cost_usd, raw_json
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
        params![
            entry.timestamp,
            entry.profile,
            entry.work_id,
            entry.mode,
            entry.backend,
            entry.effective_backend,
            entry.effective_model,
            entry.requested_model,
            entry.validation_result,
            entry.review_verdict,
            entry.human_required as i64,
            entry.duration_seconds,
            entry.failure_class,
            entry.usage.total_tokens.map(|v| v as i64),
            entry.human_required_reason_code,
            entry.usage.actual_cost_usd,
            entry.usage.estimated_cost_usd,
            raw_json,
        ],
    )?;
    Ok(())
}

/// Rebuild the mirror wholesale from the JSONL ledger (still
/// authoritative) rather than incrementally inserting/updating rows in
/// lockstep with `append`/`backfill_review_verdict`. A full rebuild
/// trivially can't drift from the JSONL it's derived from, which an
/// incremental dual-write easily could (e.g. `backfill_review_verdict`
/// rewrites an arbitrary earlier line, not just the latest one).
pub fn sync_from_jsonl(cfg: &GahConfig) -> Result<()> {
    sync_incremental_from_jsonl(cfg)
}

pub fn rebuild_from_jsonl(cfg: &GahConfig) -> Result<()> {
    let entries = read_entries(cfg)?;
    sync_full_from_entries(cfg, &entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::append;
    use crate::ledger::test_util::{profile, test_config};

    #[test]
    fn sync_mirrors_appended_entries_and_stays_in_lockstep_on_backfill() {
        let (_tmp, cfg) = test_config();

        let mut entry =
            LedgerEntry::new("test", &profile(), "codex", "improve", "target", None, None);
        entry.branch = Some("gah/test-1".to_string());
        entry.effective_model = Some("gpt-5".to_string());
        append(&cfg, &entry).unwrap();

        let db_path = db_path(&cfg);
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ledger_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let review_verdict: Option<String> = conn
            .query_row(
                "SELECT review_verdict FROM ledger_entries LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(review_verdict, None);

        super::super::backfill_review_verdict(
            &cfg,
            "gah/test-1",
            super::super::ReviewVerdictBackfill {
                verdict: "APPROVE",
                confidence: "high",
                reviewer_backend: "claude",
                reviewer_model: Some("claude-sonnet-4"),
                reviewer_tier: None,
                review_gate_reason: None,
                review_source_sha: None,
                review_metadata_fingerprint: None,
                blocking_findings: &[],
                non_blocking_findings: &[],
                risk_notes: &[],
                evidence: &[],
                compatibility_evidence: &[],
            },
        )
        .unwrap();

        let review_verdict: Option<String> = conn
            .query_row(
                "SELECT review_verdict FROM ledger_entries LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(review_verdict.as_deref(), Some("APPROVE"));
    }

    #[test]
    fn sync_incrementally_appends_without_rereading_the_full_ledger() {
        let (_tmp, cfg) = test_config();
        crate::ledger::reset_read_entries_call_count(&cfg);

        let mut first = LedgerEntry::new(
            "test",
            &profile(),
            "codex",
            "improve",
            "target-a",
            None,
            None,
        );
        first.branch = Some("gah/test-1".to_string());
        append(&cfg, &first).unwrap();
        assert_eq!(crate::ledger::read_entries_call_count(&cfg), 1);

        let db_path = db_path(&cfg);
        let conn = Connection::open(&db_path).unwrap();
        let count_after_first: i64 = conn
            .query_row("SELECT COUNT(*) FROM ledger_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after_first, 1);

        let mut second = LedgerEntry::new(
            "test",
            &profile(),
            "codex",
            "improve",
            "target-b",
            None,
            None,
        );
        second.branch = Some("gah/test-2".to_string());
        append(&cfg, &second).unwrap();
        assert_eq!(crate::ledger::read_entries_call_count(&cfg), 1);

        let count_after_second: i64 = conn
            .query_row("SELECT COUNT(*) FROM ledger_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after_second, 2);
    }

    #[test]
    fn failed_full_rebuild_preserves_previous_rows_and_sync_state() {
        let (_tmp, cfg) = test_config();
        let first = LedgerEntry::new(
            "test",
            &profile(),
            "codex",
            "improve",
            "original",
            None,
            None,
        );
        append(&cfg, &first).unwrap();

        let db_path = db_path(&cfg);
        let conn = Connection::open(&db_path).unwrap();
        let state_before: (i64, i64) = conn
            .query_row(
                "SELECT synced_byte_len, synced_entry_count FROM ledger_sync_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_rebuild BEFORE INSERT ON ledger_entries
             BEGIN SELECT RAISE(FAIL, 'injected rebuild failure'); END;",
        )
        .unwrap();
        drop(conn);

        let replacement = LedgerEntry::new(
            "test",
            &profile(),
            "codex",
            "improve",
            "replacement",
            None,
            None,
        );
        let error = sync_full_from_entries(&cfg, &[replacement]).unwrap_err();
        assert!(error.to_string().contains("injected rebuild failure"));

        let conn = Connection::open(&db_path).unwrap();
        let rows: Vec<String> = conn
            .prepare("SELECT raw_json FROM ledger_entries ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("original"));
        let state_after: (i64, i64) = conn
            .query_row(
                "SELECT synced_byte_len, synced_entry_count FROM ledger_sync_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state_after, state_before);
    }
}
