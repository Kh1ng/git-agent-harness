use super::*;
fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");
    let ledger = tmp.path().join("logs/ledger.jsonl");
    (tmp, cfg, ledger)
}
fn command(
    cfg: &std::path::Path,
    ledger: &std::path::Path,
    args: &[&str],
) -> IsolatedCommand<Command> {
    let mut cmd = bin();
    cmd.arg("manager-log")
        .args(args)
        .arg("--config-path")
        .arg(cfg)
        .env("GAH_LEDGER_PATH", ledger);
    cmd
}
fn show(cfg: &std::path::Path, ledger: &std::path::Path, args: &[&str]) -> Value {
    let out = command(cfg, ledger, args).assert().success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}
#[test]
fn manager_log_add_show_and_filter() {
    let (_tmp, cfg, ledger) = setup();
    let args = [
        "add",
        "--work-id",
        "TICKET-1475",
        "--phase",
        "verify",
        "--tier",
        "2",
        "--attempt",
        "3",
        "--backend",
        "codex",
        "--diagnosis",
        "failed_check",
        "--intervention",
        "rerun",
        "--tokens",
        "10",
        "--elapsed-seconds",
        "5",
        "--manager-rounds",
        "4",
        "--outcome",
        "passed",
        "--note",
        "done",
        "--owner",
        "manager",
    ];
    let out = command(&cfg, &ledger, &args).assert().success();
    let stored: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let log = show(&cfg, &ledger, &["show", "--json"]);
    assert_eq!(log["events"].as_array().unwrap().len(), 1);
    assert_eq!(log["events"][0], stored);
    for (key, value) in [
        ("work_id", "TICKET-1475"),
        ("phase", "verify"),
        ("backend", "codex"),
        ("diagnosis", "failed_check"),
        ("intervention", "rerun"),
        ("outcome", "passed"),
        ("note", "done"),
        ("owner", "manager"),
    ] {
        assert_eq!(stored[key], value);
    }
    for (key, n) in [
        ("tier", 2),
        ("attempt", 3),
        ("tokens", 10),
        ("elapsed_seconds", 5),
        ("manager_rounds", 4),
        ("schema_version", 1),
    ] {
        assert_eq!(stored[key], n);
    }
    time::OffsetDateTime::parse(
        stored["ts"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert_eq!(
        show(&cfg, &ledger, &["show", "--work-id", "#1475", "--json"])["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        show(&cfg, &ledger, &["show", "--work-id", "other", "--json"])["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn manager_log_summary() {
    let (_tmp, cfg, ledger) = setup();
    for args in [
        vec![
            "add",
            "--work-id",
            "TICKET-1",
            "--phase",
            "repair",
            "--attempt",
            "3",
            "--manager-rounds",
            "4",
            "--tokens",
            "10",
            "--elapsed-seconds",
            "5",
            "--outcome",
            "passed",
        ],
        vec!["add", "--work-id", "TICKET-2", "--phase", "research"],
        vec![
            "add",
            "--work-id",
            "#1",
            "--phase",
            "merge",
            "--attempt",
            "1",
            "--manager-rounds",
            "2",
            "--tokens",
            "20",
            "--elapsed-seconds",
            "7",
        ],
    ] {
        command(&cfg, &ledger, &args).assert().success();
    }
    let summary = show(&cfg, &ledger, &["show", "--summary", "--json"]);
    assert_eq!(summary["items"].as_array().unwrap().len(), 2);
    let i = &summary["items"][0];
    for (key, n) in [
        ("events", 2),
        ("attempts", 3),
        ("manager_rounds", 4),
        ("tokens", 30),
        ("elapsed_seconds", 12),
    ] {
        assert_eq!(i[key], n);
    }
    assert_eq!(i["work_id"], "TICKET-1");
    assert_eq!(i["last_outcome"], "passed");
    assert_eq!(i["last_phase"], "merge");
    assert_eq!(summary["items"][1]["attempts"], 0);
}
#[test]
fn manager_log_invalid_add_and_missing_show() {
    let (_tmp, cfg, ledger) = setup();
    assert!(show(&cfg, &ledger, &["show", "--json"])["events"]
        .as_array()
        .unwrap()
        .is_empty());
    let path = ledger.parent().unwrap().join("manager-log.jsonl");
    for (flag, value) in [
        ("--phase", "bogus"),
        ("--diagnosis", "bogus"),
        ("--tier", "5"),
        ("--attempt", "0"),
        ("--work-id", ""),
        ("--tokens", "-1"),
        ("--elapsed-seconds", "-1"),
        ("--manager-rounds", "-1"),
    ] {
        let mut args = vec!["add"];
        if flag != "--work-id" {
            args.extend(["--work-id", "TICKET-1"]);
        }
        if flag != "--phase" {
            args.extend(["--phase", "verify"]);
        }
        args.extend([flag, value]);
        command(&cfg, &ledger, &args).assert().failure();
        assert!(!path.exists());
    }
}
#[test]
fn manager_log_concurrent_adds() {
    let (_tmp, cfg, ledger) = setup();
    let exe = assert_cmd::cargo::cargo_bin!("gah");
    let mut children = vec![];
    for n in 0..8 {
        children.push(
            std::process::Command::new(exe)
                .args([
                    "manager-log",
                    "add",
                    "--work-id",
                    &format!("TICKET-{n}"),
                    "--phase",
                    "supervise",
                    "--config-path",
                ])
                .arg(&cfg)
                .env("GAH_LEDGER_PATH", &ledger)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let contents = fs::read_to_string(ledger.parent().unwrap().join("manager-log.jsonl")).unwrap();
    assert_eq!(contents.lines().count(), 8);
    for line in contents.lines() {
        let e: git_agent_harness::manager_log::Event = serde_json::from_str(line).unwrap();
        e.validate().unwrap();
    }
}
