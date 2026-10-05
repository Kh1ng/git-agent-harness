use std::fs;

#[test]
fn unattended_loop_unit_owns_and_kills_the_worker_control_group() {
    let unit = fs::read_to_string("packaging/systemd/gah-loop@.service").unwrap();

    assert!(unit.contains("ExecStart=\"@GAH@\" loop --profile %i"));
    assert!(unit.contains("Environment=TMPDIR=%h/.cache/gah/tmp"));
    assert!(unit.contains("ExecStartPre=/usr/bin/mkdir -p %h/.cache/gah/tmp"));
    assert!(unit.contains("KillMode=control-group"));
    assert!(unit.contains("Restart=no"));
    assert!(!unit.contains("nohup"));
    assert!(!unit.contains("--once"));
}

// Issue #726: the watchdog must be a portable, tracked-in-repo unit that can
// only ever observe loop state, never start/restart/enable one -- unlike its
// predecessor, which called an untracked host-local script that silently
// restarted a stopped loop.
#[test]
fn watchdog_unit_runs_the_packaged_check_and_never_references_the_old_host_script() {
    let unit = fs::read_to_string("packaging/systemd/gah-watchdog.service").unwrap();

    assert!(!unit.contains("/home/khing/workspace/agent-lab"));
    assert!(!unit.contains("gah-watchdog.py"));
    assert!(!unit.contains("/api/loop/start"));

    // Scope the "never invokes a start mechanism" check to the actual
    // executed command, not the surrounding doc comments -- those legitimately
    // name `systemctl start`/`gah loop` in prose to document why the unit
    // must never run them.
    let exec_start = unit
        .lines()
        .find(|line| line.trim_start().starts_with("ExecStart="))
        .expect("unit must define ExecStart");
    assert!(exec_start.contains("\"@GAH@\" watchdog-check"));
    for forbidden in ["systemctl", "gah loop", "curl", "wget"] {
        assert!(
            !exec_start.contains(forbidden),
            "ExecStart must not invoke '{forbidden}': {exec_start}"
        );
    }
}

#[test]
fn optional_gateway_renders_checkout_and_node_paths_without_developer_identity() {
    let installer = fs::read_to_string("scripts/install-linux.sh").unwrap();
    let script = installer
        .split("# gateway-unit-render:start")
        .nth(1)
        .unwrap()
        .split("# gateway-unit-render:end")
        .next()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("gateway.service");
    for (checkout, valid) in [
        ("/srv/memory core%test", true),
        ("/srv/bad\npath", false),
        ("/srv/$HOME", false),
        ("relative", false),
    ] {
        let output = std::process::Command::new("bash")
            .args(["-c", script])
            .env("GAH_GATEWAY_MEMORYCORE_PATH", checkout)
            .env("node_dir", "/opt/node 22/bin")
            .env("gateway_unit_dst", &target)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), valid);
        if valid {
            let rendered = fs::read_to_string(&target).unwrap();
            assert!(rendered.contains("WorkingDirectory=/srv/memory core%%test"));
            assert!(rendered.contains("ExecStart=\"/opt/node 22/bin/npx\" tsx"));
            assert!(!rendered.contains("@MEMORYCORE@") && !rendered.contains("@NODE_DIR@"));
        }
    }
}
