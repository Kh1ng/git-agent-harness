use assert_cmd::Command;
use predicates::boolean::PredicateBooleanExt;
use predicates::str::contains;

#[test]
fn sudo_environment_does_not_refuse_a_non_root_update() {
    // `sudo -iu <account> gah update` runs as that account while keeping
    // SUDO_USER set, so the guard must key on the effective uid (#1322):
    // a non-root run proceeds to checkout access, where this run fails.
    Command::cargo_bin("gah")
        .unwrap()
        .env("SUDO_USER", "provisioning-admin")
        .args(["update", "--repo", "/missing-checkout-must-not-be-accessed"])
        .assert()
        .failure()
        .stderr(contains("is not a Git checkout"))
        .stderr(contains("run gah update without sudo").not());
}

#[test]
fn update_without_pull_accepts_a_local_dirty_checkout_and_cancels_before_changes() {
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&["init", "-b", "local-work"]);
    let file = repo.path().join("untracked.txt");
    std::fs::write(&file, "local changes").unwrap();
    Command::cargo_bin("gah")
        .unwrap()
        .args(["update", "--role", "worker", "--agent", "claude", "--repo"])
        .arg(repo.path())
        .write_stdin("n\n")
        .assert()
        .failure()
        .stderr(contains("Update cancelled before installation"))
        .stdout(contains("Fetch origin").not())
        .stdout(contains("OpenCode").not())
        .stdout(contains("quota-refresh").not());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "local changes");
}
