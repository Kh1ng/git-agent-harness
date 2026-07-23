use super::*;

#[test]
fn claims_list_filters_by_profile() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let claim_state = tmp.path().join("claims.json");
    let state = serde_json::json!({
        "version": 2,
        "claims": {
            "real@real": [
                {
                    "work_id": "#436",
                    "pid": 4242,
                    "hostname": "cli-host",
                    "claimed_at": "2026-07-14T00:00:00Z"
                }
            ]
        }
    });
    fs::write(&claim_state, serde_json::to_string_pretty(&state).unwrap()).unwrap();

    let out = bin()
        .args([
            "claims",
            "list",
            "--json",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_CLAIM_STATE_PATH", claim_state.to_str().unwrap())
        .assert()
        .success();

    let claims: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let claims = claims.as_array().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0]["work_id"], "#436");
}

#[test]
fn claims_clear_removes_existing_claim() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let cfg = write_real_repo_config(&tmp, &repo, "github");

    let claim_state = tmp.path().join("claims.json");
    let state = serde_json::json!({
        "version": 2,
        "claims": {
            "real@real": [
                {
                    "work_id": "#436",
                    "pid": 4242,
                    "hostname": "cli-host",
                    "claimed_at": "2026-07-14T00:00:00Z"
                }
            ]
        }
    });
    fs::write(&claim_state, serde_json::to_string_pretty(&state).unwrap()).unwrap();

    bin()
        .args([
            "claims",
            "clear",
            "--work-id",
            "#436",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_CLAIM_STATE_PATH", claim_state.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Cleared claim for work_id #436 on profile real",
        ));

    let out = bin()
        .args([
            "claims",
            "list",
            "--json",
            "--profile",
            "real",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("GAH_CLAIM_STATE_PATH", claim_state.to_str().unwrap())
        .assert()
        .success();

    let claims: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(claims.as_array().unwrap().len(), 0);
}
