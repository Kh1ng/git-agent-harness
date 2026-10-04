use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn sudo_update_refuses_before_accessing_or_changing_the_checkout() {
    Command::cargo_bin("gah")
        .unwrap()
        .env("SUDO_USER", "tester")
        .env("HOME", "/root")
        .args(["update", "--repo", "/missing-checkout-must-not-be-accessed"])
        .assert()
        .failure()
        .stderr(contains("run gah update without sudo"));
}
