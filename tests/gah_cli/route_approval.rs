use super::*;

#[test]
fn records_exact_grant_and_revoke() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let model = "nous-portal/z-ai/glm-5.2";

    for (command, expected_mode) in [
        ("grant", "paid_route_approval_grant"),
        ("revoke", "paid_route_approval_revoke"),
    ] {
        bin()
            .args([
                "route-approval",
                command,
                "--profile",
                "real",
                "ISSUE-42",
                "--backend",
                "opencode",
                "--model",
                model,
                "--config-path",
                cfg.to_str().unwrap(),
            ])
            .env("GAH_LEDGER_PATH", &ledger_path)
            .assert()
            .success()
            .stdout(predicate::str::contains(format!(
                "Paid route approval {}",
                if command == "grant" {
                    "granted"
                } else {
                    "revoked"
                }
            )));

        let line = fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_string();
        let entry: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(entry["mode"], expected_mode);
        assert_eq!(entry["work_id"], "ISSUE-42");
        assert_eq!(entry["effective_backend"], "opencode");
        assert_eq!(entry["effective_model"], model);
        assert!(entry["failure_class"].is_null());
    }
}

#[test]
fn records_exact_backend_instance() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");
    let mut config_text = fs::read_to_string(&cfg).unwrap();
    config_text.push_str(
        r#"
[profiles.real.routing.backend_instances.opencode-api]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/bin/sh"
account_label = "team-api"
auth_source_label = "env-openai-key"
"#,
    );
    fs::write(&cfg, config_text).unwrap();
    let ledger_path = tmp.path().join("ledger.jsonl");

    bin()
        .args([
            "route-approval",
            "grant",
            "--profile",
            "real",
            "ISSUE-42",
            "--backend",
            "opencode",
            "--instance",
            "opencode-api",
            "--model",
            "openai/gpt-5",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("[opencode-api]"));

    let line = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(line.lines().last().unwrap()).unwrap();
    assert_eq!(entry["usage"]["backend_instance"], "opencode-api");
}
