use super::*;

#[test]
fn clear_attempts_resets_branch_escalation_and_spent_reviewers() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
    let mut prof = profile(tmp.path());
    prof.routing.escalatory_reviewers = vec![
        CandidateConfig {
            backend: "claude".into(),
            model: Some("sonnet".into()),
            ..Default::default()
        },
        CandidateConfig {
            backend: "opencode".into(),
            model: Some("nous-portal/z-ai/glm-5.2".into()),
            ..Default::default()
        },
    ];
    for (backend, model) in [
        ("agy", "Claude Sonnet 4.6 (Thinking)"),
        ("claude", "sonnet"),
    ] {
        let mut review = review_ledger_entry("test", &prof, "gah/branch-1", "NEEDS_FIX", "high");
        review.work_id = Some("#42".into());
        review.effective_backend = backend.into();
        review.effective_model = Some(model.into());
        crate::ledger::append(&cfg, &review).unwrap();
    }
    crate::ledger::append(
        &cfg,
        &LedgerEntry::new_clear_attempts("test", &prof, "TICKET-42"),
    )
    .unwrap();

    assert_eq!(
        review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
        None
    );
    let next = next_escalatory_reviewer(&cfg, &prof, "test", "gah/branch-1", None)
        .expect("pre-reset escalatory reviewer must be reusable");
    assert_eq!(
        (next.backend.as_str(), next.model.as_deref()),
        ("claude", Some("sonnet"))
    );
}

#[test]
fn post_reset_reviews_escalate_without_cross_scope_reset() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(tmp.path(), RoutingPolicy::default());
    let prof = profile(tmp.path());
    let append_review = |verdict: &str| {
        let mut review = review_ledger_entry("test", &prof, "gah/branch-1", verdict, "high");
        review.work_id = Some("#42".into());
        crate::ledger::append(&cfg, &review).unwrap();
    };
    append_review("NEEDS_FIX");
    crate::ledger::append(
        &cfg,
        &LedgerEntry::new_clear_attempts("other-profile", &prof, "#42"),
    )
    .unwrap();
    crate::ledger::append(&cfg, &LedgerEntry::new_clear_attempts("test", &prof, "#99")).unwrap();
    let mut other_repo = prof.clone();
    other_repo.repo_id = "other-repo".into();
    crate::ledger::append(
        &cfg,
        &LedgerEntry::new_clear_attempts("test", &other_repo, "#42"),
    )
    .unwrap();
    append_review("REJECT");

    assert_eq!(
        review_escalation_reason(&cfg, &prof, "test", "gah/branch-1"),
        Some("repeated_needs_fix")
    );
}
