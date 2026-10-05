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
    let previous_revision = read_at(&root, "primary").unwrap().revision;
    save_at(&root, info("second"), "synthetic-second").unwrap();
    save_at(&root, info("primary"), "synthetic-renewed").unwrap();
    assert_ne!(
        read_at(&root, "primary").unwrap().revision,
        previous_revision
    );
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
    assert!(validate_value(&info("primary"), "value\nInjected: true").is_err());
    assert_eq!(
        execution_env_with(info("primary"), "synthetic".into(), "nous").unwrap(),
        vec![("NOUS_API_KEY".into(), "synthetic".into())]
    );
}

#[test]
fn claude_subscription_is_private_and_only_binds_the_claude_runner() {
    let mut source = info("subscription");
    source.provider = "claude".into();
    source.kind = CredentialKind::ClaudeSubscription;
    source.env_var = None;
    let dir = tempfile::tempdir().unwrap();
    let saved = save_at(
        &dir.path().join("credentials"),
        source,
        "synthetic-subscription-token",
    )
    .unwrap();
    assert_eq!(saved.provider, "anthropic");
    assert_eq!(saved.env_var, None);
    assert_eq!(
        execution_env_with(
            saved.clone(),
            "synthetic-subscription-token".into(),
            "claude"
        )
        .unwrap(),
        vec![(
            "CLAUDE_CODE_OAUTH_TOKEN".into(),
            "synthetic-subscription-token".into()
        )]
    );
    assert!(execution_env_with(
        saved.clone(),
        "synthetic-subscription-token".into(),
        "anthropic"
    )
    .is_err());
    let mut invalid = saved;
    invalid.env_var = Some("ANTHROPIC_API_KEY".into());
    assert!(validate(&invalid).is_err());
}

#[test]
#[cfg(unix)]
fn private_record_cannot_return_its_value_as_display_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("credentials");
    save_at(&root, info("primary"), "synthetic-value").unwrap();
    let path = root.join("primary.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["info"]["account_label"] = serde_json::json!("Account synthetic-value");
    std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    let error = list_at(&root).unwrap_err();
    assert!(!format!("{error:#}").contains("synthetic-value"));
}

fn mistral_login_info(id: &str) -> CredentialInfo {
    CredentialInfo {
        id: id.into(),
        provider: "mistral".into(),
        kind: CredentialKind::MistralLogin,
        account_label: "Mistral console".into(),
        env_var: None,
    }
}

#[test]
#[cfg(unix)]
fn mistral_login_session_is_cached_per_revision_and_never_executes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("credentials");
    let login = r#"{"email":"owner@example.com","password":"synthetic-pass"}"#;
    for invalid in [
        "synthetic-pass",
        r#"{"email":"owner@example.com"}"#,
        r#"{"email":"not-an-email","password":"x"}"#,
        r#"{"email":"owner@example.com","password":"x","extra":1}"#,
    ] {
        assert!(
            save_at(&root, mistral_login_info("console"), invalid).is_err(),
            "{invalid}"
        );
    }
    let mut with_env = mistral_login_info("console");
    with_env.env_var = Some("MISTRAL_API_KEY".into());
    assert!(save_at(&root, with_env, login).is_err());

    save_at(&root, mistral_login_info("console"), login).unwrap();
    let first = read_at(&root, "console").unwrap();
    cache_session_at(&root, "console", &first.revision, "ory_session_a=1").unwrap();
    let cached = read_at(&root, "console").unwrap();
    assert_eq!(cached.revision, first.revision, "caching is not a rotation");
    assert_eq!(cached.session.as_deref(), Some("ory_session_a=1"));
    assert!(!serde_json::to_string(&list_at(&root).unwrap())
        .unwrap()
        .contains("synthetic"));

    // A new password drops the old session and rejects a stale publication.
    save_at(&root, mistral_login_info("console"), login).unwrap();
    assert_eq!(read_at(&root, "console").unwrap().session, None);
    assert!(cache_session_at(&root, "console", &first.revision, "ory_session_b=2").is_err());
    assert_eq!(read_at(&root, "console").unwrap().session, None);

    assert!(execution_env_with(mistral_login_info("console"), login.into(), "mistral").is_err());
    assert!(execution_env_with(mistral_login_info("console"), login.into(), "vibe").is_err());
}

#[test]
fn replacing_api_key_with_subscription_reclassifies_existing_paid_route() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("credentials");
    let mut source = info("account");
    source.provider = "anthropic".into();
    source.env_var = Some("ANTHROPIC_API_KEY".into());
    save_at(&root, source.clone(), "synthetic-api-key").unwrap();
    let mut stored = crate::config::RoutingPolicy::default();
    stored.backend_instances.insert(
        "claude-account".into(),
        crate::config::BackendInstanceConfig {
            runner_kind: "claude".into(),
            credential_id: Some("account".into()),
            ..Default::default()
        },
    );
    stored.improve_candidates = Some(vec![crate::config::CandidateConfig {
        backend: "claude".into(),
        instance: Some("claude-account".into()),
        requires_approval: true,
        marginal_cost_usd: Some(0.5),
        ..Default::default()
    }]);
    let resolve = || {
        let mut routing = stored.clone();
        routing.normalize_subscription_candidates(|id| {
            read_at(&root, id).unwrap().info.kind == CredentialKind::ClaudeSubscription
        });
        routing.improve_candidates.unwrap().remove(0)
    };
    assert!(resolve().requires_approval);
    source.kind = CredentialKind::ClaudeSubscription;
    source.env_var = None;
    save_at(&root, source, "synthetic-subscription-token").unwrap();
    let candidate = resolve();
    assert!(candidate.included_in_quota);
    assert!(!candidate.requires_approval);
    assert_eq!(candidate.marginal_cost_usd, None);
}
