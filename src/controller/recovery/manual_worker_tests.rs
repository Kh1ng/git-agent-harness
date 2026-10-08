//! A manual worker runs outside the profile lock, so the loop's
//! reconciliation must not close out its run while the worker is alive.

use crate::config::{Defaults, GahConfig};

fn config(tmp: &std::path::Path) -> GahConfig {
    let mut cfg = GahConfig {
        context: Default::default(),
        defaults: Defaults {
            artifact_root: tmp.to_string_lossy().into_owned(),
            ..Default::default()
        },
        profiles: Default::default(),
    };
    let profile: crate::config::Profile = toml::from_str(
        r#"
display_name = "Real"
repo_id = "real"
provider = "github"
repo = "owner/real"
local_path = "/tmp/real"
artifact_root = "/tmp/real-artifacts"
default_target_branch = "main"
"#,
    )
    .unwrap();
    cfg.profiles.insert("real".into(), profile);
    cfg
}

fn start(cfg: &GahConfig, work_id: &str, run_id: &str) {
    crate::events::record_with_run_id(
        cfg,
        crate::events::EventType::DispatchStarted,
        Some("real"),
        Some(work_id),
        Some(run_id),
        "dispatch: manual",
    )
    .unwrap();
}

fn finished(cfg: &GahConfig, run_id: &str) -> bool {
    crate::events::read_events(cfg)
        .unwrap()
        .iter()
        .any(|event| {
            event.run_id.as_deref() == Some(run_id) && event.event_type == "dispatch_finished"
        })
}

/// A pid that belonged to a process which has already exited.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn reconciliation_skips_a_live_manual_worker_and_closes_a_dead_one() {
    let tmp = tempfile::tempdir().unwrap();
    let claims = tmp.path().join("claims.json");
    let _claims = crate::test_support::ClaimStateEnvGuard::set(&claims);
    let cfg = config(tmp.path());
    let scope = crate::work_claim::canonical_claim_scope("real", "real");

    // A dead manual worker's claim, left behind by a killed process.
    let host = hostname::get().unwrap().to_string_lossy().into_owned();
    let state = serde_json::json!({
        "version": 2u32,
        "claims": { scope.clone(): [{
            "work_id": "#501",
            "pid": dead_pid(),
            "hostname": host,
            "claimed_at": "2026-07-14T00:00:00Z",
            "manual_worker": true
        }]}
    });
    std::fs::write(&claims, serde_json::to_string(&state).unwrap()).unwrap();
    // A manual worker in flight in this (live) process.
    assert!(crate::work_claim::try_claim_manual_work(&scope, "#500").unwrap());
    start(&cfg, "#500", "manual-live");
    start(&cfg, "#501", "manual-dead");

    let mut entries = crate::ledger::read_entries(&cfg).unwrap();
    assert_eq!(
        super::reconcile_abandoned_dispatches(&cfg, "real", &mut entries).unwrap(),
        1
    );
    assert!(!finished(&cfg, "manual-live"));
    assert!(finished(&cfg, "manual-dead"));
    assert!(!entries
        .iter()
        .any(|entry| entry.session_id.as_deref() == Some("manual-live")));

    // Once the worker is gone, its open run is abandoned like any other.
    crate::work_claim::release_owned_work(&scope, "#500").unwrap();
    assert_eq!(
        super::reconcile_abandoned_dispatches(&cfg, "real", &mut entries).unwrap(),
        1
    );
    assert!(finished(&cfg, "manual-live"));
}
