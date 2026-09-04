pub(super) fn acquire_cargo_target(
    tmp: &tempfile::TempDir,
) -> crate::build_cache::ScopedCargoTarget {
    crate::build_cache::ScopedCargoTarget::acquire(tmp.path().to_str().unwrap()).unwrap()
}

#[test]
fn cargo_target_environment_includes_sccache_when_available() {
    // This test verifies that the complete ScopedCargoTarget environment
    // (including CARGO_TARGET_DIR and RUSTC_WRAPPER when sccache is available)
    // is properly constructed and would reach a launched backend.
    let tmp = tempfile::tempdir().unwrap();
    let cargo_target = acquire_cargo_target(&tmp);
    let env = cargo_target.environment();

    // CARGO_TARGET_DIR must always be present
    let target_dir = env
        .iter()
        .find(|(key, _)| key == "CARGO_TARGET_DIR")
        .expect("CARGO_TARGET_DIR must be in environment");
    assert_eq!(target_dir.1, cargo_target.path().to_string_lossy());

    // If sccache is available on PATH, RUSTC_WRAPPER must be present
    let has_sccache = std::process::Command::new("which")
        .arg("sccache")
        .output()
        .ok()
        .is_some_and(|out| out.status.success());

    if has_sccache {
        let rustc_wrapper = env
            .iter()
            .find(|(key, _)| key == "RUSTC_WRAPPER")
            .expect("RUSTC_WRAPPER must be in environment when sccache is available");
        assert!(rustc_wrapper.1.contains("sccache"));
    }
}
