use super::*;
#[cfg(unix)]
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

fn info(id: &str) -> CredentialInfo {
    CredentialInfo {
        id: id.into(),
        provider: "nous".into(),
        kind: CredentialKind::ApiKey,
        account_label: "Personal account".into(),
        env_var: Some("NOUS_API_KEY".into()),
    }
}

#[test]
#[cfg(unix)]
fn private_rotation_and_listing_never_return_secret_material() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("credentials");
    save_at(&root, info("primary"), "synthetic-first").unwrap();
    save_at(&root, info("second"), "synthetic-second").unwrap();
    save_at(&root, info("primary"), "synthetic-renewed").unwrap();
    assert_eq!(
        read_at(&root, "primary").unwrap().secret,
        "synthetic-renewed"
    );
    assert_eq!(read_at(&root, "second").unwrap().secret, "synthetic-second");
    let summary = serde_json::to_string(&list_at(&root).unwrap()).unwrap();
    assert!(!summary.contains("synthetic"));
    assert_eq!(std::fs::metadata(&root).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        std::fs::metadata(root.join("primary.json")).unwrap().mode() & 0o777,
        0o600
    );
}

#[test]
#[cfg(unix)]
fn private_store_rejects_redirection_and_public_records() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("credentials");
    save_at(&root, info("primary"), "synthetic-key").unwrap();
    let target = dir.path().join("target");
    std::fs::write(&target, "unchanged").unwrap();
    symlink(&target, root.join("linked.json")).unwrap();
    assert!(read_at(&root, "linked").is_err());
    save_at(&root, info("linked"), "synthetic-new").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
    std::fs::set_permissions(
        root.join("primary.json"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(read_at(&root, "primary").is_err());
    let link = dir.path().join("directory-link");
    symlink(&root, &link).unwrap();
    assert!(save_at(&link, info("other"), "synthetic").is_err());
}

#[test]
fn bindings_fail_closed_and_cannot_change_runtime_configuration() {
    assert!(execution_env_with(info("primary"), "synthetic".into(), "openai").is_err());
    let mut dashboard = info("dashboard");
    dashboard.provider = "mistral".into();
    dashboard.kind = CredentialKind::MistralDashboard;
    dashboard.env_var = None;
    assert!(execution_env_with(dashboard, "synthetic".into(), "mistral").is_err());
    for name in [
        "HOME",
        "PATH",
        "LD_PRELOAD",
        "GAH_API_KEY",
        "DYLD_API_KEY",
        "NODE_OPTIONS",
        "bad-key",
        "NOUS_API_KEY=other",
    ] {
        let mut candidate = info("primary");
        candidate.env_var = Some(name.into());
        assert!(validate(&candidate).is_err(), "{name}");
    }
    for id in ["../outside", "a/b", "", "../../"] {
        assert!(validate(&info(id)).is_err());
    }
    assert!(validate(&info(&"a".repeat(64))).is_err());
    let mut google = info("google");
    google.provider = "google".into();
    google.env_var = Some("GOOGLE_API_KEY".into());
    assert!(execution_env_with(google, "synthetic".into(), "gemini").is_ok());
    assert!(validate_secret(&info("primary"), "value\nInjected: true").is_err());
    assert_eq!(
        execution_env_with(info("primary"), "synthetic".into(), "nous").unwrap(),
        vec![("NOUS_API_KEY".into(), "synthetic".into())]
    );
}
