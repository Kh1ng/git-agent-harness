use std::fs;

#[test]
fn unattended_loop_unit_owns_and_kills_the_worker_control_group() {
    let unit = fs::read_to_string("packaging/systemd/gah-loop@.service").unwrap();

    assert!(unit.contains("ExecStart=%h/.cargo/bin/gah loop --profile %i"));
    assert!(unit.contains("Environment=TMPDIR=%h/.cache/gah/tmp"));
    assert!(unit.contains("ExecStartPre=/usr/bin/mkdir -p %h/.cache/gah/tmp"));
    assert!(unit.contains("KillMode=control-group"));
    assert!(unit.contains("Restart=no"));
    assert!(!unit.contains("nohup"));
    assert!(!unit.contains("--once"));
}

#[test]
fn watchdog_unit_preserves_its_printf_placeholder_through_systemd_expansion() {
    let unit = fs::read_to_string("packaging/systemd/gah-watchdog.service").unwrap();

    assert!(unit.contains(r#"printf "%%s\\n" "$$msg""#));
    assert!(!unit.contains(r#"printf "%s\\n" "$$msg""#));
}
