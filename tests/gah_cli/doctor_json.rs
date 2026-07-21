use super::*;

#[test]
fn text_output_still_passes_for_a_valid_profile() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");

    bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"))
        .stdout(predicate::str::contains("manager memory"));
}

#[test]
fn emits_structured_readiness_checks_without_text_noise() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin(&fake_bin, "gh");

    let output = bin()
        .args([
            "doctor",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("GITHUB_TOKEN", "token")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let snapshot: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(snapshot["schema_version"], 1);
    assert!(matches!(
        snapshot["overall_status"].as_str(),
        Some("ok" | "warn")
    ));
    assert!(snapshot["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|check| {
            check["profile"] == "real"
                && check["name"] == "manager memory"
                && check["status"] == "ok"
        })
    }));
}
