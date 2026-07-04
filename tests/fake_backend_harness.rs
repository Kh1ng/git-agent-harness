//! Self-tests for the reusable hermetic fake-backend harness
//! (`tests/support/mod.rs`), proving each configurable capability actually
//! works before anything else (TICKET-066 quota parsing, TICKET-067/068
//! routing) is built on top of it.
//!
//! This file is deliberately separate from `tests/gah_cli.rs` — it tests
//! the harness itself, not `gah`.

mod support;

use std::process::Command;
use std::time::Instant;
use support::{FakeBackend, Scenario};
use tempfile::TempDir;

fn run(bin_dir: &std::path::Path, name: &str, args: &[&str]) -> std::process::Output {
    Command::new(bin_dir.join(name))
        .args(args)
        .env(
            "PATH",
            format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap()
}

#[test]
fn configurable_exit_code() {
    let tmp = TempDir::new().unwrap();
    let backend = FakeBackend::new(tmp.path(), "claude");
    backend.install(Scenario::failure(7));

    let out = run(backend.bin_dir(), "claude", &[]);
    assert_eq!(out.status.code(), Some(7));
}

#[test]
fn configurable_stdout_and_stderr() {
    let tmp = TempDir::new().unwrap();
    let backend = FakeBackend::new(tmp.path(), "codex");
    backend.install(
        Scenario::success()
            .with_stdout("hello from stdout")
            .with_stderr("hello from stderr"),
    );

    let out = run(backend.bin_dir(), "codex", &[]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello from stdout"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("hello from stderr"));
}

#[test]
fn argv_capture() {
    let tmp = TempDir::new().unwrap();
    let backend = FakeBackend::new(tmp.path(), "openhands");
    backend.install(Scenario::success());

    run(
        backend.bin_dir(),
        "openhands",
        &["--headless", "-t", "do the thing"],
    );

    assert_eq!(
        backend.argv_for_call(1),
        vec!["--headless", "-t", "do the thing"]
    );
}

#[test]
fn selected_environment_capture() {
    let tmp = TempDir::new().unwrap();
    let backend =
        FakeBackend::new(tmp.path(), "opencode").capture_env_vars(&["LLM_MODEL", "LLM_API_KEY"]);
    backend.install(Scenario::success());

    Command::new(backend.bin_dir().join("opencode"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                backend.bin_dir().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("LLM_MODEL", "opencode/test-model")
        .env("LLM_API_KEY", "secret")
        .env("SOMETHING_ELSE_NOT_CAPTURED", "ignored")
        .output()
        .unwrap();

    let env = backend.env_for_call(1);
    assert_eq!(
        env.get("LLM_MODEL").map(String::as_str),
        Some("opencode/test-model")
    );
    assert_eq!(env.get("LLM_API_KEY").map(String::as_str), Some("secret"));
    assert!(
        !env.contains_key("SOMETHING_ELSE_NOT_CAPTURED"),
        "only explicitly requested vars should be captured"
    );
}

#[test]
fn delay_actually_delays() {
    let tmp = TempDir::new().unwrap();
    let backend = FakeBackend::new(tmp.path(), "claude");
    backend.install(Scenario::success().with_delay_ms(150));

    let start = Instant::now();
    run(backend.bin_dir(), "claude", &[]);
    assert!(
        start.elapsed().as_millis() >= 150,
        "expected the fake backend to actually sleep"
    );
}

#[test]
fn deterministic_scenario_sequence_advances_per_call() {
    let tmp = TempDir::new().unwrap();
    let backend = FakeBackend::new(tmp.path(), "codex");
    backend.install_sequence(vec![
        Scenario::failure(1).with_stderr("quota exceeded"),
        Scenario::failure(1).with_stderr("still exceeded"),
        Scenario::success().with_stdout("done"),
    ]);

    let first = run(backend.bin_dir(), "codex", &[]);
    let second = run(backend.bin_dir(), "codex", &[]);
    let third = run(backend.bin_dir(), "codex", &[]);

    assert_eq!(first.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&first.stderr).contains("quota exceeded"));
    assert_eq!(second.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&second.stderr).contains("still exceeded"));
    assert_eq!(third.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&third.stdout).contains("done"));
    assert_eq!(backend.call_count(), 3);
}

#[test]
fn scenario_sequence_repeats_last_entry_beyond_its_length() {
    let tmp = TempDir::new().unwrap();
    let backend = FakeBackend::new(tmp.path(), "claude");
    backend.install_sequence(vec![Scenario::failure(1), Scenario::success()]);

    run(backend.bin_dir(), "claude", &[]); // call 1: fails
    let second = run(backend.bin_dir(), "claude", &[]); // call 2: succeeds
    let third = run(backend.bin_dir(), "claude", &[]); // call 3: beyond sequence

    assert_eq!(second.status.code(), Some(0));
    assert_eq!(
        third.status.code(),
        Some(0),
        "calls beyond the scripted sequence should repeat the last scenario"
    );
}

#[test]
fn independent_state_per_instance_of_the_same_backend_name() {
    let tmp = TempDir::new().unwrap();
    // Two independently configured "claude" instances -- e.g. representing
    // two separate subscription accounts in future availability/quota
    // routing tests, which must never share call counts or recordings.
    let account_a = FakeBackend::new(&tmp.path().join("account-a"), "claude");
    let account_b = FakeBackend::new(&tmp.path().join("account-b"), "claude");
    account_a.install(Scenario::failure(1).with_stderr("account A quota exhausted"));
    account_b.install(Scenario::success().with_stdout("account B is fine"));

    run(account_a.bin_dir(), "claude", &["run-1"]);
    run(account_a.bin_dir(), "claude", &["run-2"]);
    let b_out = run(account_b.bin_dir(), "claude", &["run-1"]);

    assert_eq!(account_a.call_count(), 2);
    assert_eq!(
        account_b.call_count(),
        1,
        "account B's call count must be unaffected by account A's calls"
    );
    assert_eq!(b_out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&b_out.stdout).contains("account B is fine"));
    assert_eq!(account_a.argv_for_call(1), vec!["run-1"]);
    assert_eq!(account_a.argv_for_call(2), vec!["run-2"]);
    // Account B never received account A's argv.
    assert_eq!(account_b.argv_for_call(2), Vec::<String>::new());
}

/// TICKET request: the harness must support OpenHands, OpenCode, Claude,
/// and Codex by name. (OpenCode is not a real `gah` runner backend today —
/// the harness is name-agnostic and supports faking it regardless.)
#[test]
fn supports_all_four_named_backends() {
    for name in ["openhands", "opencode", "claude", "codex"] {
        let tmp = TempDir::new().unwrap();
        let backend = FakeBackend::new(tmp.path(), name);
        backend.install(Scenario::success().with_stdout(format!("{name} ran")));

        let out = run(backend.bin_dir(), name, &[]);
        assert_eq!(out.status.code(), Some(0), "backend {name} should exit 0");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(&format!("{name} ran")),
            "backend {name} should produce its configured stdout"
        );
    }
}
