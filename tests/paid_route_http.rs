//! Exercise the HTTP approval adapter against a real CLI and isolated ledger.
//! No backend is launched and no provider/notification command is configured.
use git_agent_harness::{config, ledger};
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn paid_route_http_controls_round_trip_real_ledger() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    if !root.join("node_modules/tsx").exists() {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI must install Node dependencies before the real HTTP test"
        );
        eprintln!("Run npm ci to enable the paid-route HTTP/CLI test");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config.toml");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let location = tmp.path().display().to_string();
    fs::write(
        &cfg_path,
        format!(
            r#"
[defaults]
artifact_root = "{location}"
worktree_base = "{location}/worktrees"
llm_base_url = "http://127.0.0.1:1"
llm_model_local = "unused"
llm_model_cloud = "unused"
[profiles.real]
display_name = "Fixture"
repo_id = "real"
provider = "github"
repo = "test/fixture"
local_path = "{location}"
artifact_root = "{location}"
default_target_branch = "main"
[profiles.real.routing.backend_instances.paid-a]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/bin/true"
account_label = "test-a"
auth_source_label = "unused"
[profiles.real.routing.backend_instances.paid-b]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/bin/true"
account_label = "test-b"
auth_source_label = "unused"
"#
        ),
    )
    .unwrap();
    let cfg = config::load(cfg_path.to_str()).unwrap();
    let profile = config::get_profile(&cfg, "real").unwrap();
    let mut gate = ledger::LedgerEntry::new("real", profile, "auto", "fix", "#822", None, None);
    gate.work_id = Some("#822".into());
    gate.human_required = true;
    gate.human_required_reason_code = Some("policy_approval".into());
    gate.failure_class = Some("human_blocked".into());
    gate.routing_diagnostics = Some(ledger::RoutingDiagnostics {
        candidates: ["paid-a", "paid-b"]
            .into_iter()
            .map(|instance| ledger::RoutingCandidateDiagnostic {
                backend: "opencode".into(),
                backend_instance: Some(instance.into()),
                model: Some("provider/model".into()),
                skip_reason: Some("operator_approval_required".into()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    fs::write(
        &ledger_path,
        format!("{}\n", serde_json::to_string(&gate).unwrap()),
    )
    .unwrap();
    let output = Command::new("node")
        .args([
            "--import",
            "tsx",
            "--test",
            "apps/server/src/paidRouteApprovals.test.ts",
        ])
        .current_dir(root)
        .env("GAH_REAL_PAID_ROUTE_TEST", "1")
        .env("GAH_BINARY", env!("CARGO_BIN_EXE_gah"))
        .env("GAH_CONFIG_PATH", &cfg_path)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CONFIG_HOME", tmp.path().join("xdg-config"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
