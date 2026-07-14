use super::*;
use crate::ledger::test_util::{profile, test_config};
use crate::ledger::{append, LedgerEntry};

#[test]
fn is_strong_model_returns_false_for_cheap_models() {
    assert!(!is_strong_model("deepseek-flash"));
    assert!(!is_strong_model("gpt-4o-mini"));
    assert!(!is_strong_model("claude-sonnet-tiny"));
    assert!(!is_strong_model("llama-lite"));
    assert!(!is_strong_model("openai/gpt-4o-mini"));
    assert!(!is_strong_model("anthropic/claude-3-haiku-flash"));
    assert!(!is_strong_model("deepseek/deepseek-v4-flash"));
}

#[test]
fn is_strong_model_returns_true_for_strong_models() {
    assert!(is_strong_model("claude-sonnet-4"));
    assert!(is_strong_model("gpt-4o"));
    assert!(is_strong_model("anthropic/claude-opus-4"));
    assert!(is_strong_model("openai/gpt-4o"));
    assert!(is_strong_model("gpt-5.5"));
}

#[test]
fn cheap_flash_model_does_not_increment_strong_run_count() {
    let (_tmp, cfg) = test_config();

    let mut entry = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "improve",
        "do something",
        Some("session-1".into()),
        None,
    );
    entry.effective_model = Some("deepseek-flash".into());
    append(&cfg, &entry).unwrap();

    let summary = usage_summary_for_backend(&cfg, "codex", None, None).unwrap();
    assert_eq!(
        summary.runs_this_week, 1,
        "cheap model run should still be counted as a run"
    );
    assert_eq!(summary.strong_runs_this_week, 0);
    assert_eq!(summary.strong_runs_this_session, 0);
}

#[test]
fn strong_model_increments_strong_run_count() {
    let (_tmp, cfg) = test_config();

    let mut entry = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "improve",
        "do something",
        Some("session-1".into()),
        None,
    );
    entry.effective_model = Some("claude-sonnet-4".into());
    append(&cfg, &entry).unwrap();

    // Verify the entry was written and can be read back
    let entries = crate::ledger::read_entries(&cfg).unwrap();
    assert_eq!(entries.len(), 1, "should have 1 entry");
    assert_eq!(
        entries[0].effective_model.as_deref(),
        Some("claude-sonnet-4")
    );
    assert!(super::is_strong_model(
        entries[0].effective_model.as_deref().unwrap()
    ));

    let summary = usage_summary_for_backend(&cfg, "codex", None, None).unwrap();
    assert_eq!(summary.runs_this_week, 1, "should count as a run");
    assert_eq!(summary.strong_runs_this_week, 1);
    assert_eq!(
        summary.strong_runs_this_session, 0,
        "no session filter applied"
    );
}

#[test]
fn usage_summary_preserves_existing_counts() {
    let (_tmp, cfg) = test_config();

    // Cheap model run — should NOT count as strong but should count as a run
    let mut e1 = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "improve",
        "task 1",
        Some("session-1".into()),
        None,
    );
    e1.effective_model = Some("deepseek-flash".into());
    e1.usage.estimated_cost_usd = Some(0.01);
    e1.usage.actual_cost_usd = Some(0.01);
    append(&cfg, &e1).unwrap();

    // Strong model run — should count as both run and strong run
    let mut e2 = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "fix",
        "task 2",
        Some("session-1".into()),
        None,
    );
    e2.effective_model = Some("claude-sonnet-4".into());
    e2.usage.estimated_cost_usd = Some(0.10);
    e2.usage.actual_cost_usd = Some(0.10);
    append(&cfg, &e2).unwrap();

    // Another strong model run in review mode
    let mut e3 = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "review",
        "task 3",
        Some("session-1".into()),
        None,
    );
    e3.effective_model = Some("gpt-4o".into());
    e3.usage.estimated_cost_usd = Some(0.05);
    e3.usage.actual_cost_usd = Some(0.05);
    append(&cfg, &e3).unwrap();

    // Strong model run with low confidence — should NOT count as strong
    let mut e4 = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "improve",
        "task 4",
        Some("session-1".into()),
        None,
    );
    e4.effective_model = Some("claude-sonnet-4".into());
    e4.confidence_impact = Some("low".into());
    e4.usage.estimated_cost_usd = Some(0.10);
    e4.usage.actual_cost_usd = Some(0.10);
    append(&cfg, &e4).unwrap();

    let summary = usage_summary_for_backend(&cfg, "codex", None, None).unwrap();
    assert_eq!(summary.runs_this_week, 4);
    assert_eq!(summary.runs_this_session, 0, "no session filter applied");
    assert_eq!(summary.strong_runs_this_week, 2); // e2 (claude-sonnet-4, fix) + e3 (gpt-4o, review)
    assert_eq!(
        summary.strong_runs_this_session, 0,
        "no session filter applied"
    );
    assert!((summary.estimated_cost_this_week - 0.26).abs() < 0.001);
    assert!((summary.actual_cost_this_week - 0.26).abs() < 0.001);
}

#[test]
fn strong_run_on_other_backend_does_not_increment_summary() {
    let (_tmp, cfg) = test_config();

    let mut claude = LedgerEntry::new(
        "test",
        &profile(),
        "claude",
        "fix",
        "task",
        Some("session-1".into()),
        None,
    );
    claude.effective_model = Some("claude-sonnet-4".into());
    append(&cfg, &claude).unwrap();

    let summary = usage_summary_for_backend(&cfg, "codex", None, Some("session-1")).unwrap();
    assert_eq!(summary.runs_this_week, 0);
    assert_eq!(summary.strong_runs_this_week, 0);
    assert_eq!(summary.strong_runs_this_session, 0);
}

#[test]
fn strong_run_on_other_model_does_not_increment_filtered_model_summary() {
    let (_tmp, cfg) = test_config();

    let mut entry = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "fix",
        "task",
        Some("session-1".into()),
        None,
    );
    entry.effective_model = Some("claude-sonnet-4".into());
    append(&cfg, &entry).unwrap();

    let summary =
        usage_summary_for_backend(&cfg, "codex", Some("gpt-4o"), Some("session-1")).unwrap();
    assert_eq!(summary.runs_this_week, 0);
    assert_eq!(summary.strong_runs_this_week, 0);
    assert_eq!(summary.strong_runs_this_session, 0);
}

#[test]
fn strong_session_count_only_uses_same_backend_and_model() {
    let (_tmp, cfg) = test_config();

    let mut same = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "fix",
        "task-same",
        Some("session-1".into()),
        None,
    );
    same.effective_model = Some("claude-sonnet-4".into());
    append(&cfg, &same).unwrap();

    let mut other_backend = LedgerEntry::new(
        "test",
        &profile(),
        "claude",
        "fix",
        "task-other-backend",
        Some("session-1".into()),
        None,
    );
    other_backend.effective_model = Some("claude-sonnet-4".into());
    append(&cfg, &other_backend).unwrap();

    let mut other_model = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "fix",
        "task-other-model",
        Some("session-1".into()),
        None,
    );
    other_model.effective_model = Some("gpt-4o".into());
    append(&cfg, &other_model).unwrap();

    let summary =
        usage_summary_for_backend(&cfg, "codex", Some("claude-sonnet-4"), Some("session-1"))
            .unwrap();
    assert_eq!(summary.runs_this_session, 1);
    assert_eq!(summary.strong_runs_this_session, 1);
    assert_eq!(summary.strong_runs_this_week, 1);
}

#[test]
fn effective_model_takes_priority_over_requested_model() {
    let (_tmp, cfg) = test_config();
    let mut entry = LedgerEntry::new("test", &profile(), "codex", "improve", "task", None, None);
    entry.effective_model = Some("claude-sonnet-4".into());
    entry.requested_model = Some("deepseek-flash".into());
    append(&cfg, &entry).unwrap();

    let summary = usage_summary_for_backend(&cfg, "codex", None, None).unwrap();
    assert_eq!(
        summary.strong_runs_this_week, 1,
        "effective_model should take priority over requested_model"
    );
}

#[test]
fn requested_model_fallback_counts_strong_when_effective_missing() {
    let (_tmp, cfg) = test_config();
    let mut entry = LedgerEntry::new("test", &profile(), "codex", "fix", "task", None, None);
    entry.effective_model = None;
    entry.requested_model = Some("claude-sonnet-4".into());
    append(&cfg, &entry).unwrap();

    let summary = usage_summary_for_backend(&cfg, "codex", None, None).unwrap();
    assert_eq!(
        summary.strong_runs_this_week, 1,
        "requested_model fallback should count when effective_model is None"
    );
}

#[test]
fn unknown_model_identity_does_not_count_as_strong() {
    let (_tmp, cfg) = test_config();
    // Empty string and whitespace-only models are filtered by ledger_entry_model_name
    // Note: "unknown" model strings are treated as strong by is_strong_model heuristic
    // (conservative assumption). Only truly missing/empty identities are not-strong.
    let mut e1 = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "improve",
        "task-empty",
        None,
        None,
    );
    e1.effective_model = Some("".into());
    append(&cfg, &e1).unwrap();

    let mut e2 = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "improve",
        "task-whitespace",
        None,
        None,
    );
    e2.effective_model = Some("  ".into());
    append(&cfg, &e2).unwrap();

    let mut e3 = LedgerEntry::new(
        "test",
        &profile(),
        "codex",
        "improve",
        "task-none",
        None,
        None,
    );
    e3.effective_model = None;
    append(&cfg, &e3).unwrap();

    let summary = usage_summary_for_backend(&cfg, "codex", None, None).unwrap();
    assert_eq!(
        summary.strong_runs_this_week, 0,
        "missing/empty/whitespace model should not count as strong"
    );
    assert_eq!(
        summary.runs_this_week, 3,
        "all entries should count as regular runs"
    );
}

#[test]
fn group_by_enum_parsing() {
    assert_eq!("backend".parse::<GroupBy>().unwrap(), GroupBy::Backend);
    assert_eq!("model".parse::<GroupBy>().unwrap(), GroupBy::Model);
    assert_eq!("none".parse::<GroupBy>().unwrap(), GroupBy::None);
    assert_eq!("BACKEND".parse::<GroupBy>().unwrap(), GroupBy::Backend);
    assert_eq!("MODEL".parse::<GroupBy>().unwrap(), GroupBy::Model);
    assert!("invalid".parse::<GroupBy>().is_err());
}

#[test]
fn build_grouped_summary_by_backend() {
    let (_tmp, _cfg) = test_config();
    let mut entry1 = LedgerEntry::new("test", &profile(), "codex", "improve", "test1", None, None);
    entry1.effective_backend = "codex".to_string();
    entry1.effective_model = Some("claude-sonnet".to_string());
    entry1.review_verdict = Some("APPROVE".to_string());
    entry1.review_confidence = Some("high".to_string());
    entry1.reviewer_backend = Some("codex".to_string());
    entry1.reviewer_model = Some("claude-sonnet".to_string());
    entry1.validation_result = Some("passed".to_string());
    entry1.usage.actual_cost_usd = Some(0.5);

    let mut entry2 = LedgerEntry::new("test", &profile(), "vibe", "improve", "test2", None, None);
    entry2.effective_backend = "vibe".to_string();
    entry2.effective_model = Some("mistral-medium".to_string());
    entry2.review_verdict = Some("NEEDS_FIX".to_string());
    entry2.review_confidence = Some("medium".to_string());
    entry2.reviewer_backend = Some("vibe".to_string());
    entry2.reviewer_model = Some("mistral-medium".to_string());
    entry2.validation_result = Some("failed".to_string());
    entry2.usage.actual_cost_usd = Some(0.3);

    let entries = vec![entry1, entry2];
    let grouped = super::build_grouped_summary(
        &entries,
        |entry| entry.effective_backend.clone(),
        |observed| observed.backend.to_string(),
        |backend, _model| backend.to_string(),
        true,
    );

    assert!(grouped.is_some());
    let grouped = grouped.unwrap();
    assert_eq!(grouped.len(), 2);

    // Find codex group
    let codex_group = grouped.iter().find(|g| g.group_key == "codex").unwrap();
    assert_eq!(codex_group.entries, 1);
    assert_eq!(codex_group.validation_pass, 1);
    assert_eq!(
        codex_group.review_verdict_distribution.get("APPROVE"),
        Some(&1)
    );
    assert!((codex_group.total_cost_usd.unwrap() - 0.5).abs() < f64::EPSILON);

    // Find vibe group
    let vibe_group = grouped.iter().find(|g| g.group_key == "vibe").unwrap();
    assert_eq!(vibe_group.entries, 1);
    assert_eq!(vibe_group.validation_pass, 0);
    assert_eq!(
        vibe_group.review_verdict_distribution.get("NEEDS_FIX"),
        Some(&1)
    );
    assert!((vibe_group.total_cost_usd.unwrap() - 0.3).abs() < f64::EPSILON);
}

#[test]
fn model_grouping_labels_missing_model_instead_of_collapsing_to_empty_string() {
    // Regression: build_summary's grouped_by_model closures used to be
    // `unwrap_or_default()`, so every entry with no effective_model
    // (an early-exit dispatch, a review-mode entry, etc.) silently
    // merged into one opaque `""` group -- indistinguishable in the API
    // response from a real model literally named "". Production now
    // uses `summary::UNKNOWN_MODEL_LABEL` for this same fallback.
    let (_tmp, _cfg) = test_config();
    let mut entry1 = LedgerEntry::new("test", &profile(), "auto", "fix", "test1", None, None);
    entry1.effective_backend = "auto".to_string();
    entry1.effective_model = None;

    let mut entry2 = LedgerEntry::new("test", &profile(), "codex", "improve", "test2", None, None);
    entry2.effective_backend = "codex".to_string();
    entry2.effective_model = Some("gpt-4".to_string());

    let entries = vec![entry1, entry2];
    let grouped = super::build_grouped_summary(
        &entries,
        |entry| {
            entry
                .effective_model
                .clone()
                .unwrap_or_else(|| super::UNKNOWN_MODEL_LABEL.to_string())
        },
        |observed| {
            observed
                .model
                .map(str::to_string)
                .unwrap_or_else(|| super::UNKNOWN_MODEL_LABEL.to_string())
        },
        |_backend, model| {
            model
                .map(str::to_string)
                .unwrap_or_else(|| super::UNKNOWN_MODEL_LABEL.to_string())
        },
        false,
    )
    .unwrap();

    assert!(grouped.iter().any(|g| g.group_key == "(unknown model)"));
    assert!(grouped.iter().all(|g| !g.group_key.is_empty()));
    assert!(grouped.iter().any(|g| g.group_key == "gpt-4"));
}

#[test]
fn build_grouped_summary_by_model() {
    let (_tmp, _cfg) = test_config();
    let mut entry1 = LedgerEntry::new("test", &profile(), "codex", "improve", "test1", None, None);
    entry1.effective_backend = "codex".to_string();
    entry1.effective_model = Some("gpt-4".to_string());
    entry1.review_verdict = Some("APPROVE".to_string());
    entry1.reviewer_tier = Some("strong".to_string());
    entry1.validation_result = Some("passed".to_string());
    entry1.usage.actual_cost_usd = Some(1.0);

    let mut entry2 = LedgerEntry::new("test", &profile(), "codex", "improve", "test2", None, None);
    entry2.effective_backend = "codex".to_string();
    entry2.effective_model = Some("gpt-4".to_string());
    entry2.review_verdict = Some("APPROVE".to_string());
    entry2.reviewer_tier = Some("strong".to_string());
    entry2.validation_result = Some("passed".to_string());
    entry2.usage.actual_cost_usd = Some(2.0);

    let mut entry3 = LedgerEntry::new("test", &profile(), "vibe", "improve", "test3", None, None);
    entry3.effective_backend = "vibe".to_string();
    entry3.effective_model = Some("mistral-medium".to_string());
    entry3.review_verdict = Some("REJECT".to_string());
    entry3.validation_result = Some("failed".to_string());
    entry3.usage.actual_cost_usd = Some(0.5);

    let entries = vec![entry1, entry2, entry3];
    let grouped = super::build_grouped_summary(
        &entries,
        |entry| entry.effective_model.clone().unwrap_or_default(),
        |observed| observed.model.unwrap_or_default().to_string(),
        |_backend, model| model.unwrap_or_default().to_string(),
        false,
    );

    assert!(grouped.is_some());
    let grouped = grouped.unwrap();
    assert_eq!(grouped.len(), 2); // gpt-4 and mistral-medium

    // Find gpt-4 group
    let gpt4_group = grouped.iter().find(|g| g.group_key == "gpt-4").unwrap();
    assert_eq!(gpt4_group.entries, 2);
    assert_eq!(gpt4_group.validation_pass, 2);
    assert_eq!(
        gpt4_group.review_verdict_distribution.get("APPROVE"),
        Some(&2)
    );
    assert!((gpt4_group.total_cost_usd.unwrap() - 3.0).abs() < f64::EPSILON);
    assert!((gpt4_group.average_cost_usd.unwrap() - 1.5).abs() < f64::EPSILON);
    assert!((gpt4_group.cost_per_approve_strong.unwrap() - 1.5).abs() < f64::EPSILON);

    // Find mistral-medium group
    let mistral_group = grouped
        .iter()
        .find(|g| g.group_key == "mistral-medium")
        .unwrap();
    assert_eq!(mistral_group.entries, 1);
    assert_eq!(mistral_group.validation_pass, 0);
    assert_eq!(
        mistral_group.review_verdict_distribution.get("REJECT"),
        Some(&1)
    );
    assert!((mistral_group.total_cost_usd.unwrap() - 0.5).abs() < f64::EPSILON);
}

#[test]
fn cost_per_approve_strong_keys_on_reviewer_tier_not_verdict_text() {
    // Issue #214: cost_per_approve_strong must count only *strong-tier*
    // APPROVE verdicts, keyed on the persisted `reviewer_tier` field. A
    // weak-tier or unknown-tier APPROVE must not be folded into the
    // strong metric (and an unknown tier must stay unknown -- never
    // coerced to strong).
    let (_tmp, _cfg) = test_config();
    let mut strong = LedgerEntry::new("test", &profile(), "codex", "improve", "t1", None, None);
    strong.effective_backend = "codex".to_string();
    strong.effective_model = Some("gpt-4".to_string());
    strong.review_verdict = Some("APPROVE".to_string());
    strong.reviewer_tier = Some("strong".to_string());
    strong.validation_result = Some("passed".to_string());
    strong.usage.actual_cost_usd = Some(2.0);

    let mut weak = LedgerEntry::new("test", &profile(), "codex", "improve", "t2", None, None);
    weak.effective_backend = "codex".to_string();
    weak.effective_model = Some("gpt-4".to_string());
    weak.review_verdict = Some("APPROVE".to_string());
    weak.reviewer_tier = Some("weak".to_string());
    weak.validation_result = Some("passed".to_string());
    weak.usage.actual_cost_usd = Some(4.0);

    let mut unknown = LedgerEntry::new("test", &profile(), "codex", "improve", "t3", None, None);
    unknown.effective_backend = "codex".to_string();
    unknown.effective_model = Some("gpt-4".to_string());
    unknown.review_verdict = Some("APPROVE".to_string());
    unknown.reviewer_tier = None;
    unknown.validation_result = Some("passed".to_string());
    unknown.usage.actual_cost_usd = Some(8.0);

    let entries = vec![strong, weak, unknown];
    let grouped = super::build_grouped_summary(
        &entries,
        |entry| entry.effective_model.clone().unwrap_or_default(),
        |observed| observed.model.unwrap_or_default().to_string(),
        |_backend, model| model.unwrap_or_default().to_string(),
        false,
    )
    .unwrap();

    let gpt4 = grouped.iter().find(|g| g.group_key == "gpt-4").unwrap();
    // Total grouped cost is 2 + 4 + 8 = 14 (all entries), but the
    // denominator of cost_per_approve_strong is only the single strong-tier
    // APPROVE -- the weak/unknown APPROVEs are excluded from the count, so
    // the metric reflects the real per-strong-approval cost (14.0), not a
    // diluted per-any-approval figure.
    assert!((gpt4.total_cost_usd.unwrap() - 14.0).abs() < f64::EPSILON);
    assert_eq!(gpt4.review_verdict_distribution.get("APPROVE"), Some(&3));
    assert!((gpt4.cost_per_approve_strong.unwrap() - 14.0).abs() < f64::EPSILON);
}

// Issue #206: an account-level quota observation (backend-scoped,
// model = None) must surface in the backend-grouped view, and must NOT
// leak into the model-grouped view where the group key is a model name.
#[test]
fn account_quota_merges_into_backend_group_only() {
    let (_tmp, _cfg) = test_config();
    let mut entry = LedgerEntry::new("test", &profile(), "codex", "improve", "test1", None, None);
    entry.effective_backend = "codex".to_string();
    entry.effective_model = Some("gpt-5".to_string());
    let entries = vec![entry];

    let account = crate::quota_store::QuotaObservationRecord {
        backend: "codex".to_string(),
        model: None,
        quota_window: Some("weekly".to_string()),
        quota_used_percent: Some(42.0),
        quota_remaining_percent: Some(58.0),
        quota_reset_at: Some("2026-07-12T00:00:00Z".to_string()),
        observed_at: Some("2026-07-10T00:00:00Z".to_string()),
        usage_source: Some("codex status --json".to_string()),
    };
    let observations = vec![account];

    // Backend-grouped: the account observation must appear on the codex row.
    let backend_grouped = super::build_grouped_summary_with_account_quota(
        &entries,
        |entry| entry.effective_backend.clone(),
        |observed| observed.backend.to_string(),
        |backend, _model| backend.to_string(),
        true,
        &observations,
    )
    .unwrap();
    let codex_group = backend_grouped
        .iter()
        .find(|g| g.group_key == "codex")
        .unwrap();
    assert!(
        codex_group
            .quota_observations
            .iter()
            .any(|q| q.quota_used_percent == Some(42.0)),
        "backend-grouped view should surface the account quota observation"
    );

    // Model-grouped: the account observation must NOT show up (the group
    // key "gpt-5" is a model name, not a backend).
    let model_grouped = super::build_grouped_summary_with_account_quota(
        &entries,
        |entry| entry.effective_model.clone().unwrap_or_default(),
        |observed| observed.model.unwrap_or_default().to_string(),
        |_backend, model| model.unwrap_or_default().to_string(),
        false,
        &observations,
    )
    .unwrap();
    let gpt5_group = model_grouped
        .iter()
        .find(|g| g.group_key == "gpt-5")
        .unwrap();
    assert!(
        !gpt5_group
            .quota_observations
            .iter()
            .any(|q| q.quota_used_percent == Some(42.0)),
        "model-grouped view must not leak the backend-scoped account quota observation"
    );
}

#[test]
fn build_grouped_summary_empty_entries() {
    let entries: Vec<LedgerEntry> = vec![];
    let grouped = super::build_grouped_summary(
        &entries,
        |entry| entry.effective_backend.clone(),
        |observed| observed.backend.to_string(),
        |backend, _model| backend.to_string(),
        true,
    );
    assert!(grouped.is_none());
}

/// Issue #240 acceptance #2: a legacy (pre-tracking) fixture ledger must
/// surface attempt counters as *unknown* in the summary JSON, not as 0,
/// and must count the unknown entries separately.
#[test]
fn legacy_fixture_summary_surfaces_attempts_as_unknown() {
    let (_tmp, cfg) = test_config();
    let legacy = r#"{"timestamp":"2026-07-10T00:00:00Z","profile":"test","display_name":"R","repo_id":"r","repo":"o/r","local_path":"/tmp","provider":"github","backend":"codex","requested_backend":"codex","effective_backend":"codex","mode":"fix","commit_attempted":false,"commit_created":false,"push_attempted":false,"push_succeeded":false,"mr_attempted":false,"mr_created":false,"fallback_used":false,"human_required":false,"attempts":[],"usage":{}}"#;
    let known = r#"{"timestamp":"2026-07-10T00:00:01Z","schema_version":2,"profile":"test","display_name":"R","repo_id":"r","repo":"o/r","local_path":"/tmp","provider":"github","backend":"codex","requested_backend":"codex","effective_backend":"codex","mode":"fix","commit_attempted":false,"commit_created":false,"push_attempted":false,"push_succeeded":false,"mr_attempted":false,"mr_created":false,"fallback_used":false,"human_required":false,"attempts_started":2,"attempts_completed":1,"attempts":[],"usage":{}}"#;
    let path = cfg.defaults.ledger_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("{legacy}\n{known}\n")).unwrap();

    let data = super::build_summary(&cfg, "7d", None, GroupBy::None).unwrap();
    // Unknown (legacy) entry must not be coerced to 0 in the total sum.
    assert_eq!(
        data.attempts_started,
        Some(2),
        "legacy unknown entry must be excluded from the known sum"
    );
    assert_eq!(data.attempts_completed, Some(1));
    // Exactly one entry (the legacy one) is unknown.
    assert_eq!(data.attempts_started_unknown, 1);
    assert_eq!(data.attempts_completed_unknown, 1);
}

/// Issue #240 acceptance #2 (grouped view): unknown attempt counters are
/// excluded from the group sum and counted in `*-unknown`, while mixed
/// known values still aggregate correctly.
#[test]
fn legacy_fixture_grouped_summary_separates_unknown() {
    let legacy: LedgerEntry = serde_json::from_str(
        r#"{"timestamp":"2026-07-10T00:00:00Z","profile":"test","display_name":"R","repo_id":"r","repo":"o/r","local_path":"/tmp","provider":"github","backend":"codex","requested_backend":"codex","effective_backend":"codex","mode":"fix","commit_attempted":false,"commit_created":false,"push_attempted":false,"push_succeeded":false,"mr_attempted":false,"mr_created":false,"fallback_used":false,"human_required":false,"attempts":[],"usage":{}}"#,
    )
    .unwrap();
    let known: LedgerEntry = serde_json::from_str(
        r#"{"timestamp":"2026-07-10T00:00:01Z","schema_version":2,"profile":"test","display_name":"R","repo_id":"r","repo":"o/r","local_path":"/tmp","provider":"github","backend":"codex","requested_backend":"codex","effective_backend":"codex","mode":"fix","commit_attempted":false,"commit_created":false,"push_attempted":false,"push_succeeded":false,"mr_attempted":false,"mr_created":false,"fallback_used":false,"human_required":false,"attempts_started":2,"attempts_completed":1,"attempts":[],"usage":{}}"#,
    )
    .unwrap();

    let grouped = super::build_grouped_summary(
        &[legacy, known],
        |entry| entry.effective_backend.clone(),
        |observed| observed.backend.to_string(),
        |backend, _model| backend.to_string(),
        true,
    )
    .unwrap();
    let group = grouped.iter().find(|g| g.group_key == "codex").unwrap();
    assert_eq!(group.attempts_started, Some(2));
    assert_eq!(group.attempts_completed, Some(1));
    assert_eq!(group.attempts_started_unknown, 1);
    assert_eq!(group.attempts_completed_unknown, 1);
}
