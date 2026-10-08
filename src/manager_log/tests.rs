use super::*;
fn event() -> Event {
    serde_json::from_value(serde_json::json!({"schema_version":1,"ts":"2026-10-07T12:00:00Z","work_id":"TICKET-1475","phase":"verify"})).unwrap()
}
#[test]
fn validation_and_round_trip() {
    let e = event();
    e.validate().unwrap();
    let value = serde_json::to_value(&e).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 4);
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<Event>(value.clone()).unwrap()).unwrap(),
        value
    );
    for (key, bad) in [
        ("work_id", serde_json::json!("")),
        ("work_id", serde_json::json!("  ")),
        ("tier", serde_json::json!(0)),
        ("tier", serde_json::json!(5)),
        ("attempt", serde_json::json!(0)),
        ("schema_version", serde_json::json!(2)),
        ("ts", serde_json::json!("bad")),
        ("ts", serde_json::json!("2026-10-07T12:00:00+01:00")),
        ("phase", serde_json::json!("bogus")),
        ("diagnosis", serde_json::json!("bogus")),
        ("tokens", serde_json::json!(-1)),
        ("elapsed_seconds", serde_json::json!(-1)),
        ("manager_rounds", serde_json::json!(-1)),
        ("attempt", serde_json::json!(-1)),
    ] {
        let mut invalid = value.clone();
        invalid[key] = bad;
        assert!(
            serde_json::from_value::<Event>(invalid)
                .map(|e| e.validate().is_err())
                .unwrap_or(true),
            "{key}"
        );
    }
}
#[test]
fn summary_arithmetic_and_aliases() {
    let mut first = event();
    first.attempt = Some(3);
    first.manager_rounds = Some(4);
    first.tokens = Some(10);
    first.elapsed_seconds = Some(5);
    first.outcome = Some("passed".into());
    let mut second = first.clone();
    second.work_id = "#1475".into();
    second.attempt = Some(1);
    second.manager_rounds = Some(2);
    second.tokens = Some(20);
    second.elapsed_seconds = Some(7);
    second.outcome = None;
    second.phase = Phase::Merge;
    second.ts = "2026-10-07T13:00:00Z".into();
    let mut other = event();
    other.work_id = "other".into();
    let log = Events {
        schema_version: 1,
        path: "x".into(),
        skipped_lines: 0,
        events: vec![first, other, second],
    };
    let summary = summarize(&log).unwrap();
    assert_eq!(summary.items.len(), 2);
    let i = &summary.items[0];
    assert_eq!(
        (
            &i.work_id,
            i.events,
            i.attempts,
            i.manager_rounds,
            i.tokens,
            i.elapsed_seconds
        ),
        (&"TICKET-1475".to_string(), 2, 3, 4, 30, 12)
    );
    assert_eq!(i.last_outcome.as_deref(), Some("passed"));
    assert_eq!(i.last_phase, Phase::Merge);
    assert_eq!(i.first_ts, "2026-10-07T12:00:00Z");
    assert_eq!(i.last_ts, "2026-10-07T13:00:00Z");
    assert_eq!(summary.items[1].attempts, 0);
}
#[test]
fn a_bare_number_is_the_same_item_as_its_issue_forms() {
    for (a, b) in [
        ("1475", "#1475"),
        ("1475", "TICKET-1475"),
        (" 1475 ", "#1475"),
        ("#1475", "TICKET-1475"),
    ] {
        assert!(aliases(a, b) && aliases(b, a), "{a} {b}");
    }
    for (a, b) in [("1475", "#14750"), ("1475", "other"), ("job-1", "job-2")] {
        assert!(!aliases(a, b), "{a} {b}");
    }
}
#[test]
fn damaged_tail_append_and_redaction() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nested/manager-log.jsonl");
    let mut e = event();
    e.note = Some("ghp_abcdefghijklmnopqrstuvwxyz".into());
    let stored = append(&path, &e).unwrap();
    assert_eq!(stored.note.as_deref(), Some("[REDACTED:GITHUB_TOKEN]"));
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"bad\n{\"schema_version\":").unwrap();
    let log = load(&path, None).unwrap();
    assert_eq!((log.events.len(), log.skipped_lines), (1, 2));
    append(&path, &event()).unwrap();
    let log = load(&path, Some("#1475")).unwrap();
    assert_eq!((log.events.len(), log.skipped_lines), (2, 2));
    assert!(fs::read(&path).unwrap().ends_with(b"\n"));
    assert_eq!(
        load(&tmp.path().join("missing"), None)
            .unwrap()
            .events
            .len(),
        0
    );
}
