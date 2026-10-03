use super::*;

fn cookie(name: &str, domain: &str, path: &str, value: &str) -> Cookie<'static> {
    Cookie::build((name.to_owned(), value.to_owned()))
        .domain(domain.to_owned())
        .path(path.to_owned())
        .secure(true)
        .http_only(true)
        .build()
}

#[test]
fn cookie_capture_preserves_http_only_and_parent_domains_but_rejects_other_scope() {
    let cookies = [
        cookie("session", "admin.mistral.ai", "/", "synthetic-session"),
        cookie("parent", ".mistral.ai", "/api", "synthetic-parent"),
        cookie("google", "accounts.google.com", "/", "other-login"),
        cookie("lookalike", "admin.mistral.ai.evil.test", "/", "other"),
        cookie("page", "admin.mistral.ai", "/subscription", "unrelated"),
        cookie(
            "similar-path",
            "admin.mistral.ai",
            "/api/local",
            "unrelated",
        ),
    ];
    let header = cookie_header(&cookies, unix_seconds()).unwrap();
    assert_eq!(header, "parent=synthetic-parent; session=synthetic-session");
    assert!(cookies[0].http_only().unwrap());
    assert!(matches_api_path("/api/local-trpc"));
    assert!(!matches_api_path("/api/local-trpc-evil"));
}

#[test]
fn cookie_header_rejects_injection_ambiguous_values_and_oversized_secrets() {
    for (name, value) in [
        ("session", "valid\nInjected: value"),
        ("bad;name", "value"),
        ("session", "value; another=cookie"),
        ("session", "value,other"),
    ] {
        assert!(cookie_header(
            &[cookie(name, "admin.mistral.ai", "/", value)],
            unix_seconds()
        )
        .is_err());
    }
    assert!(cookie_header(
        &[cookie(
            "session",
            "admin.mistral.ai",
            "/",
            &"x".repeat(32769)
        )],
        unix_seconds()
    )
    .is_err());
    assert!(cookie_header(&[], unix_seconds()).is_err());
}

#[test]
fn native_connection_cannot_claim_automatic_refresh_for_another_cookie_source() {
    let root = Path::new("/synthetic/config");
    assert!(default_cookie_source(root, None));
    assert!(default_cookie_source(
        root,
        Some(std::ffi::OsStr::new(
            "/synthetic/config/mistral-dashboard.cookie"
        ))
    ));
    for other in ["", "/other/account.cookie"] {
        assert!(!default_cookie_source(
            root,
            Some(std::ffi::OsStr::new(other))
        ));
    }
}

#[test]
fn provider_window_has_no_native_command_or_event_capability() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    for capability in config["app"]["security"]["capabilities"]
        .as_array()
        .unwrap()
    {
        assert!(!capability["windows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|window| window == WINDOW));
        if capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|permission| {
                permission == "allow-mistral-login-start"
                    || permission == "allow-mistral-login-finish"
            })
        {
            assert_eq!(capability["windows"], serde_json::json!(["dashboard"]));
            assert_eq!(capability["local"], true);
            assert!(capability.get("remote").is_none());
        }
    }
}

#[test]
fn automatic_finish_is_only_the_fixed_authenticated_usage_page() {
    for address in [
        "https://admin.mistral.ai/organization/usage",
        "https://admin.mistral.ai/organization/usage/",
    ] {
        assert!(authenticated_page(&address.parse().unwrap()));
    }
    for address in [
        "https://admin.mistral.ai/login",
        "https://admin.mistral.ai/subscription",
        "https://accounts.google.com/usage",
        "http://admin.mistral.ai/usage",
        "https://admin.mistral.ai.evil.test/usage",
        "https://user@admin.mistral.ai/usage",
    ] {
        assert!(!authenticated_page(&address.parse().unwrap()), "{address}");
    }
}

#[cfg(unix)]
struct TestRoot(PathBuf);
#[cfg(unix)]
impl TestRoot {
    fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt;
        static NEXT_ROOT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gah-native-mistral-test-{}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        Self(root)
    }
}
#[cfg(unix)]
impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn temporary_session_is_private_and_removed_without_replacing_an_existing_cookie() {
    use std::os::unix::fs::PermissionsExt;
    let root = TestRoot::new();
    let existing = root.0.join("mistral-dashboard.cookie");
    std::fs::write(&existing, "existing-session").unwrap();
    let path = {
        let check = PrivateCheck::new(&root.0, "private-synthetic-session").unwrap();
        let path = check.dir.join("cookie");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&check.dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        path
    };
    assert!(!path.exists());
    assert_eq!(
        std::fs::read_to_string(existing).unwrap(),
        "existing-session"
    );
}

#[cfg(unix)]
fn fake_gah(root: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("gah");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn only_verified_session_is_atomically_saved_and_supported_record_is_used() {
    use std::os::unix::fs::PermissionsExt;
    let root = TestRoot::new();
    let gah = fake_gah(
        &root.0,
        r#"
if [ "$2" = refresh ]; then
  [ "$3" = --backend ] && [ "$4" = mistral-dashboard ] && [ "$5" = --store ] || exit 2
  [ -n "$MISTRAL_DASHBOARD_COOKIE_FILE" ] || exit 3
  printf '%s' '{"backend":"mistral-dashboard","account_usage":{"account_id":"synthetic"},"check_error":null}' > "$6"
elif [ "$2" = record ]; then
  input=$(cat)
  case "$input" in *synthetic*) exit 0 ;; *) exit 4 ;; esac
else exit 5
fi
"#,
    );
    let reply = verify_and_save(&gah, &root.0, "synthetic-cookie");
    assert_eq!(reply.state, "connected");
    let saved = root.0.join("mistral-dashboard.cookie");
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "synthetic-cookie");
    assert_eq!(
        std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn rejected_session_preserves_existing_cookie_and_returns_no_provider_output() {
    let root = TestRoot::new();
    let saved = root.0.join("mistral-dashboard.cookie");
    std::fs::write(&saved, "existing-session").unwrap();
    let gah = fake_gah(
        &root.0,
        r#"printf '%s' '{"backend":"mistral-dashboard","check_error":"auth_required: expired"}' > "$6"
echo "synthetic-secret-must-not-reach-ui" >&2
exit 1"#,
    );
    let reply = verify_and_save(&gah, &root.0, "rejected-cookie");
    assert_eq!(reply.state, "pending");
    assert!(!reply.message.contains("synthetic-secret"));
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "existing-session");
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn legacy_cli_success_without_account_data_never_saves_a_session() {
    let root = TestRoot::new();
    let saved = root.0.join("mistral-dashboard.cookie");
    std::fs::write(&saved, "existing-session").unwrap();
    let gah = fake_gah(&root.0, "exit 0");
    assert_eq!(
        verify_and_save(&gah, &root.0, "unverified-cookie").state,
        "unavailable"
    );
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "existing-session");
}

#[cfg(unix)]
#[test]
fn atomic_verified_save_replaces_destination_symlink_without_touching_its_target() {
    use std::os::unix::fs::symlink;
    let root = TestRoot::new();
    let unrelated = root.0.join("unrelated");
    std::fs::write(&unrelated, "untouched").unwrap();
    let saved = root.0.join("mistral-dashboard.cookie");
    symlink(&unrelated, &saved).unwrap();
    let gah = fake_gah(
        &root.0,
        r#"if [ "$2" = refresh ]; then
printf '%s' '{"backend":"mistral-dashboard","account_usage":{"account_id":"synthetic"}}' > "$6"
fi
exit 0"#,
    );
    assert_eq!(
        verify_and_save(&gah, &root.0, "verified-cookie").state,
        "connected"
    );
    assert!(!std::fs::symlink_metadata(&saved)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "verified-cookie");
    assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "untouched");
}

#[cfg(unix)]
#[test]
fn cli_verification_deadline_kills_its_owned_process_group() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "sleep 60 & wait"]);
    let start = Instant::now();
    assert!(!run_gah(command, Duration::from_millis(100)));
    assert!(start.elapsed() < Duration::from_secs(5));
}
