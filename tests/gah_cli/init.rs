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

#[test]
fn init_fills_empty_worktree_base_in_existing_config() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.toml");
    std::fs::write(&config, "[defaults]\nworktree_base = \"\" # keep\n").unwrap();
    bin()
        .env("HOME", tmp.path())
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
            "--config-path",
            config.to_str().unwrap(),
        ])
        .assert()
        .success();
    let written = std::fs::read_to_string(&config).unwrap();
    let base = tmp.path().join(".local/share/gah/worktrees");
    assert!(
        written.contains(&format!("worktree_base = \"{}\" # keep", base.display())),
        "{written}"
    );
    assert!(written.contains("[profiles.sample]"));
}
