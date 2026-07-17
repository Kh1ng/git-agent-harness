use super::*;

fn profile_with_escalatory_reviewers(
    tmp: &tempfile::TempDir,
    reviewers: Vec<CandidateConfig>,
) -> Profile {
    let mut prof = profile(tmp.path());
    prof.routing.escalatory_reviewers = reviewers;
    prof
}

fn append_routine_reviews(cfg: &GahConfig, prof: &Profile, count: usize, work_id: &str) {
    for _ in 0..count {
        let mut entry = review_ledger_entry("test", prof, "gah/42", "NEEDS_FIX", "high");
        entry.work_id = Some(work_id.into());
        crate::ledger::append(cfg, &entry).unwrap();
    }
}

#[test]
fn first_escalatory_reviewer_attempt_is_reserved_after_routine_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            max_review_cycles_per_ticket: Some(2),
            ..RoutingPolicy::default()
        },
    );
    let prof = profile_with_escalatory_reviewers(
        &tmp,
        vec![CandidateConfig {
            backend: "claude".into(),
            model: Some("sonnet".into()),
            ..CandidateConfig::default()
        }],
    );
    append_routine_reviews(&cfg, &prof, 2, "#42");

    let block = check_review_budget(
        &cfg,
        &prof,
        "test",
        Some("#42"),
        &route_decision("claude", Some("sonnet"), false),
    )
    .unwrap();

    assert!(
        block.is_none(),
        "the configured escalation must get one attempt after routine reviews exhaust the cap"
    );
}

#[test]
fn attempted_escalatory_reviewer_cannot_bypass_cycle_cap_again() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            max_review_cycles_per_ticket: Some(2),
            ..RoutingPolicy::default()
        },
    );
    let prof = profile_with_escalatory_reviewers(
        &tmp,
        vec![CandidateConfig {
            backend: "claude".into(),
            model: Some("sonnet".into()),
            ..CandidateConfig::default()
        }],
    );
    append_routine_reviews(&cfg, &prof, 2, "#42");
    let mut prior_escalation = review_ledger_entry("test", &prof, "gah/42", "NEEDS_FIX", "high");
    prior_escalation.work_id = Some("#42".into());
    prior_escalation.effective_backend = "claude".into();
    prior_escalation.effective_model = Some("sonnet".into());
    // Deliberately leave reviewer_class unset to cover legacy attribution.
    crate::ledger::append(&cfg, &prior_escalation).unwrap();

    let block = check_review_budget(
        &cfg,
        &prof,
        "test",
        Some("#42"),
        &route_decision("claude", Some("sonnet"), false),
    )
    .unwrap()
    .expect("an already-attempted escalation must remain bounded");

    assert!(block.reason.contains("3/2 review cycles"));
}

#[test]
fn each_distinct_escalatory_reviewer_gets_one_bounded_attempt() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = gah_config_with_ledger(
        tmp.path(),
        RoutingPolicy {
            max_review_cycles_per_ticket: Some(1),
            ..RoutingPolicy::default()
        },
    );
    let prof = profile_with_escalatory_reviewers(
        &tmp,
        vec![
            CandidateConfig {
                backend: "claude".into(),
                model: Some("sonnet".into()),
                ..CandidateConfig::default()
            },
            CandidateConfig {
                backend: "opencode".into(),
                model: Some("nous-portal/z-ai/glm-5.2".into()),
                ..CandidateConfig::default()
            },
        ],
    );
    append_routine_reviews(&cfg, &prof, 1, "#42");
    let mut claude = review_ledger_entry("test", &prof, "gah/42", "NEEDS_FIX", "high");
    claude.work_id = Some("#42".into());
    claude.effective_backend = "claude".into();
    claude.effective_model = Some("sonnet".into());
    claude.reviewer_class = Some("escalatory:claude/sonnet".into());
    crate::ledger::append(&cfg, &claude).unwrap();

    let block = check_review_budget(
        &cfg,
        &prof,
        "test",
        Some("#42"),
        &route_decision("opencode", Some("nous-portal/z-ai/glm-5.2"), false),
    )
    .unwrap();

    assert!(
        block.is_none(),
        "Claude history must not consume GLM's one distinct escalation attempt"
    );
}
