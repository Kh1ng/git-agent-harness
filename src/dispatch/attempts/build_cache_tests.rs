pub(super) fn acquire_cargo_target(
    tmp: &tempfile::TempDir,
) -> crate::build_cache::ScopedCargoTarget {
    crate::build_cache::ScopedCargoTarget::acquire(tmp.path().to_str().unwrap()).unwrap()
}

#[test]
fn cargo_target_environment_reaches_backend_and_validation_commands() {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let backend_capture = tmp.path().join("backend-env.txt");
    let validation_capture = tmp.path().join("validation-env.txt");
    let fake_vibe = bin_dir.join("vibe");
    let fake_sccache = bin_dir.join("sccache");
    std::fs::write(
        &fake_vibe,
        format!(
            "#!/bin/sh\nprintf '%s\\n%s\\n' \"$CARGO_TARGET_DIR\" \"$RUSTC_WRAPPER\" > {}\n",
            backend_capture.display()
        ),
    )
    .unwrap();
    std::fs::write(&fake_sccache, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    for path in [&fake_vibe, &fake_sccache] {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
    let _path_guard = crate::test_support::PathGuard::set(&bin_dir);

    let cargo_target = acquire_cargo_target(&tmp);
    let mut profile = crate::dispatch::test_util::profile(tmp.path());
    profile.vibe_path = Some(fake_vibe.display().to_string());
    let cfg = crate::dispatch::test_util::gah_config_with_ledger(
        tmp.path(),
        crate::config::RoutingPolicy::default(),
    );
    let session_dir = tmp.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    super::run_backend(
        &cfg,
        "test",
        "vibe",
        &profile,
        tmp.path(),
        "test",
        &session_dir,
        &cargo_target,
        &crate::runner::LlmConfig {
            base_url: String::new(),
            api_key: String::new(),
            model: "unused".into(),
        },
        None,
        None,
        None,
        None,
    )
    .unwrap();
    crate::validation_runner::validate(
        &[format!(
            "printf '%s\\n%s\\n' \"$CARGO_TARGET_DIR\" \"$RUSTC_WRAPPER\" > {}",
            validation_capture.display()
        )],
        tmp.path(),
        &cargo_target.environment(),
        std::time::Duration::from_secs(30),
    )
    .unwrap();

    let expected = format!(
        "{}\n{}\n",
        cargo_target.path().display(),
        fake_sccache.display()
    );
    assert_eq!(std::fs::read_to_string(backend_capture).unwrap(), expected);
    assert_eq!(
        std::fs::read_to_string(validation_capture).unwrap(),
        expected
    );
}
