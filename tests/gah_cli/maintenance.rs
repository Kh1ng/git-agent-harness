// Residual maintenance-focused CLI command tests that were previously kept in
// `tests/gah_cli.rs`.

use super::*;

#[test]
fn warn_candidates_are_skipped_by_default() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    bin()
        .args([
            "candidates",
            "--gate-artifact",
            gate.to_str().unwrap(),
            "--out-root",
            out_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let artifact = latest_child_dir(&out_root.join("scout-to-backlog-candidates"));
    let data: Value =
        serde_json::from_str(&fs::read_to_string(artifact.join("candidates.json")).unwrap())
            .unwrap();

    assert_eq!(data["counts"]["seen"], 1);
    assert_eq!(data["counts"]["converted"], 0);
    assert_eq!(data["counts"]["skipped_warning"], 1);
    assert_eq!(data["candidates"].as_array().unwrap().len(), 0);
}

#[test]
fn warn_candidates_are_included_with_flag_and_hydrated_from_scout() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    bin()
        .args([
            "candidates",
            "--gate-artifact",
            gate.to_str().unwrap(),
            "--include-warnings",
            "--out-root",
            out_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let artifact = latest_child_dir(&out_root.join("scout-to-backlog-candidates"));
    let data: Value =
        serde_json::from_str(&fs::read_to_string(artifact.join("candidates.json")).unwrap())
            .unwrap();

    assert_eq!(data["counts"]["converted"], 1);
    let c = &data["candidates"][0];

    assert_eq!(c["candidate_id"], "001");
    assert_eq!(c["source_gate_status"], "warn");
    assert_eq!(c["suggested_blueprint_phase"], "needs:human");
    assert_eq!(c["provider_mutation_allowed"], false);

    let labels = c["suggested_labels"].as_array().unwrap();
    assert!(labels.iter().any(|v| v == "type:docs"));
    assert!(labels.iter().any(|v| v == "risk:low"));
    assert!(labels.iter().any(|v| v == "needs:human-review"));
    assert!(!labels.iter().any(|v| v == "agent:ready"));

    assert!(c["affected_files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "README.md"));
    assert!(!c["evidence"].as_array().unwrap().is_empty());
    assert!(!c["acceptance_criteria"].as_array().unwrap().is_empty());
    assert!(!c["verification"].as_array().unwrap().is_empty());

    assert_eq!(c["hydration_used"], true);
    assert_eq!(c["hydration_match_method"], "id");
}

#[test]
fn candidate_artifacts_are_unique_and_never_overwritten() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    for _ in 0..2 {
        bin()
            .args([
                "candidates",
                "--gate-artifact",
                gate.to_str().unwrap(),
                "--include-warnings",
                "--out-root",
                out_root.to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    let root = out_root.join("scout-to-backlog-candidates");
    let dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();

    assert_eq!(dirs.len(), 2);
    assert_ne!(dirs[0], dirs[1]);
}

#[test]
fn price_guard_allows_active_default() {
    let tmp = write_fixture_dir();
    let watchlist = tmp.path().join("model_watchlist.json");

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "deepseek/deepseek-v4-flash",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("allowed"));
}

#[test]
fn price_guard_blocks_unavailable_model() {
    let tmp = write_fixture_dir();
    let watchlist = tmp.path().join("model_watchlist.json");

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "qwen/qwen3-235b-a22b-2507",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn price_guard_blocks_model_priced_over_input_ceiling() {
    let tmp = test_tempdir();
    let watchlist = tmp.path().join("watchlist.json");
    fs::write(
        &watchlist,
        r#"{"models":[{"id":"pricey/model","status":"available","input_per_1m":0.5,"output_per_1m":0.1,"max_input_per_1m":0.2,"max_output_per_1m":0.4}]}"#,
    )
    .unwrap();

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "pricey/model",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn price_guard_blocks_model_priced_over_output_ceiling() {
    let tmp = test_tempdir();
    let watchlist = tmp.path().join("watchlist.json");
    fs::write(
        &watchlist,
        r#"{"models":[{"id":"pricey/model","status":"available","input_per_1m":0.1,"output_per_1m":0.5,"max_input_per_1m":0.2,"max_output_per_1m":0.4}]}"#,
    )
    .unwrap();

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "pricey/model",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn price_guard_blocks_models_missing_from_watchlist() {
    // Unknown models fail closed: an unwatched model must never be
    // silently treated as affordable.
    let tmp = test_tempdir();
    let watchlist = tmp.path().join("watchlist.json");
    fs::write(
        &watchlist,
        r#"{"models":[{"id":"known/model","status":"available","input_per_1m":0.1,"output_per_1m":0.1,"max_input_per_1m":0.2,"max_output_per_1m":0.4}]}"#,
    )
    .unwrap();

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "unlisted/model",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn price_guard_rejects_malformed_watchlist_as_error_not_decision() {
    // A corrupt watchlist must fail loudly (exit 1, error on stderr) and
    // must not print a decision, so callers can distinguish "blocked" from
    // "could not evaluate".
    let tmp = test_tempdir();
    let watchlist = tmp.path().join("watchlist.json");
    fs::write(&watchlist, "not json").unwrap();

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "any/model",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Error"));
}

#[test]
fn work_trust_mode_blocks_provider_mutation() {
    let tmp = test_tempdir();
    let cfg = tmp.path().join("work-readonly.toml");

    fs::write(
        &cfg,
        r#"
[repo]
name = "work/private-repo"
provider = "github"
trust_mode = "read_only"
allow_provider_mutation = false
allow_push = false
allow_draft_pr = false
allow_issue_write = false
allow_project_write = false
"#,
    )
    .unwrap();

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "open-draft-pr",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn personal_draft_pr_mode_allows_only_draft_pr() {
    let tmp = test_tempdir();
    let cfg = tmp.path().join("personal-draft.toml");

    fs::write(
        &cfg,
        r#"
[repo]
name = "personal/repo"
provider = "github"
trust_mode = "draft_pr_allowed"
allow_provider_mutation = true
allow_push = true
allow_draft_pr = true
allow_issue_write = false
allow_project_write = false
"#,
    )
    .unwrap();

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "open-draft-pr",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("allowed"));

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "edit-issue",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}
