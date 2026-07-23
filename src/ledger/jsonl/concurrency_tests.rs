use super::read_entries;
use crate::ledger::test_util as ledger_tests;
use std::io::Write;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn reader_waits_for_an_in_progress_jsonl_append() {
    let (_tmp, cfg) = ledger_tests::test_config();
    let path = cfg.defaults.ledger_path();
    let entry = crate::ledger::LedgerEntry::new(
        "test",
        &ledger_tests::profile(),
        "claude",
        "pm",
        "concurrent append",
        Some("reader-lock".into()),
        None,
    );
    let encoded = serde_json::to_vec(&entry).unwrap();
    let split = encoded.len() / 2;
    let writer_lock = crate::ledger::locking::exclusive(&path).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(&encoded[..split]).unwrap();
    file.flush().unwrap();

    std::thread::scope(|scope| {
        let (done_tx, done_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let reader_cfg = &cfg;
        scope.spawn(move || {
            started_tx.send(()).unwrap();
            done_tx.send(read_entries(reader_cfg)).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            matches!(
                done_rx.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "reader observed a record before its exclusive append completed"
        );

        file.write_all(&encoded[split..]).unwrap();
        file.write_all(b"\n").unwrap();
        file.flush().unwrap();
        drop(file);
        drop(writer_lock);

        let entries = done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].target_summary.as_deref(),
            Some("concurrent append")
        );
    });
}
