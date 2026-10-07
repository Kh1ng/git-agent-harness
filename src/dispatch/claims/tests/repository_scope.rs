use super::*;

pub(super) fn claim_fixture(
    root: &Path,
) -> (crate::config::GahConfig, Profile, super::DispatchArgs) {
    let ticket_dir = root.join("docs/tickets");
    fs::create_dir_all(&ticket_dir).unwrap();
    let ticket_path = ticket_dir.join("TICKET-500-test.md");
    fs::write(
        &ticket_path,
        "# TICKET-500: Test\n\nGoal: test claim guard\n",
    )
    .unwrap();

    let cfg = crate::config::GahConfig {
        context: Default::default(),
        defaults: crate::config::Defaults {
            current_manager: None,
            artifact_root: root.to_string_lossy().into_owned(),
            worktree_base: root.to_string_lossy().into_owned(),
            llm_base_url: String::new(),
            llm_model_local: String::new(),
            llm_model_cloud: String::new(),
            routing: crate::config::RoutingPolicy::default(),

            ..Default::default()
        },
        profiles: std::collections::HashMap::new(),
    };
    let mut prof = profile(root);
    prof.provider = "github".to_string();
    prof.repo = "owner/repo".to_string();

    let args = super::DispatchArgs {
        profile: "test".to_string(),
        mode: "improve".to_string(),
        backend: "codex".to_string(),
        target: ticket_path.display().to_string(),
        branch: None,
        mr: None,
        current_branch: false,
        dry_run: false,
        oh_profile: None,
        model: None,
        retries: 0,
        allow_draft_fail: false,
        prod: false,
        issue_intake_override: false,
        allow_unknown_red_baseline: false,
        escalate: false,
        existing_branch: None,
        expected_review_generation: None,
        skip_validation_gate: false,
        dispatch_reason: None,
        prior_attempt_context: None,
        work_id: None,
        run_id: None,
        route_admission: None,
    };

    (cfg, prof, args)
}

fn duplicate_work_has_active_claim(claim_for_this_repo: bool) -> bool {
    let _exec_guard = crate::test_support::ExecGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_fake_gh(&bin_dir, "[]");
    let _guard = PathGuard::set(&bin_dir);

    let (cfg, prof, args) = claim_fixture(tmp.path());

    let mut other_prof = prof.clone();
    other_prof.repo_id = format!("{}-other", prof.repo_id);
    let claim_profile = if claim_for_this_repo {
        &prof
    } else {
        &other_prof
    };
    let mut entries = vec![LedgerEntry::new_claim("test", claim_profile, "TICKET-500")];
    if claim_for_this_repo {
        let mut ended = LedgerEntry::new("test", &other_prof, "codex", "fix", "x", None, None);
        ended.work_id = Some("TICKET-500".into());
        ended.failure_class = Some("backend_error".into());
        ended.validation_result = Some("failed".into());
        entries.push(ended);
    }
    let lines: String = entries
        .iter()
        .map(|e| format!("{}\n", serde_json::to_string(e).unwrap()))
        .collect();
    fs::write(tmp.path().join("ledger.jsonl"), lines).unwrap();
    match super::check_duplicate_work(&cfg, &prof, &args, false) {
        Ok(work_id) => {
            assert_eq!(work_id.as_deref(), Some("TICKET-500"));
            false
        }
        Err(error) => {
            assert!(error.downcast_ref::<ActiveClaimError>().is_some());
            true
        }
    }
}

#[test]
fn other_repository_execution_does_not_resolve_this_repository_claim() {
    assert!(duplicate_work_has_active_claim(true));
}

#[test]
fn other_repository_claim_does_not_block_this_repository_dispatch() {
    assert!(!duplicate_work_has_active_claim(false));
}
