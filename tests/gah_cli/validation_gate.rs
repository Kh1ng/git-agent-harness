use super::*;
/// A broken validation command is a gate failure; the backend must never run.
#[test]
fn dispatch_refuses_on_broken_validation_gate() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) =
        setup_fix_dispatch_repo(&tmp, "validation_commands = [\"sh -c 'exit 1'\"]\n");
    let state_path = tmp.path().join("validation_check.json");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(&fake_bin, "codex", "#!/bin/sh\ntouch codex_ran\n");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "noop",
            "--retries",
            "0",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_VALIDATION_CHECK_PATH", &state_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("VALIDATION GATE FAILED"));

    let state_text = fs::read_to_string(&state_path).unwrap();
    assert!(
        state_text.contains("\"last_verified_ok\": false"),
        "broken gate must be recorded as not-ok: {}",
        state_text
    );
    assert!(
        !tmp.path().join("codex_ran").exists(),
        "backend must not run when the validation gate fails"
    );
}

#[test]
fn validation_gate_reports_the_underlying_runner_error_chain() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"sleep 2\"]\nvalidation_timeout_seconds = 1\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            "noop",
            "--retries",
            "0",
        ])
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .assert()
        .failure()
        .stderr(predicate::str::contains("failed to run 'sleep 2'"))
        .stderr(predicate::str::contains(
            "validation command 'sleep 2' timed out after",
        ));
}
