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
