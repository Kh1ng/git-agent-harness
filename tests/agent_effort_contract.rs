mod support;

#[test]
fn manual_worker_refuses_below_memory_floor_without_starting_backend_or_saving_effort() {
    use support::scenario::ScenarioHarness;
    let disk = support::test_tempdir();
    let mut harness = ScenarioHarness::new("github")
        .github_scenario("empty")
        .worker_scenario("success")
        .with_worktree_base(disk.path().join("worktrees"))
        .with_temp_dir(disk.path().to_path_buf())
        .with_config_append("\n[defaults.node_capacity]\nmemory_floor_mib = 2048\n");
    let ticket = harness.artifacts_dir.join("TICKET-7-memory.md");
    std::fs::write(
        &ticket,
        "# TICKET-7: Memory floor\n\nGoal: verify node admission.\n",
    )
    .unwrap();
    let pressure = harness.artifacts_dir.join("pressure.json");
    std::fs::write(
        &pressure,
        serde_json::json!({
            "memory_total_bytes": 8_u64 * 1024 * 1024 * 1024,
            "memory_available_bytes": 1024 * 1024 * 1024,
            "logical_cpus": 8, "load_one": 0.0,
            "memory_full_psi_avg10": 0.0, "cpu_some_psi_avg10": 0.0
        })
        .to_string(),
    )
    .unwrap();
    harness.node_pressure_fixture = Some(pressure);
    let result = harness
        .run_dispatch(&[
            "--mode",
            "improve",
            "--backend",
            "codex",
            "--model",
            "gpt-5",
            "--manual-worker",
            "--reasoning-effort",
            "high",
            "--target",
            ticket.to_str().unwrap(),
        ])
        .unwrap();
    assert_ne!(result.exit_code, Some(0), "{result:?}");
    assert!(
        result.stderr.contains("Manual worker did not start"),
        "{result:?}"
    );
    assert!(
        result
            .stderr
            .contains("node memory reserve would be crossed"),
        "{result:?}"
    );
    assert!(harness.worker_argv_for_call("codex", 1).is_empty());
    let saved = std::fs::read_to_string(&harness.config_path).unwrap();
    assert!(!saved.contains("model_reasoning_effort"));
    let claims: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            harness
                .config_path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("xdg-state/gah/work_claims.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(claims["claims"]
        .as_object()
        .unwrap()
        .values()
        .all(|claims| claims.as_array().unwrap().is_empty()));
}

#[test]
fn manual_worker_does_not_wait_for_or_interrupt_the_loops_profile_lock() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = dir.path().join("gah.toml");
    let mut cfg: git_agent_harness::config::GahConfig = toml::from_str(
        r#"
[profiles.repo]
display_name = "Repo"
repo_id = "repo"
provider = "github"
repo = "owner/repo"
local_path = "/tmp/repo"
artifact_root = "/tmp/artifacts"
default_target_branch = "main"
"#,
    )
    .unwrap();
    cfg.profiles.get_mut("repo").unwrap().artifact_root = dir.path().to_string_lossy().into_owned();
    std::fs::write(&config, toml::to_string(&cfg).unwrap()).unwrap();
    let before = std::fs::read_to_string(&config).unwrap();
    let loop_lock = git_agent_harness::controller::acquire_profile_lock("repo", &config).unwrap();
    support::gah_command()
        .args([
            "dispatch",
            "--profile",
            "repo",
            "--mode",
            "improve",
            "--backend",
            "codex",
            "--model",
            "gpt-6.1-sol",
            "--target",
            "ticket.md",
            "--dry-run",
            "--config-path",
        ])
        .arg(&config)
        .assert()
        .failure();
    support::gah_command()
        .args([
            "dispatch",
            "--profile",
            "repo",
            "--mode",
            "improve",
            "--backend",
            "codex",
            "--model",
            "gpt-6.1-sol",
            "--target",
            "ticket.md",
            "--dry-run",
            "--manual-worker",
            "--reasoning-effort",
            "high",
            "--config-path",
        ])
        .arg(&config)
        .assert()
        .success();
    assert!(git_agent_harness::controller::acquire_profile_lock("repo", &config).is_err());
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
    drop(loop_lock);
}

#[test]
fn manual_worker_refuses_work_the_loop_has_claimed_and_leaves_that_claim_alone() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = dir.path().join("gah.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[profiles.repo]
display_name = "Repo"
repo_id = "repo"
provider = "github"
repo = "owner/repo"
local_path = "/tmp/repo"
artifact_root = "{}"
default_target_branch = "main"
"#,
            dir.path().display()
        ),
    )
    .unwrap();
    let ticket = dir.path().join("TICKET-7-example.md");
    std::fs::write(&ticket, "# Example\n").unwrap();
    let claims = dir.path().join("work-claims.json");
    // What the loop writes before it starts #7.
    let mut state = git_agent_harness::work_claim::WorkClaimState::new();
    state.claim("repo@repo", "#7");
    let before = serde_json::to_string(&state).unwrap();
    std::fs::write(&claims, &before).unwrap();

    let output = support::gah_command()
        .env("GAH_CLAIM_STATE_PATH", &claims)
        .args([
            "dispatch",
            "--profile",
            "repo",
            "--mode",
            "improve",
            "--backend",
            "codex",
            "--model",
            "gpt-6.1-sol",
            "--manual-worker",
            "--target",
        ])
        .arg(&ticket)
        .arg("--config-path")
        .arg(&config)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("#7 is already being worked on by the loop or another worker"),
        "{stderr}"
    );
    // The refused run did not release the loop's claim on its way out.
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&claims).unwrap()).unwrap();
    assert_eq!(
        after,
        serde_json::from_str::<serde_json::Value>(&before).unwrap()
    );
}

#[test]
fn task_reasoning_round_trips_through_profile_cli_without_changing_other_args() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = dir.path().join("gah.toml");
    std::fs::write(
        &config,
        r#"
[profiles.repo]
display_name = "Repo"
repo_id = "repo"
provider = "github"
repo = "owner/repo"
local_path = "/tmp/repo"
artifact_root = "/tmp/artifacts"
default_target_branch = "main"
codex_args = ["--sandbox", "workspace-write", "-c", "model_reasoning_effort=\"low\""]
claude_args = ["--verbose"]
"#,
    )
    .unwrap();
    let set = |settings: &[&str]| {
        support::gah_command()
            .args(["profile", "set", "repo"])
            .args(settings)
            .arg("--config")
            .arg(&config)
            .assert()
    };
    set(&[
        "--agent-effort",
        "codex=high",
        "--agent-effort",
        "claude=max",
    ])
    .success();
    let output = support::gah_command()
        .args(["profile", "list", "--json", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(output.status.success());
    let profiles: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        profiles[0]["agent_reasoning_effort"],
        serde_json::json!({"codex":"high", "claude":"max"})
    );
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(
        saved["profiles"]["repo"]["codex_args"].as_array().unwrap()[0].as_str(),
        Some("--sandbox")
    );
    assert_eq!(
        saved["profiles"]["repo"]["claude_args"].as_array().unwrap()[0].as_str(),
        Some("--verbose")
    );
    let before = std::fs::read_to_string(&config).unwrap();
    set(&["--agent-effort", "claude=ultra"]).failure();
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
    set(&[
        "--agent-effort",
        "codex=default",
        "--agent-effort",
        "claude=default",
    ])
    .success();
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(
        saved["profiles"]["repo"]["claude_args"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        saved["profiles"]["repo"]["codex_args"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
