use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::tempdir;

fn bin() -> Command {
    static COMMAND_COUNTER: AtomicU64 = AtomicU64::new(0);
    let invocation_id = COMMAND_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut cmd = Command::cargo_bin("gah").unwrap();
    cmd.env(
        "XDG_STATE_HOME",
        std::env::temp_dir().join(format!(
            "gah-cli-test-state-{}-{invocation_id}",
            std::process::id()
        )),
    );
    cmd.env(
        "GAH_AVAILABILITY_PATH",
        "/nonexistent-availability-path.json",
    );
    cmd.env(
        "GAH_VALIDATION_CHECK_PATH",
        std::env::temp_dir().join(format!(
            "gah-cli-test-validation-{}-{invocation_id}.json",
            std::process::id(),
        )),
    );
    cmd
}

fn make_fake_bin_with_body(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}

fn prepend_path(dir: &std::path::Path) -> String {
    let old = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), old)
}

#[test]
fn claims_profile_and_status_share_canonical_scope() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join("docs").join("tickets")).unwrap();
    fs::write(
        repo.join("docs").join("tickets").join("TICKET-436.md"),
        "# TICKET-436: canonical scope regression\n",
    )
    .unwrap();

    let cfg = tmp.path().join("gah-config.toml");
    fs::write(
        &cfg,
        format!(
            r#"
[defaults]
artifact_root = "{artifact_root}"
worktree_base = "{artifact_root}"
llm_base_url  = "http://localhost:4000"
llm_model_local = "local/test"
llm_model_cloud = "cloud/test"

[profiles.gah]
display_name          = "Test Gah"
repo_id               = "gah"
provider              = "github"
repo                  = "owner/gah"
local_path            = "{local_path}"
artifact_root         = "{artifact_root}/profiles/gah"
default_target_branch = "main"
"#,
            artifact_root = tmp.path().display(),
            local_path = repo.display()
        ),
    )
    .unwrap();

    let claim_state = tmp.path().join("claims.json");
    let state = serde_json::json!({
        "version": 2u32,
        "claims": {
            "gah@gah": [
                {
                    "work_id": "TICKET-436",
                    "pid": std::process::id(),
                    "hostname": "cli-host",
                    "claimed_at": "2026-07-14T00:00:00Z"
                }
            ]
        }
    });
    fs::write(&claim_state, serde_json::to_string(&state).unwrap()).unwrap();

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then printf '%s\\n' '[]'; fi\nexit 0\n",
    );

    let claims_by_profile_out = bin()
        .current_dir(&repo)
        .env("GAH_CLAIM_STATE_PATH", &claim_state)
        .env("PATH", prepend_path(&fake_bin))
        .args([
            "claims",
            "list",
            "--json",
            "--profile",
            "gah",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let claims_by_profile: Value = serde_json::from_slice(&claims_by_profile_out).unwrap();
    let claims_by_profile = claims_by_profile.as_array().unwrap();
    assert_eq!(claims_by_profile.len(), 1);
    assert_eq!(claims_by_profile[0]["work_id"], "TICKET-436");

    let claims_all_out = bin()
        .current_dir(&repo)
        .env("GAH_CLAIM_STATE_PATH", &claim_state)
        .env("PATH", prepend_path(&fake_bin))
        .args([
            "claims",
            "list",
            "--json",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let claims_all: Value = serde_json::from_slice(&claims_all_out).unwrap();
    let claims_all = claims_all.as_array().unwrap();
    let normalized = claims_all
        .iter()
        .find(|entry| entry["profile"] == "gah@gah")
        .expect("claims list for all profiles must include normalized scope");
    assert_eq!(normalized["work_id"], "TICKET-436");

    let status_out = bin()
        .current_dir(&repo)
        .env("GAH_CLAIM_STATE_PATH", &claim_state)
        .env("GAH_LEDGER_PATH", tmp.path().join("ledger.jsonl"))
        .env("PATH", prepend_path(&fake_bin))
        .args([
            "status",
            "--json",
            "--profile",
            "gah",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: Value = serde_json::from_slice(&status_out).unwrap();
    let tickets = status["available_tickets"].as_array().unwrap();
    let ticket = tickets
        .iter()
        .find(|item| item["work_id"] == "TICKET-436")
        .expect("status should include canonical test ticket");
    assert_eq!(ticket["has_active_claim"], true);

    let active_claims = status["active_claims"].as_array().unwrap();
    let active = active_claims
        .iter()
        .find(|item| item["work_id"] == "TICKET-436")
        .expect("status should include active claim snapshot");
    assert_eq!(active["scope"], "gah@gah");
}
