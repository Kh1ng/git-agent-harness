use super::*;

fn make_fake_github_pm_snapshot(dir: &std::path::Path) {
    make_fake_bin_with_body(
        dir,
        "gh",
        r#"#!/bin/sh
case "$*" in
  *"/issues?state=open"*) printf '[]\n' ;;
  *"/pulls?state=open"*) printf '[]\n' ;;
  *"search/issues"*) printf '{"incomplete_results":false,"items":[]}\n' ;;
  *) echo "unexpected gh invocation: $@" >&2; exit 1 ;;
esac
"#,
    );
}

#[test]
fn dispatch_pm_target_parses_structured_plan_and_writes_ticket() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\npm_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        "#!/bin/sh\nprintf '%s\n' '{\"title\":\"Plan\",\"summary\":\"Summary\",\"tickets\":[{\"key\":\"fix-auth\",\"title\":\"Fix auth\",\"objective\":\"Tighten auth checks\",\"task_class\":\"fix\",\"difficulty\":\"easy\",\"risk\":\"low\",\"execution_disposition\":\"autonomous\",\"recommended_routing\":{\"capability\":\"edit\",\"min_tier\":\"standard\"},\"duplicate_evidence\":[],\"affected_areas\":[\"auth\"],\"depends_on\":[],\"affected_files\":[\"src/auth.rs\"],\"acceptance_criteria\":[\"auth rejects invalid token\"],\"verification_commands\":[\"pytest tests/test_auth.py -x\"],\"uncovered_reason\":\"No open MR or ticket covers this auth edge case.\"}]}'\n",
    );
    make_fake_github_pm_snapshot(&fake_bin);

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "pm",
            "--target",
            "Plan auth work",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success()
        .stdout(predicate::str::contains("Created 1 ticket"));

    let tickets_dir = repo.join("docs/tickets");
    let entries: Vec<_> = fs::read_dir(&tickets_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(entries.iter().any(|name| name.contains("fix-auth")));
}

#[test]
fn dispatch_pm_skips_unavailable_preferred_backend() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\npm_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let claude_marker = tmp.path().join("claude-launched.txt");
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\ntouch '{}'\nprintf '%s\n' '{{\"title\":\"Wrong\",\"summary\":\"Wrong\",\"tickets\":[]}}'\n",
            claude_marker.display()
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/bin/sh\nprintf '%s\n' '{\"title\":\"Plan\",\"summary\":\"Summary\",\"tickets\":[{\"key\":\"fallback\",\"title\":\"Fallback ticket\",\"objective\":\"Handled by codex fallback\",\"task_class\":\"fix\",\"difficulty\":\"easy\",\"risk\":\"low\",\"execution_disposition\":\"autonomous\",\"recommended_routing\":{\"capability\":\"edit\",\"min_tier\":\"standard\"},\"duplicate_evidence\":[],\"affected_areas\":[\"ops\"],\"depends_on\":[],\"affected_files\":[\"docs/tickets/placeholder.md\"],\"acceptance_criteria\":[\"ticket exists\"],\"verification_commands\":[\"test -f docs/tickets\"],\"uncovered_reason\":\"No duplicate work found.\"}]}'\n",
    );
    make_fake_github_pm_snapshot(&fake_bin);

    let availability_path = tmp.path().join("availability.json");
    fs::write(
        &availability_path,
        "{\"version\":1,\"records\":[{\"backend\":\"claude\",\"status\":\"unavailable\",\"reason\":\"quota_exhausted\",\"observed_at\":\"2099-01-01T00:00:00Z\",\"unavailable_until\":\"2099-01-02T00:00:00Z\",\"source\":\"backend_error\"}]}",
    )
    .unwrap();
    let ledger_path = tmp.path().join("ledger.jsonl");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "pm",
            "--target",
            "Plan fallback work",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("Created 1 ticket"));

    assert!(!claude_marker.exists());
    let ledger = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "codex");
    assert_eq!(entry["fallback_used"], true);
    assert!(entry["routing_reason"]
        .as_str()
        .unwrap()
        .contains("quota_exhausted"));
}

#[test]
fn dispatch_pm_claude_session_limit_marks_model_and_reroutes_once() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\npm_backend = \"claude\"\npm_candidates = [{ backend = \"claude\", model = \"haiku\" }, { backend = \"codex\", model = \"gpt-5.4-mini\" }]\n",
        "",
    );

    let claude = FakeBackend::new(&tmp.path().join("claude-backend"), "claude");
    claude.install(Scenario::failure(1).with_stderr(include_str!(
        "../fixtures/quota-logs/claude_session_limit_tz_reset.txt"
    )));
    let codex = FakeBackend::new(&tmp.path().join("codex-backend"), "codex");
    codex.install(Scenario::success().with_stdout(
        "{\"title\":\"Plan\",\"summary\":\"Summary\",\"tickets\":[{\"key\":\"fallback\",\"title\":\"Fallback ticket\",\"objective\":\"Handled by reroute\",\"task_class\":\"fix\",\"difficulty\":\"easy\",\"risk\":\"low\",\"execution_disposition\":\"autonomous\",\"recommended_routing\":{\"capability\":\"edit\",\"min_tier\":\"standard\"},\"duplicate_evidence\":[],\"affected_areas\":[\"ops\"],\"depends_on\":[],\"affected_files\":[\"docs/tickets/placeholder.md\"],\"acceptance_criteria\":[\"ticket exists\"],\"verification_commands\":[\"test -d docs/tickets\"],\"uncovered_reason\":\"No duplicate work found.\"}]}",
    ));

    let github_bin = tmp.path().join("github-backend/bin");
    fs::create_dir_all(&github_bin).unwrap();
    make_fake_github_pm_snapshot(&github_bin);
    let path = format!(
        "{}:{}:{}:{}",
        github_bin.display(),
        codex.bin_dir().display(),
        claude.bin_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let availability_path = tmp.path().join("availability.json");
    let ledger_path = tmp.path().join("ledger.jsonl");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "pm",
            "--target",
            "Plan fallback work",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", path)
        .env("GAH_AVAILABILITY_PATH", &availability_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "PM rerouting: claude/haiku -> codex/gpt-5.4-mini (QuotaExhausted)",
        ));

    assert_eq!(claude.call_count(), 1);
    assert_eq!(codex.call_count(), 1);
    let availability: Value =
        serde_json::from_str(&fs::read_to_string(&availability_path).unwrap()).unwrap();
    let records = availability["records"].as_array().unwrap();
    assert_eq!(records[0]["backend"], "claude");
    assert_eq!(records[0]["model"], "haiku");
    assert_eq!(records[0]["reason"], "quota_exhausted");
    let ledger = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();
    assert_eq!(entry["effective_backend"], "codex");
    assert_eq!(entry["fallback_used"], true);
}
