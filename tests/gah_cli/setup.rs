use super::*;

#[test]
fn memory_hook_setup_installs_from_the_binary_and_merges_repeatably() {
    let home = test_tempdir();
    let claude = home.path().join(".claude/settings.json");
    fs::create_dir_all(claude.parent().unwrap()).unwrap();
    fs::write(&claude, r#"{"permissions":{"allow":["Read"]}}"#).unwrap();
    for _ in 0..2 {
        bin()
            .args([
                "setup",
                "memory-hooks",
                "--tool",
                "claude,codex",
                "--home-dir",
            ])
            .arg(home.path())
            .assert()
            .success();
    }
    let settings: Value = serde_json::from_slice(&fs::read(&claude).unwrap()).unwrap();
    assert_eq!(
        settings["permissions"]["allow"],
        serde_json::json!(["Read"])
    );
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().unwrap().len(),
        1
    );
    assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert!(home.path().join(".codex/hooks.json").is_file());
    assert_eq!(
        fs::read_to_string(home.path().join(".local/bin/gah-memory-hook")).unwrap(),
        include_str!("../../scripts/gah-memory-hook.py")
    );
    assert!(!home.path().join(".config/gah/memory-hooks.json").exists());
}
