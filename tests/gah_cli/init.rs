use super::*;

#[test]
fn init_prints_profile_snippet() {
    bin()
        .args([
            "init",
            "--profile",
            "sample",
            "--display-name",
            "Sample Repo",
            "--provider",
            "github",
            "--repo",
            "owner/sample",
            "--local-path",
            "/tmp/sample",
            "--print",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[profiles.sample]"))
        .stdout(predicate::str::contains("provider = \"github\""));
}
