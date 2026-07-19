// Parser / help-only CLI tests (ticket #406).
//
// These tests exercise the Clap parser definitions in `git_agent_harness::cli::args`
// directly (and the `--help` surface) without the full integration harness. They
// are declared as a submodule of the `gah_cli` integration test target; see
// `tests/gah_cli.rs`.

use clap::Parser;
use git_agent_harness::cli::args::Cli;
use predicates::prelude::*;

/// The top-level help text advertises the tool's identity unchanged.
#[test]
fn help_works() {
    super::bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("git agent harness"));
}

/// `gah --help` enumerates every top-level subcommand so operators can
/// discover the full surface.
#[test]
fn top_level_help_lists_all_subcommands() {
    super::bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("availability"))
        .stdout(predicate::str::contains("candidates"))
        .stdout(predicate::str::contains("price-guard"))
        .stdout(predicate::str::contains("policy-check"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("prune"))
        .stdout(predicate::str::contains("ledger"))
        .stdout(predicate::str::contains("hold"))
        .stdout(predicate::str::contains("route-approval"))
        .stdout(predicate::str::contains("loop"))
        .stdout(predicate::str::contains("events"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("sync"))
        .stdout(predicate::str::contains("dispatch"))
        .stdout(predicate::str::contains("tui"))
        .stdout(predicate::str::contains("profile"))
        .stdout(predicate::str::contains("report"))
        .stdout(predicate::str::contains("server"))
        .stdout(predicate::str::contains("telemetry"))
        .stdout(predicate::str::contains("quota"))
        .stdout(predicate::str::contains("claims"));
}

/// The parser rejects an unknown top-level subcommand (no silent default).
#[test]
fn unknown_top_level_subcommand_fails() {
    let err = Cli::try_parse_from(["gah", "definitely-not-a-command"]);
    assert!(err.is_err(), "unknown subcommand must be rejected");
}

/// `dispatch` requires its mandatory `--profile` and `--mode` options, so the
/// parser rejects a bare `dispatch` invocation.
#[test]
fn dispatch_requires_profile_and_mode() {
    let err = Cli::try_parse_from(["gah", "dispatch"]);
    assert!(err.is_err(), "dispatch without --profile/--mode must fail");
}

/// `dispatch --mode` accepts a known free-string mode without erroring at the
/// parse layer (routing validation happens later).
#[test]
fn dispatch_parses_mode_and_profile() {
    let parsed = Cli::try_parse_from(["gah", "dispatch", "--profile", "real", "--mode", "improve"]);
    assert!(parsed.is_ok(), "valid dispatch args must parse");
}

#[test]
fn profile_add_set_and_clear_preserve_max_open_managed_mrs() {
    let tmp = super::test_tempdir();
    let config_path = tmp.path().join("config.toml");
    let repo_path = tmp.path().join("repo");
    let artifact_root = tmp.path().join("artifacts");
    std::fs::write(&config_path, "").unwrap();

    super::bin()
        .args([
            "profile",
            "add",
            "test",
            "--display-name",
            "Test",
            "--repo-id",
            "owner/repo",
            "--provider",
            "github",
            "--repo",
            "owner/repo",
            "--local-path",
            repo_path.to_str().unwrap(),
            "--artifact-root",
            artifact_root.to_str().unwrap(),
            "--max-open-managed-mrs",
            "7",
            "--config-path",
            config_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let cfg = git_agent_harness::config::load(Some(config_path.to_str().unwrap())).unwrap();
    assert_eq!(cfg.profiles["test"].max_open_managed_mrs, Some(7));

    super::bin()
        .args([
            "profile",
            "set",
            "test",
            "--max-open-managed-mrs",
            "3",
            "--config-path",
            config_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let cfg = git_agent_harness::config::load(Some(config_path.to_str().unwrap())).unwrap();
    assert_eq!(cfg.profiles["test"].max_open_managed_mrs, Some(3));

    super::bin()
        .args([
            "profile",
            "set",
            "test",
            "--clear",
            "max_open_managed_mrs",
            "--config-path",
            config_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let cfg = git_agent_harness::config::load(Some(config_path.to_str().unwrap())).unwrap();
    assert_eq!(cfg.profiles["test"].max_open_managed_mrs, None);
}

/// `--help` on a subcommand resolves without touching any handler logic.
#[test]
fn subcommand_help_resolves() {
    for sub in [
        "availability",
        "candidates",
        "price-guard",
        "policy-check",
        "doctor",
        "update",
        "init",
        "prune",
        "ledger",
        "hold",
        "route-approval",
        "loop",
        "events",
        "status",
        "sync",
        "dispatch",
        "tui",
        "profile",
        "report",
        "server",
        "telemetry",
        "quota",
        "claims",
    ] {
        // `--help` makes Clap print help and exit; `try_parse_from` surfaces
        // that as an `Error` (carrying an exit code), not an `unknown
        // subcommand` parse failure. A genuinely unrecognized subcommand would
        // instead error with an "unknown" kind, so reject only that case.
        let err = Cli::try_parse_from(["gah", sub, "--help"]).err();
        let is_unknown = err
            .map(|e| format!("{e:?}"))
            .map(|s| s.contains("UnknownArgument") || s.contains("unrecognized"))
            .unwrap_or(false);
        assert!(
            !is_unknown,
            "subcommand '{sub}' must be recognized by the parser"
        );
    }
}
