use crate::*;

#[test]
fn dispatch_experiment_missing_backend_records_backend_not_invoked() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, "");
    let ledger_path = tmp.path().join("ledger.jsonl");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    // Routing sees an installed, executable codex, but its interpreter does
    // not exist, so spawning fails before any backend process or output log
    // is created: the backend is selected yet never invoked.
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        "#!/definitely/does/not/exist/interpreter\n",
    );
    // Shadow every other runner so neither a fallback nor the experiment
    // judge (`claude -p`) can reach a real host CLI.
    for backend in ["claude", "openhands", "opencode", "vibe", "agy", "hermes"] {
        make_fake_bin_with_body(&fake_bin, backend, "#!/bin/sh\necho NO\nexit 0\n");
    }

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "experiment",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "experiment with the thing",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let text = fs::read_to_string(&ledger_path).unwrap();
    let entry: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(entry["mode"], "experiment");
    // The selected route did not fall back to another backend.
    assert_eq!(entry["backend"], "codex");
    assert_eq!(
        entry["usage"]["usage_unknown_reason"].as_str(),
        Some("backend_not_invoked")
    );
    assert!(entry["usage"]["usage_source"].is_null());
    assert!(entry["usage"]["total_tokens"].is_null());
}
