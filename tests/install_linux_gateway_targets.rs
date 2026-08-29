use std::path::PathBuf;
use std::process::Command;

// scripts/install-linux.sh has no shell-test harness of its own and its
// gateway-target section runs `sudo`, so this doesn't invoke the installer
// end to end. Instead it runs a fixture (tests/fixtures/
// install_linux_gateway_mapping_test.sh) that extracts the real
// role-to-target mapping verbatim from between that script's
// gateway-target-mapping:start/:end markers and exercises it against a
// scratch HOME with a no-op `sudo` stub -- see issue #919, where a central
// install only ever wrote /etc/gah/server.env and never
// ~/.config/gah/gah-loop.env, the file gah-loop@.service actually reads.
#[test]
fn install_linux_gateway_mapping_writes_correct_targets_per_role() {
    let script = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/install_linux_gateway_mapping_test.sh"
    ));

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("failed to run install_linux_gateway_mapping_test.sh");

    assert!(
        output.status.success(),
        "gateway mapping fixture failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
