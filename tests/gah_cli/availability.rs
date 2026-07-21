use super::*;

/// CLI-level smoke test for `gah availability`. Unit tests cover eligibility
/// and scope logic; this proves the command uses GAH_AVAILABILITY_PATH and
/// preserves the legacy human/JSON shapes end to end.
#[test]
fn availability_human_and_json_views() {
    let tmp = test_tempdir();
    let state_path = tmp.path().join("availability.json");
    fs::write(
        &state_path,
        r#"{"version":1,"records":[
            {"backend":"claude","status":"unavailable","reason":"quota_exhausted","observed_at":"2026-07-04T13:00:00Z","unavailable_until":"2099-01-01T00:00:00Z","source":"backend_error","last_error_summary":"quota exhausted"},
            {"backend":"codex","status":"available","reason":"unknown","observed_at":"2026-07-04T13:00:00Z","source":"manual"}
        ]}"#,
    )
    .unwrap();

    bin()
        .args(["availability"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("claude"))
        .stdout(predicate::str::contains("unavailable"))
        .stdout(predicate::str::contains("quota_exhausted"))
        .stdout(predicate::str::contains("codex"))
        .stdout(predicate::str::contains("available"));

    let out = bin()
        .args(["availability", "--json"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success();
    let parsed: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let rows = parsed.as_array().unwrap();
    let claude = rows.iter().find(|row| row["backend"] == "claude").unwrap();
    assert_eq!(claude["eligible"], false);
    assert_eq!(claude["reason"], "quota_exhausted");
    assert_eq!(claude["source"], "backend_error");
    let codex = rows.iter().find(|row| row["backend"] == "codex").unwrap();
    assert_eq!(codex["eligible"], true);
}

#[test]
fn availability_with_no_state_file_reports_eligible_by_default() {
    let tmp = test_tempdir();
    let state_path = tmp.path().join("does-not-exist.json");

    bin()
        .args(["availability"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("eligible by default"));

    bin()
        .args(["availability", "--json"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success()
        .stdout("[]\n");
}

#[test]
fn availability_clear_can_target_one_backend_instance() {
    let tmp = test_tempdir();
    let state_path = tmp.path().join("availability.json");
    fs::write(
        &state_path,
        r#"{"version":2,"records":[
            {"backend":"opencode","backend_instance":"account-a","model":"shared-model","status":"unavailable","reason":"authentication_error","observed_at":"2026-07-20T00:00:00Z","source":"backend_error"},
            {"backend":"opencode","backend_instance":"account-b","model":"shared-model","status":"unavailable","reason":"authentication_error","observed_at":"2026-07-20T00:00:00Z","source":"backend_error"}
        ]}"#,
    )
    .unwrap();

    bin()
        .args([
            "availability",
            "clear",
            "--backend",
            "opencode",
            "--backend-instance",
            "account-a",
            "--model",
            "shared-model",
        ])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success();

    let output = bin()
        .args(["availability", "--json"])
        .env("GAH_AVAILABILITY_PATH", &state_path)
        .assert()
        .success();
    let rows: Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    let rows = rows.as_array().unwrap();
    let first = rows
        .iter()
        .find(|row| row["backend_instance"] == "account-a")
        .unwrap();
    let second = rows
        .iter()
        .find(|row| row["backend_instance"] == "account-b")
        .unwrap();
    assert_eq!(first["eligible"], true);
    assert_eq!(second["eligible"], false);
}
