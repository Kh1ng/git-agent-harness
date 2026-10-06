use super::*;
use std::fs;

#[test]
fn review_usage_records_unparsed_artifact() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("review.log");
    let artifact = tmp.path().join("meta.json");
    fs::write(&log, "review completed").unwrap();
    fs::write(&artifact, "{invalid json").unwrap();
    let usage = review_usage(
        log.to_str().unwrap(),
        None,
        UsageAttribution::backend(Some("vibe"), None),
        Some(artifact.to_str().unwrap()),
        None,
    );
    assert_eq!(
        usage.usage_unknown_reason,
        Some(crate::ledger::UsageUnknownReason::UsageArtifactUnparsed)
    );
    assert_eq!(usage.total_tokens, None);
    assert_eq!(usage.requests_count, None);
}

#[test]
fn vibe_run_with_preexisting_cumulative_session_records_missing_usage() {
    use crate::runner::backends::vibe;
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("worktree");
    let session = tmp.path().join("attempt");
    let vibe_home = tmp.path().join("vibe");
    let old = vibe_home.join("logs/session/old/meta.json");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&session).unwrap();
    fs::write(
        &old,
        serde_json::json!({
            "environment": {"working_directory": worktree},
            "stats": {"session_total_llm_tokens": 999999}
        })
        .to_string(),
    )
    .unwrap();
    let fake_vibe = tmp.path().join("fake-vibe");
    fs::write(
        &fake_vibe,
        "#!/bin/sh\necho \"total_tokens: 999999\"\necho \"input_tokens: 500\"\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&fake_vibe).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_vibe, perms).unwrap();
    }
    let envs = vec![("VIBE_HOME".to_string(), vibe_home.display().to_string())];
    let run = vibe::run_with_executable(
        &fake_vibe,
        &worktree,
        "task",
        &session,
        None,
        &[],
        &envs,
        30,
    )
    .unwrap();
    assert_eq!(run.exit_code, 0);
    assert_eq!(run.transcript_path, None);
    let usage = attempt_usage(
        &run.log_path,
        None,
        UsageAttribution::backend(Some("vibe"), None),
        run.transcript_path.as_deref(),
        None,
    );
    assert_eq!(
        usage.usage_unknown_reason,
        Some(crate::ledger::UsageUnknownReason::UsageArtifactMissing)
    );
    assert_eq!(usage.total_tokens, None);
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.output_tokens, None);
    assert_eq!(usage.requests_count, None);
}
