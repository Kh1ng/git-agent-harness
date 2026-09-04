use std::path::Path;

pub(super) fn acquire_cargo_target(
    tmp: &tempfile::TempDir,
    session_dir: &Path,
) -> crate::build_cache::ScopedCargoTarget {
    crate::build_cache::ScopedCargoTarget::acquire(tmp.path().to_str().unwrap(), session_dir)
        .unwrap()
}

#[test]
fn sequential_sessions_reuse_a_slot_but_live_sessions_do_not_share() {
    let tmp = tempfile::tempdir().unwrap();
    let session_a = tmp.path().join("sessions/a");
    let session_b = tmp.path().join("sessions/b");
    let live = acquire_cargo_target(&tmp, &session_a);
    let live_path = live.path().to_path_buf();
    assert_ne!(live_path, acquire_cargo_target(&tmp, &session_b).path());
    drop(live);
    assert_eq!(
        live_path,
        acquire_cargo_target(&tmp, &tmp.path().join("sessions/c")).path()
    );
}
