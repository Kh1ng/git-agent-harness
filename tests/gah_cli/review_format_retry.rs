use super::*;

const FORMAT_VIOLATION: &str = "Found a worrying edge case.\nReview notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}";
const VALID_APPROVAL: &str = "Review notes\n{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[],\"risk_notes\":[],\"evidence\":[\"file:src.txt\"]}";

#[test]
fn review_writes_structured_verdict_and_posts_comment() {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    let prompt_log = tmp.path().join("review-prompt.txt");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );

    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nprintf '%s' \"$2\" > \"{}\"\ncat <<'EOF'\nReview notes\n{{\"verdict\":\"APPROVE\",\"confidence\":\"high\",\"human_required\":false,\"blocking_findings\":[],\"non_blocking_findings\":[\"Looks fine\"],\"risk_notes\":[\"low risk\"],\"evidence\":[\"file:src.txt\"]}}\nEOF\n",
            prompt_log.display()
        ),
    );
    make_fake_github_review_api(&fake_bin);

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "feature/review",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env("PATH", prepend_path(&fake_bin))
        .assert()
        .success();

    let sessions = tmp.path().join("artifacts/real/sessions");
    let session = latest_child_dir(&sessions);
    let report = fs::read_to_string(session.join("review-report.md")).unwrap();
    let verdict = fs::read_to_string(session.join("review-verdict.json")).unwrap();
    let prompt = fs::read_to_string(prompt_log).unwrap();
    assert!(report.contains("Review notes"));
    assert!(verdict.contains("\"verdict\": \"APPROVE\""));
    assert!(verdict.contains("\"reviewer_backend\": \"claude\""));
    assert!(prompt.contains("Source: feature/review"));
    assert!(prompt.contains("Target: main"));
    assert!(prompt.contains("Changed files:\nsrc.txt"));
}

fn run_format_retry(sequence: Vec<Scenario>) -> (TempDir, FakeBackend, Value) {
    let tmp = test_tempdir();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    add_origin_and_feature_commit(&repo);
    checkout_branch(&repo, "main");

    let backend = FakeBackend::new(tmp.path(), "claude");
    backend.install_sequence(sequence);
    make_fake_github_review_api(backend.bin_dir());
    let cfg = write_real_repo_config_with_extra(
        &tmp,
        &repo,
        "github",
        "[profiles.real.routing]\nreview_backend = \"claude\"\n",
        "",
    );
    let ledger_path = tmp.path().join("ledger.jsonl");

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "review",
            "--branch",
            "feature/review",
            "--config-path",
            cfg.to_str().unwrap(),
        ])
        .env(
            "PATH",
            backend.path_with(&std::env::var("PATH").unwrap_or_default()),
        )
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    let line = fs::read_to_string(ledger_path).unwrap();
    let entry = serde_json::from_str(line.lines().last().unwrap()).unwrap();
    (tmp, backend, entry)
}

#[test]
fn review_workflow_repairs_format_once_on_the_same_reviewer() {
    let (_tmp, backend, entry) = run_format_retry(vec![
        Scenario::success().with_stdout(FORMAT_VIOLATION),
        Scenario::success().with_stdout(VALID_APPROVAL),
    ]);

    assert_eq!(backend.call_count(), 2);
    assert_eq!(entry["attempts_started"], 2);
    assert_eq!(entry["attempts_completed"], 2);
    assert_eq!(entry["attempts"][0]["attempt_number"], 1);
    assert!(entry["attempts"][0]["validation_result"]
        .as_str()
        .unwrap()
        .contains("substantive prose"));
    assert_eq!(entry["attempts"][1]["attempt_number"], 2);
    assert_eq!(entry["attempts"][1]["validation_result"], "APPROVE");
    assert_eq!(entry["review_verdict"], "APPROVE");

    let repaired_prompt = backend.argv_for_call(2).join("\n");
    assert!(repaired_prompt.contains("## Review Format Repair"));
    assert!(!repaired_prompt.contains("## Prior Review Attempt"));
    assert!(!repaired_prompt.contains("Found a worrying edge case"));
}

#[test]
fn review_workflow_stops_after_one_failed_format_repair() {
    let (_tmp, backend, entry) = run_format_retry(vec![
        Scenario::success().with_stdout(FORMAT_VIOLATION),
        Scenario::success().with_stdout(FORMAT_VIOLATION),
    ]);

    assert_eq!(backend.call_count(), 2);
    assert_eq!(entry["attempts_started"], 2);
    assert_eq!(entry["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(entry["review_verdict"], "HUMAN_REVIEW");
    assert_eq!(entry["human_required"], true);
}
