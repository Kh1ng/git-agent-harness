use crate::config::{self, GahConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

/// TICKET-070: `json = true` prints one JSON array to stdout and nothing
/// else (the existing human-readable output is otherwise unchanged when
/// `json = false`) -- Hermes and other automation should never have to
/// parse pretty-printed terminal output to learn MR state.
pub fn run(cfg: &GahConfig, profile_name: &str, json: bool) -> Result<()> {
    let profile = config::get_profile(cfg, profile_name)?;
    let mrs = fetch_mrs(profile)?;

    if json {
        let rows: Vec<SyncMrJson> = mrs
            .iter()
            .map(|mr| SyncMrJson {
                profile: Some(profile_name.to_string()),
                branch: mr.branch.clone(),
                work_id: mr.work_id.clone(),
                id: mr.id.clone(),
                url: mr.url.clone(),
                state: mr.state.clone(),
                draft: mr.draft,
                merge_status: mr.merge_status.clone(),
                merged: mr.merged,
                ci_passed: mr.ci_passed,
                classification: classify(mr).to_string(),
                recommended_action: RecommendedAction::from_class(classify(mr)),
            })
            .collect();
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }

    println!("Profile: {}", profile_name);
    for mr in &mrs {
        let class = classify(mr);
        println!(
            "{}  {}  {}  {}",
            class,
            mr.branch,
            mr.title,
            mr.url.as_deref().unwrap_or("")
        );
        println!("  recommended: {}", recommended_action(class));
    }
    Ok(())
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecommendedAction {
    ReuseBranch,
    HumanMergeDecision,
    RunReview,
    None,
    InspectBranch,
    InspectManually,
}

impl RecommendedAction {
    pub fn from_class(class: &str) -> Self {
        match class {
            "CI_FAILED" | "NEEDS_FIX" => RecommendedAction::ReuseBranch,
            "READY_FOR_HUMAN" => RecommendedAction::HumanMergeDecision,
            "NEEDS_REVIEW" => RecommendedAction::RunReview,
            "MERGED" | "CLOSED_UNMERGED" => RecommendedAction::None,
            "STALE" => RecommendedAction::InspectBranch,
            _ => RecommendedAction::InspectManually,
        }
    }
}

/// Machine-readable row for `gah sync --json`. Field set is the ticket's
/// "at minimum" list: profile, branch, MR identifier, MR URL, state, draft,
/// merge status if available, classification, recommended next action.
#[derive(Debug, Serialize, Clone)]
pub struct SyncMrJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    pub id: Option<String>,
    pub url: Option<String>,
    pub state: Option<String>,
    pub draft: bool,
    pub merge_status: Option<String>,
    pub merged: bool,
    pub ci_passed: bool,
    pub classification: String,
    pub recommended_action: RecommendedAction,
}

#[derive(Debug, Clone)]
pub struct SyncMr {
    pub title: String,
    pub body: Option<String>,
    pub branch: String,
    pub labels: Vec<String>,
    pub url: Option<String>,
    pub id: Option<String>,
    pub state: Option<String>,
    pub draft: bool,
    pub merge_status: Option<String>,
    pub merged: bool,
    pub updated_at: Option<String>,
    pub ci_failed: bool,
    /// True only when CI has *conclusively* passed (every check/pipeline
    /// terminal and green) -- distinct from `!ci_failed`, which is also
    /// true while CI is still pending/running or absent entirely. Gates
    /// auto-merge (TICKET-127): merging on "not failed yet" would merge
    /// mid-pipeline.
    pub ci_passed: bool,
    /// TICKET-096: populated from an authoritative `TICKET-NNN` token in
    /// the PR/MR title (see `build_mr_title` in dispatch.rs), not from a
    /// separate reconciliation structure.
    pub work_id: Option<String>,
}

/// TICKET-096 AC2: extract a `TICKET-NNN` work ID from a PR/MR title where
/// one is present. No attempt to disambiguate authoritative vs stale IDs
/// here -- that check already happened when the title was generated.
fn extract_work_id_from_title(title: &str) -> Option<String> {
    let idx = title.find("TICKET-")?;
    let rest = &title[idx + "TICKET-".len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(format!("TICKET-{digits}"))
    }
}

pub fn fetch_mrs(profile: &config::Profile) -> Result<Vec<SyncMr>> {
    match profile.provider.as_str() {
        "github" => github_prs(profile),
        "gitlab" => gitlab_mrs(profile),
        other => anyhow::bail!("unsupported provider: {}", other),
    }
}

pub fn classify(mr: &SyncMr) -> &'static str {
    if mr.merged {
        return "MERGED";
    }
    if is_closed_unmerged(mr.state.as_deref(), mr.merged) {
        return "CLOSED_UNMERGED";
    }
    if mr.ci_failed {
        return "CI_FAILED";
    }
    if mr.labels.iter().any(|l| l == "gah-needs-fix") {
        return "NEEDS_FIX";
    }
    if mr.labels.iter().any(|l| l == "gah-ready-for-human") {
        return "READY_FOR_HUMAN";
    }
    if mr
        .labels
        .iter()
        .any(|l| l == "gah-human-review" || l == "gah-review-weak")
    {
        return "NEEDS_REVIEW";
    }
    if is_stale(mr.updated_at.as_deref()) {
        return "STALE";
    }
    if mr.branch.starts_with("gah/") {
        return "NEEDS_REVIEW";
    }
    "UNKNOWN"
}

pub fn recommended_action(class: &str) -> &'static str {
    match class {
        "CI_FAILED" => "reuse same branch/MR for a fix run",
        "NEEDS_FIX" => "reuse same branch/MR for a fix run",
        "READY_FOR_HUMAN" => "human review and merge decision",
        "NEEDS_REVIEW" => "run review or request human review",
        "MERGED" => "none",
        "CLOSED_UNMERGED" => "none",
        "STALE" => "inspect before reusing branch",
        _ => "inspect manually",
    }
}

fn is_closed_unmerged(state: Option<&str>, merged: bool) -> bool {
    !merged && matches!(state.map(|s| s.to_ascii_lowercase()), Some(ref s) if s == "closed")
}

fn is_stale(updated_at: Option<&str>) -> bool {
    let Some(updated_at) = updated_at else {
        return false;
    };
    let cutoff = (OffsetDateTime::now_utc() - Duration::days(14))
        .format(&Rfc3339)
        .unwrap_or_default();
    updated_at < cutoff.as_str()
}

#[derive(Debug, Deserialize, PartialEq)]
struct GithubPr {
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    labels: Vec<GithubLabel>,
    #[serde(default)]
    number: Option<i64>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(default)]
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(default)]
    #[serde(rename = "mergedAt")]
    merged_at: Option<String>,
    #[serde(default)]
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(default)]
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<Vec<GithubCheck>>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct GithubLabel {
    name: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct GithubCheck {
    #[serde(default)]
    conclusion: Option<String>,
}

fn github_prs(profile: &crate::config::Profile) -> Result<Vec<SyncMr>> {
    let out = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            &profile.repo,
            "--state",
            "all",
            "--json",
            "title,body,headRefName,url,labels,number,state,isDraft,mergeStateStatus,mergedAt,updatedAt,statusCheckRollup",
        ])
        .output()
        .context("gh pr list")?;
    if !out.status.success() {
        anyhow::bail!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let prs: Vec<GithubPr> = serde_json::from_slice(&out.stdout)?;
    Ok(prs
        .into_iter()
        .filter(|pr| pr.head_ref_name.starts_with("gah/"))
        .map(|pr| SyncMr {
            work_id: extract_work_id_from_title(&pr.title),
            title: pr.title,
            body: pr.body,
            branch: pr.head_ref_name,
            labels: pr.labels.into_iter().map(|l| l.name).collect(),
            url: pr.url,
            id: pr.number.map(|n| n.to_string()),
            state: pr.state,
            draft: pr.is_draft,
            merge_status: pr.merge_state_status,
            merged: pr.merged_at.is_some(),
            updated_at: pr.updated_at,
            ci_failed: github_ci_failed(pr.status_check_rollup.as_deref()),
            ci_passed: github_ci_passed(pr.status_check_rollup.as_deref()),
        })
        .collect())
}

fn github_ci_failed(checks: Option<&[GithubCheck]>) -> bool {
    let checks = checks.unwrap_or_default();
    checks
        .iter()
        .any(|check| check.conclusion.as_deref() == Some("FAILURE"))
}

/// Every check present, all terminal, none failed -- as opposed to
/// `!github_ci_failed`, which is also true while checks are still pending
/// (`conclusion == None`) or absent altogether.
fn github_ci_passed(checks: Option<&[GithubCheck]>) -> bool {
    let checks = checks.unwrap_or_default();
    !checks.is_empty()
        && checks.iter().all(|check| {
            matches!(
                check.conclusion.as_deref(),
                Some("SUCCESS") | Some("NEUTRAL") | Some("SKIPPED")
            )
        })
}

#[derive(Debug, Deserialize)]
struct GitlabMr {
    title: String,
    #[serde(default)]
    description: Option<String>,
    source_branch: String,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    iid: Option<serde_json::Value>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    detailed_merge_status: Option<String>,
    #[serde(default)]
    merge_status: Option<String>,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    head_pipeline: Option<GitlabPipeline>,
}

#[derive(Debug, Deserialize)]
struct GitlabPipeline {
    #[serde(default)]
    status: Option<String>,
}

fn gitlab_mrs(profile: &crate::config::Profile) -> Result<Vec<SyncMr>> {
    let out = Command::new("glab")
        .args([
            "mr",
            "list",
            "--repo",
            &profile.repo,
            "--all",
            "--output",
            "json",
        ])
        .output()
        .context("glab mr list")?;
    if !out.status.success() {
        anyhow::bail!(
            "glab mr list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mrs: Vec<GitlabMr> = serde_json::from_slice(&out.stdout)?;
    Ok(mrs
        .into_iter()
        .filter(|mr| mr.source_branch.starts_with("gah/"))
        .map(|mr| {
            // Previously always `false` -- GitLab CI failures were
            // invisible to classify()/decide_next_action, so a red
            // pipeline never triggered an automatic FixMr. `head_pipeline`
            // is GitLab's own terminal-status field for the MR's latest
            // pipeline (success/failed/canceled/running/pending/...).
            let pipeline_status = mr.head_pipeline.as_ref().and_then(|p| p.status.as_deref());
            SyncMr {
                work_id: extract_work_id_from_title(&mr.title),
                title: mr.title,
                body: mr.description,
                branch: mr.source_branch,
                labels: mr.labels,
                url: mr.web_url,
                id: mr.iid.map(|v| v.to_string().trim_matches('"').to_string()),
                state: mr.state,
                draft: mr.draft,
                merge_status: mr.detailed_merge_status.or(mr.merge_status),
                merged: mr.merged_at.is_some(),
                updated_at: mr.updated_at,
                ci_failed: gitlab_ci_failed(pipeline_status),
                ci_passed: gitlab_ci_passed(pipeline_status),
            }
        })
        .collect())
}

fn gitlab_ci_failed(pipeline_status: Option<&str>) -> bool {
    matches!(pipeline_status, Some("failed") | Some("canceled"))
}

fn gitlab_ci_passed(pipeline_status: Option<&str>) -> bool {
    pipeline_status == Some("success")
}

/// Used to implement the retry cap for FixMr actions on existing branches.
/// Counts ONLY ledger entries with `dispatch_reason == "post_review_repair"`
/// — internal OpenHands retries within a single dispatch (attempts_started)
/// do NOT consume retry budget.  This prevents a ticket that needed 2
/// internal attempts to pass validation from being blocked from its first
/// post-review fix before the review even happens.
pub fn count_fix_attempts_per_branch(cfg: &GahConfig) -> std::collections::HashMap<String, usize> {
    use std::collections::HashMap;

    let entries = match crate::ledger::read_entries(cfg) {
        Ok(entries) => entries,
        Err(_) => return HashMap::new(),
    };

    let mut counts = HashMap::new();

    for entry in entries {
        if entry.mode == "fix"
            && entry
                .dispatch_reason
                .as_deref()
                .is_some_and(|r| r == "post_review_repair")
        {
            if let Some(branch) = &entry.branch {
                *counts.entry(branch.clone()).or_insert(0) += 1;
            }
        }
    }

    counts
}

/// Used to cap auto-merge retries (TICKET-127): a merge that fails
/// (conflicts, unresolved discussions, external checks) would otherwise be
/// re-attempted every loop iteration forever. Mirrors
/// `count_fix_attempts_per_branch` exactly, filtered to `mode == "merge"`.
pub fn count_merge_attempts_per_branch(
    cfg: &GahConfig,
) -> std::collections::HashMap<String, usize> {
    use std::collections::HashMap;

    let entries = match crate::ledger::read_entries(cfg) {
        Ok(entries) => entries,
        Err(_) => return HashMap::new(),
    };

    let mut counts = HashMap::new();

    for entry in entries {
        if entry.mode == "merge" && entry.attempts_started > 0 {
            if let Some(branch) = &entry.branch {
                *counts.entry(branch.clone()).or_insert(0) += entry.attempts_started as usize;
            }
        }
    }

    counts
}

#[cfg(test)]
mod tests {
    use super::{
        classify, extract_work_id_from_title, github_ci_failed, github_ci_passed, gitlab_ci_failed,
        gitlab_ci_passed, recommended_action, GithubCheck, GithubPr, SyncMr,
    };

    #[test]
    fn gitlab_ci_status_only_passed_on_explicit_success() {
        assert!(gitlab_ci_passed(Some("success")));
        assert!(!gitlab_ci_passed(Some("running")));
        assert!(!gitlab_ci_passed(Some("pending")));
        assert!(!gitlab_ci_passed(None));
    }

    #[test]
    fn gitlab_ci_status_failed_on_failed_or_canceled() {
        assert!(gitlab_ci_failed(Some("failed")));
        assert!(gitlab_ci_failed(Some("canceled")));
        assert!(!gitlab_ci_failed(Some("success")));
        assert!(!gitlab_ci_failed(Some("running")));
        // Absent pipeline is "unknown", not "failed" -- matches the
        // pre-existing (if previously hardcoded) semantics.
        assert!(!gitlab_ci_failed(None));
    }

    fn check(conclusion: Option<&str>) -> GithubCheck {
        GithubCheck {
            conclusion: conclusion.map(str::to_string),
        }
    }

    #[test]
    fn github_ci_passed_requires_at_least_one_check_all_terminal_and_green() {
        assert!(github_ci_passed(Some(&[check(Some("SUCCESS"))])));
        assert!(github_ci_passed(Some(&[
            check(Some("SUCCESS")),
            check(Some("SKIPPED"))
        ])));
        // No checks at all must not read as "passed" -- a repo with no CI
        // configured shouldn't silently qualify for auto-merge.
        assert!(!github_ci_passed(Some(&[])));
        // Still running (conclusion not yet set) is not passed.
        assert!(!github_ci_passed(Some(&[
            check(Some("SUCCESS")),
            check(None)
        ])));
        assert!(!github_ci_passed(Some(&[check(Some("FAILURE"))])));
    }

    #[test]
    fn github_ci_failed_on_any_failure_conclusion() {
        assert!(github_ci_failed(Some(&[
            check(Some("SUCCESS")),
            check(Some("FAILURE"))
        ])));
        assert!(!github_ci_failed(Some(&[check(Some("SUCCESS"))])));
        assert!(!github_ci_failed(Some(&[check(None)])));
    }

    #[test]
    fn github_status_check_rollup_handles_null_explicitly() {
        // Test that explicit null deserializes correctly
        let json_with_null =
            r#"{"title":"Test","headRefName":"gah/test","statusCheckRollup":null}"#;
        let pr: GithubPr = serde_json::from_str(json_with_null).unwrap();
        assert_eq!(pr.status_check_rollup, None);

        // Test that missing field deserializes correctly
        let json_without_field = r#"{"title":"Test","headRefName":"gah/test"}"#;
        let pr: GithubPr = serde_json::from_str(json_without_field).unwrap();
        assert_eq!(pr.status_check_rollup, None);

        // Test that populated array deserializes correctly
        let json_with_checks = r#"{"title":"Test","headRefName":"gah/test","statusCheckRollup":[{"conclusion":"SUCCESS"}]}"#;
        let pr: GithubPr = serde_json::from_str(json_with_checks).unwrap();
        assert!(pr.status_check_rollup.is_some());
        assert_eq!(pr.status_check_rollup.as_ref().unwrap().len(), 1);

        // Test that empty array deserializes correctly
        let json_with_empty = r#"{"title":"Test","headRefName":"gah/test","statusCheckRollup":[]}"#;
        let pr: GithubPr = serde_json::from_str(json_with_empty).unwrap();
        assert!(pr.status_check_rollup.is_some());
        assert_eq!(pr.status_check_rollup.as_ref().unwrap().len(), 0);
    }

    #[test]
    fn github_ci_functions_handle_none() {
        // Test that None (null or missing) is treated as empty for CI functions
        assert!(!github_ci_failed(None));
        assert!(!github_ci_passed(None));
    }

    fn base_mr() -> SyncMr {
        SyncMr {
            title: "x".into(),
            body: None,
            branch: "gah/test".into(),
            labels: vec![],
            url: None,
            id: None,
            state: Some("OPEN".into()),
            draft: false,
            merge_status: None,
            merged: false,
            updated_at: None,
            ci_failed: false,
            ci_passed: false,
            work_id: None,
        }
    }

    #[test]
    fn work_id_extracted_from_authoritative_title() {
        assert_eq!(
            extract_work_id_from_title("[GAH] Fix: TICKET-093 Derive PR titles"),
            Some("TICKET-093".to_string())
        );
    }

    #[test]
    fn work_id_absent_when_title_has_no_ticket_token() {
        assert_eq!(extract_work_id_from_title("[GAH] Fix: gah-real-1234"), None);
    }

    #[test]
    fn ready_label_maps_to_ready_for_human() {
        let mut mr = base_mr();
        mr.labels = vec!["gah-ready-for-human".into()];
        assert_eq!(classify(&mr), "READY_FOR_HUMAN");
    }

    #[test]
    fn closed_unmerged_takes_precedence_over_labels_and_other_state() {
        let mut mr = base_mr();
        mr.state = Some("closed".into());
        mr.draft = true;
        mr.merge_status = Some("DIRTY".into());
        mr.labels = vec!["gah-ready-for-human".into(), "gah-human-review".into()];
        mr.ci_failed = true;

        assert_eq!(classify(&mr), "CLOSED_UNMERGED");
        assert_eq!(recommended_action(classify(&mr)), "none");
    }

    #[test]
    fn merged_still_wins_over_closed_state() {
        let mut mr = base_mr();
        mr.state = Some("closed".into());
        mr.merged = true;

        assert_eq!(classify(&mr), "MERGED");
    }

    // ===== Bug 1: count_fix_attempts_per_branch counting semantics =====
    // These tests prove the retry cap counts ONLY actual post-review repair
    // dispatches, not internal OpenHands retries or initial dispatches.

    fn test_cfg_with_ledger(
        entries: &[crate::ledger::LedgerEntry],
    ) -> (
        crate::config::GahConfig,
        tempfile::TempDir,
        crate::test_support::LedgerEnvGuard,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = crate::config::GahConfig {
            defaults: crate::config::Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: tmp.path().to_string_lossy().into_owned(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: crate::config::RoutingPolicy::default(),
            },
            profiles: std::collections::HashMap::new(),
        };
        // GAH_LEDGER_PATH is a process-global env var that status tests set
        // during `cargo test`. Without this guard, parallel status tests can
        // redirect our ledger reads/writes to their tempdir. The guard sets
        // the env var to our tempdir's ledger file and restores it on drop.
        // Must be held for the entire test (including the read in
        // count_fix_attempts_per_branch), so it's returned to the caller.
        let guard = crate::test_support::LedgerEnvGuard::set(tmp.path().join("ledger.jsonl"));
        for entry in entries {
            crate::ledger::append(&cfg, entry).unwrap();
        }
        (cfg, tmp, guard)
    }

    fn ledger_entry(
        mode: &str,
        branch: &str,
        dispatch_reason: Option<&str>,
        attempts_started: u32,
    ) -> crate::ledger::LedgerEntry {
        let tmp = tempfile::tempdir().unwrap();
        let prof = crate::config::Profile {
            display_name: "test".into(),
            repo_id: "test".into(),
            repo: "test".into(),
            provider: String::new(),
            local_path: tmp.path().display().to_string(),
            artifact_root: tmp.path().display().to_string(),
            default_target_branch: "main".into(),
            provider_api_base: None,
            provider_project_id: None,
            oh_profile: None,
            openhands_args: vec![],
            codex_args: vec![],
            codex_path: None,
            claude_args: vec![],
            claude_path: None,
            agy_path: None,
            vibe_args: vec![],
            vibe_path: None,
            opencode_args: vec![],
            opencode_path: None,
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
            notify_command: None,
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            auto_fix_commands: vec![],
            test_file_patterns: vec![],
            known_baseline_failure_markers: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            review_timeout_seconds: None,
            routing: crate::config::RoutingPolicy::default(),
            pacing: crate::quota::PacingConfig::default(),
            publishing: Default::default(),
        };
        let mut entry =
            crate::ledger::LedgerEntry::new("test", &prof, "codex", mode, "x", None, None);
        entry.branch = Some(branch.into());
        entry.attempts_started = attempts_started;
        entry.dispatch_reason = dispatch_reason.map(|s| s.into());
        entry
    }

    /// Two internal OpenHands attempts inside one initial dispatch consume
    /// zero post-review repair retries.
    #[test]
    fn internal_retries_in_initial_dispatch_count_zero() {
        let (cfg, _tmp, _guard) =
            test_cfg_with_ledger(&[ledger_entry("fix", "branch-A", Some("initial"), 2)]);
        let counts = super::count_fix_attempts_per_branch(&cfg);
        assert_eq!(
            counts.get("branch-A").copied().unwrap_or(0),
            0,
            "internal retries in an initial dispatch must not consume repair budget"
        );
    }

    /// One actual post-review repair dispatch increments the count by exactly 1,
    /// regardless of how many internal attempts it used.
    #[test]
    fn one_repair_dispatch_counts_one() {
        let (cfg, _tmp, _guard) = test_cfg_with_ledger(&[
            ledger_entry("fix", "branch-A", Some("initial"), 2),
            ledger_entry("fix", "branch-A", Some("post_review_repair"), 3),
        ]);
        let counts = super::count_fix_attempts_per_branch(&cfg);
        assert_eq!(
            counts.get("branch-A").copied().unwrap_or(0),
            1,
            "one post_review_repair entry = count 1, not attempts_started"
        );
    }

    /// Internal retries within a FixMr (post_review_repair) dispatch do not
    /// inflate the repair-cycle count — it increments exactly once per entry.
    #[test]
    fn internal_retries_in_repair_dispatch_do_not_inflate() {
        let (cfg, _tmp, _guard) = test_cfg_with_ledger(&[ledger_entry(
            "fix",
            "branch-A",
            Some("post_review_repair"),
            5,
        )]);
        let counts = super::count_fix_attempts_per_branch(&cfg);
        assert_eq!(
            counts.get("branch-A").copied().unwrap_or(0),
            1,
            "attempts_started=5 within one repair dispatch must count as 1 repair cycle"
        );
    }

    /// Retry cap triggers only after the configured number of actual
    /// post-review repair cycles (AUTO_RETRY_CAP=2).
    #[test]
    fn two_repair_dispatches_hit_cap() {
        let (cfg, _tmp, _guard) = test_cfg_with_ledger(&[
            ledger_entry("fix", "branch-A", Some("initial"), 2),
            ledger_entry("fix", "branch-A", Some("post_review_repair"), 1),
            ledger_entry("fix", "branch-A", Some("post_review_repair"), 1),
        ]);
        let counts = super::count_fix_attempts_per_branch(&cfg);
        assert_eq!(
            counts.get("branch-A").copied().unwrap_or(0),
            2,
            "two post_review_repair entries = count 2 = AUTO_RETRY_CAP"
        );
    }

    /// Entries with mode != "fix" (review, merge) never count.
    #[test]
    fn review_and_merge_entries_never_count() {
        let (cfg, _tmp, _guard) = test_cfg_with_ledger(&[
            ledger_entry("review", "branch-A", Some("review"), 1),
            ledger_entry("merge", "branch-A", None, 1),
        ]);
        let counts = super::count_fix_attempts_per_branch(&cfg);
        assert!(
            counts.is_empty(),
            "review/merge entries must not count toward fix retry cap"
        );
    }

    /// Pre-existing ledger entries without dispatch_reason (legacy) must not
    /// count — they cannot be proven to be post-review repairs.
    #[test]
    fn legacy_entries_without_dispatch_reason_do_not_count() {
        let (cfg, _tmp, _guard) = test_cfg_with_ledger(&[ledger_entry("fix", "branch-A", None, 2)]);
        let counts = super::count_fix_attempts_per_branch(&cfg);
        assert_eq!(
            counts.get("branch-A").copied().unwrap_or(0),
            0,
            "legacy entries without dispatch_reason must not count (unprovable)"
        );
    }
}
