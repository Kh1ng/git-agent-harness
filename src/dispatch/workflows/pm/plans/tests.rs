use super::*;
use crate::config::RoutingPolicy;
use crate::dispatch::test_util::{gah_config_with_ledger, profile};
use serde_json::Value;

fn fixture(provider: &str, root: &Path) -> (GahConfig, String, Value) {
    let mut cfg = gah_config_with_ledger(root, RoutingPolicy::default());
    let value: Value = serde_json::from_str(if provider == "github" {
        include_str!("../../../../../packages/contracts/src/fixtures/pm-plan-github.json")
    } else {
        include_str!("../../../../../packages/contracts/src/fixtures/pm-plan-gitlab.json")
    })
    .unwrap();
    let name = value["profile"].as_str().unwrap().to_string();
    let mut configured = profile(root);
    configured.display_name = "PM project".into();
    configured.provider = provider.into();
    configured.repo = format!("owner/{provider}");
    configured.artifact_root = root.join(&name).display().to_string();
    let session = Path::new(&configured.artifact_root).join("sessions/plan-1");
    fs::create_dir_all(&session).unwrap();
    fs::write(
        session.join("pm-plan-v1.json"),
        serde_json::to_vec(&value["artifact"]).unwrap(),
    )
    .unwrap();
    if provider == "gitlab" {
        fs::write(
            session.join("pm-plan-v1.json.publication-v1.json"),
            serde_json::to_vec(&value["publication"]).unwrap(),
        )
        .unwrap();
    }
    cfg.profiles.insert(name.clone(), configured);
    (cfg, name, value)
}

#[test]
fn shared_contract_fixtures_preserve_provider_graph_and_partial_publication() {
    for provider in ["github", "gitlab"] {
        let root = tempfile::tempdir().unwrap();
        let (cfg, name, expected) = fixture(provider, root.path());
        let detail = show(&cfg, &name, "plan-1").unwrap();
        let mut actual = serde_json::to_value(detail).unwrap();
        actual["generated_at"] = expected["generated_at"].clone();
        actual["updated_at"] = expected["updated_at"].clone();
        assert_eq!(actual, expected);
        let listed = list(&cfg, &name, None, 1).unwrap();
        assert_eq!(listed.plans.len(), 1);
        assert!(listed.errors.is_empty());
        assert_eq!(
            listed.plans[0].publication_status,
            expected["publication"]["status"]
        );
        assert!(list(&cfg, &name, Some("plan-1"), 1)
            .unwrap()
            .plans
            .is_empty());
    }
}

#[test]
fn scope_rejects_unknown_profiles_paths_symlinks_and_foreign_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, name, _) = fixture("github", root.path());
    for id in ["../plan-1", "/tmp/plan-1", "..", "a/b", "a\\b", "%2e%2e"] {
        assert!(show(&cfg, &name, id).is_err(), "{id}");
    }
    assert!(show(&cfg, "unknown", "plan-1").is_err());
    let mut other = cfg.profiles[&name].clone();
    other.artifact_root = root.path().join("other").display().to_string();
    cfg.profiles.insert("other".into(), other);
    assert!(show(&cfg, "other", "plan-1").is_err());
    let path = plan_path(&cfg, &name, "plan-1").unwrap();
    #[cfg(unix)]
    {
        let session = path.parent().unwrap();
        std::os::unix::fs::symlink(session, session.parent().unwrap().join("linked")).unwrap();
        assert!(show(&cfg, &name, "linked").is_err());
        let original = session.join("original.json");
        fs::rename(&path, &original).unwrap();
        std::os::unix::fs::symlink(&original, &path).unwrap();
        assert!(show(&cfg, &name, "plan-1").is_err());
        fs::remove_file(&path).unwrap();
        fs::rename(original, &path).unwrap();
        let state = publication_state_path(&path).unwrap();
        std::os::unix::fs::symlink(&path, &state).unwrap();
        assert!(show(&cfg, &name, "plan-1").is_err());
        fs::remove_file(state).unwrap();
        let sessions = session.parent().unwrap();
        let outside = root.path().join("foreign-sessions");
        fs::rename(sessions, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, sessions).unwrap();
        assert!(show(&cfg, &name, "plan-1").is_err());
        assert!(list(&cfg, &name, None, 25).is_err());
        fs::remove_file(sessions).unwrap();
        fs::rename(outside, sessions).unwrap();
    }
    cfg.profiles.get_mut(&name).unwrap().repo = "other/repo".into();
    assert!(show(&cfg, &name, "plan-1").is_err());
    let listed = list(&cfg, &name, None, 25).unwrap();
    assert!(listed.plans.is_empty());
    assert_eq!(listed.errors.len(), 1);
}

#[test]
fn publication_requires_current_approval_and_keeps_backend_policy_authoritative() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, name, _) = fixture("github", root.path());
    let fingerprint = show(&cfg, &name, "plan-1")
        .unwrap()
        .publication
        .plan_fingerprint;
    assert!(publish(&cfg, &name, "plan-1", None, false)
        .err()
        .unwrap()
        .to_string()
        .contains("fingerprint"));
    assert!(publish(&cfg, &name, "plan-1", Some("stale"), false).is_err());
    // Approval must also be checked against the publisher's own read, not just the API projection.
    let artifact_path = plan_path(&cfg, &name, "plan-1").unwrap();
    let mut edited: Value = serde_json::from_slice(&fs::read(&artifact_path).unwrap()).unwrap();
    edited["plan"]["summary"] = "Changed after review".into();
    fs::write(&artifact_path, serde_json::to_vec(&edited).unwrap()).unwrap();
    let changed =
        crate::dispatch::publish_pm_plan(&cfg, &name, &artifact_path, false, Some(&fingerprint));
    assert!(changed
        .err()
        .unwrap()
        .to_string()
        .contains("changed after approval"));
    let fingerprint = show(&cfg, &name, "plan-1")
        .unwrap()
        .publication
        .plan_fingerprint;
    let policy = root.path().join("policy.toml");
    fs::write(&policy, "[repo]\ntrust_mode = 'read_only'\nallow_push = false\nallow_draft_pr = false\nallow_provider_mutation = false\nallow_issue_write = false\nallow_project_write = false\n").unwrap();
    cfg.profiles.get_mut(&name).unwrap().policy_path = Some(policy.display().to_string());
    let result = publish(&cfg, &name, "plan-1", Some(&fingerprint), false).unwrap();
    assert!(!result.success);
    assert!(result.error.unwrap().contains("POLICY BLOCKED"));
    assert_eq!(result.plan.publication.status, "planned");
    assert!(!result.plan.failures.is_empty());
    assert!(result.plan.failures[0].message.contains("POLICY BLOCKED"));
}
