use crate::config::{self, GahConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

mod repository;
mod review_state;
pub use repository::fetch_repository_mrs;
pub(crate) use review_state::review_metadata_fingerprint;
pub(crate) use review_state::{
    current_review_generation, review_contract_matches, review_generation_matches_mr,
};
use review_state::{latest_review_for_mr, ledger_info_for_mr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MrFetchScope {
    Active,
    FullHistory,
}

/// TICKET-070: `json = true` prints one JSON array to stdout and nothing
/// else (the existing human-readable output is otherwise unchanged when
/// `json = false`) -- Hermes and other automation should never have to
/// parse pretty-printed terminal output to learn MR state.
pub fn run(cfg: &GahConfig, profile_name: &str, json: bool) -> Result<()> {
    let profile = config::get_profile(cfg, profile_name)?;
    let mrs = fetch_mrs(profile)?;

    if json {
        let ledger_index = crate::ledger::read_entries(cfg)
            .ok()
            .map(|e| crate::ledger::index_entries_by_work_id(&e))
            .unwrap_or_default();
        let rows: Vec<SyncMrJson> = mrs
            .iter()
            .map(|mr| sync_mr_to_json(mr, Some(profile_name.to_string()), &ledger_index))
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
    /// TICKET-198: human-readable PR/MR title, surfaced on the dashboard's
    /// Recently Merged panel instead of the raw branch name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// TICKET-198: merge timestamp (RFC3339) for merged MRs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
    /// TICKET-198: backend/model that produced the merge, from the ledger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_backend: Option<String>,
    /// TICKET-198: model that produced the merge, from the ledger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_model: Option<String>,
    /// TICKET-198: review verdict recorded for the merge, from the ledger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_gate_reason: Option<String>,
    /// Immutable source and independently-versioned lifecycle generation used
    /// to decide whether historical review-derived gates remain authoritative.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_sha: Option<String>,
    pub review_contract_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_generation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_generation_status: Option<String>,
    /// Non-terminal / unknown CI (running/pending/missing) for which there
    /// is no defined next controller action yet. GitHub is never `ci_pending`
    /// (see `SyncMr::ci_pending`); this mirrors the typed field on `SyncMr`.
    pub ci_pending: bool,
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
    /// Immutable source commit currently attached to the provider MR.
    pub source_sha: Option<String>,
    pub merge_status: Option<String>,
    pub merged: bool,
    pub updated_at: Option<String>,
    /// TICKET-198: merge timestamp (RFC3339) for merged MRs, surfaced on the
    /// dashboard's Recently Merged panel. Empty for unmerged MRs.
    pub merged_at: Option<String>,
    pub ci_failed: bool,
    /// True only when CI has *conclusively* passed (every check/pipeline
    /// terminal and green) -- distinct from `!ci_failed`, which is also
    /// true while CI is still pending/running or absent entirely. Gates
    /// auto-merge (TICKET-127): merging on "not failed yet" would merge
    /// mid-pipeline.
    pub ci_passed: bool,
    /// Issue #156: CI is non-terminal / unknown (running/pending/missing) --
    /// there is no conclusive pass or failure yet. Distinct from `!ci_passed`
    /// (which is also true once CI has *failed*). GitHub never sets this
    /// (its classification handles pending via `github_ci_passed`/`_failed`);
    /// only GitLab's `head_pipeline.status` gap populates it.
    pub ci_pending: bool,
    /// TICKET-096: populated from an authoritative `TICKET-NNN` token in
    /// the PR/MR title (see `build_mr_title` in dispatch.rs), not from a
    /// separate reconciliation structure.
    pub work_id: Option<String>,
}

/// Extract the canonical provider issue identity (`#NNN`) from new PR/MR
/// titles, while continuing to read legacy `TICKET-NNN` titles. No attempt
/// to disambiguate authoritative vs stale IDs happens here -- that check
/// already happened when the title was generated.
pub(crate) fn extract_work_id_from_title(title: &str) -> Option<String> {
    if let Some(issue_id) = title.split('#').skip(1).find_map(|rest| {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        (!digits.is_empty()).then(|| format!("#{digits}"))
    }) {
        return Some(issue_id);
    }

    if let Some(idx) = title.find("TICKET-") {
        let rest = &title[idx + "TICKET-".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return Some(format!("TICKET-{digits}"));
        }
    }
    None
}

pub fn fetch_mrs(profile: &config::Profile) -> Result<Vec<SyncMr>> {
    fetch_mrs_for_scope(profile, MrFetchScope::FullHistory, true)
}

/// Fetch only merge requests that can drive a controller action. Recurring
/// status/controller observations must use this bounded path; full history is
/// reserved for explicit synchronization, reconciliation, and pruning.
pub fn fetch_active_mrs(profile: &config::Profile) -> Result<Vec<SyncMr>> {
    fetch_mrs_for_scope(profile, MrFetchScope::Active, true)
}

fn fetch_mrs_for_scope(
    profile: &config::Profile,
    scope: MrFetchScope,
    filter_gah_branches: bool,
) -> Result<Vec<SyncMr>> {
    let mut mrs = match profile.provider.as_str() {
        "github" => github_prs(profile, scope, filter_gah_branches),
        "gitlab" => gitlab_mrs(profile, scope, filter_gah_branches),
        other => anyhow::bail!("unsupported provider: {}", other),
    }?;
    if scope == MrFetchScope::Active {
        mrs.retain(|mr| {
            mr.state.as_deref().is_some_and(|state| {
                state.eq_ignore_ascii_case("open") || state.eq_ignore_ascii_case("opened")
            }) && !mr.merged
        });
    }
    Ok(mrs)
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
        .any(|l| l == "gah-human-review" || l == "gah-review-weak" || l == "gah-review-escalating")
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

/// TICKET-198: build the machine-readable `SyncMrJson` row, enriching it with
/// the ledger-derived backend/model and review verdict for the merge (when a
/// `work_id` is available to join on). Centralized so `gah sync --json` and
/// `gah status --json` emit an identical shape.
pub fn sync_mr_to_json(
    mr: &SyncMr,
    profile: Option<String>,
    ledger: &crate::ledger::LedgerEntriesByWorkId,
) -> SyncMrJson {
    let mut class = classify(mr);
    let (effective_backend, effective_model, mut review_verdict, mut review_gate_reason) =
        ledger_info_for_mr(ledger, mr);
    let latest_review = latest_review_for_mr(ledger, mr);
    let latest_review_is_current =
        latest_review.is_some_and(|latest| review_contract_matches(latest, mr));
    let review_generation = if latest_review_is_current {
        latest_review.and_then(|entry| entry.review_generation.clone())
    } else {
        current_review_generation(mr)
    };
    let review_generation_status = latest_review.and_then(|latest| {
        (!latest_review_is_current).then(|| {
            if latest.review_contract_version
                != Some(crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION)
            {
                format!(
                    "superseded review contract {} -> {}",
                    latest
                        .review_contract_version
                        .map_or_else(|| "legacy".to_string(), |version| version.to_string()),
                    crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION
                )
            } else if latest.review_generation.is_none() {
                "superseded review missing generation identity".to_string()
            } else {
                "superseded review because source or provider metadata changed".to_string()
            }
        })
    });
    if !matches!(class, "MERGED" | "CLOSED_UNMERGED" | "CI_FAILED") {
        if let Some(latest) = latest_review {
            if !latest_review_is_current {
                // The provider label reflects an opinion about older
                // metadata or a superseded review contract. Both require a
                // fresh bounded review, not repair or a terminal human gate.
                class = "NEEDS_REVIEW";
                review_verdict = None;
                review_gate_reason = review_generation_status.clone();
            } else {
                // Provider label mutations can fail or lag after a completed
                // review. Reconcile from the exact-branch, exact-metadata
                // structured verdict so stale NEEDS_FIX labels cannot launch
                // a repair that preflight must reject.
                class = match latest.review_verdict.as_deref() {
                    Some("APPROVE") => "READY_FOR_HUMAN",
                    Some("NEEDS_FIX" | "REJECT") => "NEEDS_FIX",
                    Some("HUMAN_REVIEW") if latest.human_required => "READY_FOR_HUMAN",
                    Some("HUMAN_REVIEW") => "NEEDS_REVIEW",
                    Some("REVIEW_OUTPUT_INVALID") => "NEEDS_REVIEW",
                    _ => class,
                };
            }
        }
    }
    SyncMrJson {
        profile,
        branch: mr.branch.clone(),
        work_id: mr.work_id.clone(),
        id: mr.id.clone(),
        url: mr.url.clone(),
        title: Some(mr.title.clone()),
        state: mr.state.clone(),
        draft: mr.draft,
        merge_status: mr.merge_status.clone(),
        merged: mr.merged,
        merged_at: mr.merged_at.clone(),
        ci_passed: mr.ci_passed,
        ci_pending: mr.ci_pending,
        effective_backend,
        effective_model,
        review_verdict,
        review_gate_reason,
        source_sha: mr.source_sha.clone(),
        review_contract_version: crate::ledger::REVIEW_CONTRACT_VERSION,
        review_generation,
        review_generation_status,
        classification: class.to_string(),
        recommended_action: RecommendedAction::from_class(class),
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
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
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

fn github_pr_list_args(profile: &crate::config::Profile, scope: MrFetchScope) -> Vec<String> {
    let (state, limit) = match scope {
        MrFetchScope::Active => ("open", "100"),
        MrFetchScope::FullHistory => ("all", "1000"),
    };
    [
        "pr",
        "list",
        "--repo",
        &profile.repo,
        "--state",
        state,
        "--limit",
        limit,
        "--json",
        "title,body,headRefName,headRefOid,url,labels,number,state,isDraft,mergeStateStatus,mergedAt,updatedAt,statusCheckRollup",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn github_prs(
    profile: &crate::config::Profile,
    scope: MrFetchScope,
    filter_gah_branches: bool,
) -> Result<Vec<SyncMr>> {
    let out = crate::provider::provider_command("gh")
        .args(github_pr_list_args(profile, scope))
        .output()
        .context("gh pr list")?;
    if !out.status.success() {
        anyhow::bail!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let prs: Vec<GithubPr> = serde_json::from_slice(&out.stdout)?;
    if scope == MrFetchScope::Active && prs.len() >= 100 {
        anyhow::bail!(
            "gh pr list reached the active observation cap (100); refusing an incomplete snapshot"
        );
    }
    Ok(prs
        .into_iter()
        .filter(|pr| !filter_gah_branches || pr.head_ref_name.starts_with("gah/"))
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
            source_sha: pr.head_ref_oid,
            merge_status: pr.merge_state_status,
            merged: pr.merged_at.is_some(),
            updated_at: pr.updated_at,
            merged_at: pr.merged_at,
            ci_failed: github_ci_failed(pr.status_check_rollup.as_deref()),
            ci_passed: github_ci_passed(pr.status_check_rollup.as_deref()),
            ci_pending: false,
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
    sha: Option<String>,
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

fn gitlab_mr_list_args(profile: &crate::config::Profile, scope: MrFetchScope) -> Vec<String> {
    let mut args = vec![
        "mr".to_string(),
        "list".to_string(),
        "--repo".to_string(),
        profile.repo.clone(),
    ];
    match scope {
        MrFetchScope::Active => args.extend(["--per-page".into(), "100".into()]),
        MrFetchScope::FullHistory => args.push("--all".into()),
    }
    args.extend(["--output".into(), "json".into()]);
    args
}

fn gitlab_mrs(
    profile: &crate::config::Profile,
    scope: MrFetchScope,
    filter_gah_branches: bool,
) -> Result<Vec<SyncMr>> {
    let out = crate::provider::provider_command("glab")
        .args(gitlab_mr_list_args(profile, scope))
        .output()
        .context("glab mr list")?;
    if !out.status.success() {
        anyhow::bail!(
            "glab mr list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mrs: Vec<GitlabMr> = serde_json::from_slice(&out.stdout)?;
    let mut synced = Vec::new();
    for mr in mrs
        .into_iter()
        .filter(|mr| !filter_gah_branches || mr.source_branch.starts_with("gah/"))
    {
        let iid = mr
            .iid
            .as_ref()
            .map(|value| value.to_string().trim_matches('"').to_string());
        let mut pipeline_status = mr.head_pipeline.and_then(|pipeline| pipeline.status);
        if pipeline_status.is_none() && mr.state.as_deref() == Some("opened") {
            if let Some(iid) = iid.as_deref() {
                pipeline_status = gitlab_latest_pipeline_status(profile, iid)?;
            }
        }
        synced.push(SyncMr {
            work_id: extract_work_id_from_title(&mr.title),
            title: mr.title,
            body: mr.description,
            branch: mr.source_branch,
            labels: mr.labels,
            url: mr.web_url,
            id: iid,
            state: mr.state,
            draft: mr.draft,
            source_sha: mr.sha,
            merge_status: mr.detailed_merge_status.or(mr.merge_status),
            merged: mr.merged_at.is_some(),
            updated_at: mr.updated_at,
            merged_at: mr.merged_at,
            ci_failed: gitlab_ci_failed(pipeline_status.as_deref()),
            ci_passed: gitlab_ci_passed(pipeline_status.as_deref()),
            ci_pending: gitlab_ci_pending(pipeline_status.as_deref()),
        });
    }
    Ok(synced)
}

fn gitlab_latest_pipeline_status(
    profile: &crate::config::Profile,
    iid: &str,
) -> Result<Option<String>> {
    let project_id = profile
        .provider_project_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("profile missing provider_project_id for gitlab"))?;
    let endpoint = format!("projects/{project_id}/merge_requests/{iid}/pipelines");
    let response = crate::provider::gitlab_api(profile, &endpoint, "GET", &[("per_page", "1")])?;
    let pipelines = response.as_array().ok_or_else(|| {
        anyhow::anyhow!(
            "GitLab pipeline lookup returned a non-list response for MR !{}: {}",
            iid,
            crate::redact::redact(&response.to_string())
        )
    })?;
    Ok(pipelines
        .first()
        .and_then(|pipeline| pipeline.get("status"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
}

fn gitlab_ci_failed(pipeline_status: Option<&str>) -> bool {
    matches!(pipeline_status, Some("failed") | Some("canceled"))
}

fn gitlab_ci_passed(pipeline_status: Option<&str>) -> bool {
    pipeline_status == Some("success")
}

/// True when the GitLab pipeline status is *non-terminal / unknown* -- a
/// pipeline still `running`/`pending`, or no pipeline reported at all
/// (`head_pipeline` absent). Distinct from both `gitlab_ci_passed` and
/// `gitlab_ci_failed` so the controller can surface a visible "wait and
/// re-check" action instead of letting an MR silently no-op forever
/// (issue #156). `skipped`/`manual`/`created` are treated as pending too:
/// they are not a conclusive pass or failure and must not be merged.
fn gitlab_ci_pending(pipeline_status: Option<&str>) -> bool {
    matches!(
        pipeline_status,
        None | Some("running")
            | Some("pending")
            | Some("created")
            | Some("skipped")
            | Some("manual")
    )
}

/// Count only post-review repairs from the active source/metadata generation.
/// Internal backend retries within one dispatch still count as one repair
/// cycle, and `clear_attempts` tombstones reset the append-only history.
pub fn count_current_fix_attempts_for_mrs(
    entries: &[crate::ledger::LedgerEntry],
    profile: &str,
    repo_id: &str,
    merge_requests: &[SyncMrJson],
) -> std::collections::HashMap<String, usize> {
    let generations: std::collections::HashMap<&str, &str> = merge_requests
        .iter()
        .filter_map(|mr| Some((mr.branch.as_str(), mr.review_generation.as_deref()?)))
        .collect();
    count_branch_attempts(entries, Some((profile, repo_id)), |entry| {
        usize::from(
            entry.mode == "fix"
                && entry.dispatch_reason.as_deref() == Some("post_review_repair")
                && entry.review_contract_version
                    == Some(crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION)
                && entry
                    .review_generation
                    .as_deref()
                    .is_some_and(|generation| {
                        entry
                            .branch
                            .as_deref()
                            .and_then(|branch| generations.get(branch).copied())
                            == Some(generation)
                    }),
        )
    })
}

/// Used to cap auto-merge retries (TICKET-127): a merge that fails
/// (conflicts, unresolved discussions, external checks) would otherwise be
/// re-attempted every loop iteration forever.
pub fn count_merge_attempts_per_branch_for_scope(
    entries: &[crate::ledger::LedgerEntry],
    profile: &str,
    repo_id: &str,
) -> std::collections::HashMap<String, usize> {
    count_branch_attempts(entries, Some((profile, repo_id)), |entry| {
        if entry.mode == "merge" {
            entry.attempts_started.unwrap_or(0) as usize
        } else {
            0
        }
    })
}

fn count_branch_attempts(
    entries: &[crate::ledger::LedgerEntry],
    scope: Option<(&str, &str)>,
    attempt_weight: impl Fn(&crate::ledger::LedgerEntry) -> usize,
) -> std::collections::HashMap<String, usize> {
    use std::collections::{HashMap, HashSet};

    let in_scope = |entry: &crate::ledger::LedgerEntry| {
        scope.is_none_or(|(profile, repo_id)| entry.profile == profile && entry.repo_id == repo_id)
    };
    let mut counts = HashMap::new();
    let mut branches_by_work: HashMap<(String, String, String), HashSet<String>> = HashMap::new();

    for entry in entries.iter().filter(|entry| in_scope(entry)) {
        if entry.mode == "clear_attempts" {
            let Some(work_id) = entry.work_id.as_deref() else {
                continue;
            };
            let aliases = crate::ledger::work_id_aliases(work_id);
            let branches_to_reset: HashSet<String> = branches_by_work
                .iter()
                .filter(|((profile, repo_id, known_work_id), _)| {
                    profile == &entry.profile
                        && repo_id == &entry.repo_id
                        && aliases.iter().any(|alias| alias == known_work_id)
                })
                .flat_map(|(_, branches)| branches.iter().cloned())
                .collect();
            for branch in branches_to_reset {
                counts.remove(&branch);
            }
            continue;
        }

        // Reviews and other records often carry the authoritative issue ID
        // even when an older FixMr record used its branch as a synthetic ID.
        // Retain every attributable branch/work pairing so an operator reset
        // can also clear those historical repair counters.
        if let (Some(branch), Some(work_id)) = (&entry.branch, &entry.work_id) {
            branches_by_work
                .entry((
                    entry.profile.clone(),
                    entry.repo_id.clone(),
                    work_id.clone(),
                ))
                .or_default()
                .insert(branch.clone());
        }

        let weight = attempt_weight(entry);
        if weight == 0 {
            continue;
        }
        let Some(branch) = entry.branch.as_ref() else {
            continue;
        };
        *counts.entry(branch.clone()).or_insert(0) += weight;
    }

    counts
}

#[cfg(test)]
mod tests {
    use super::{
        classify, extract_work_id_from_title, github_ci_failed, github_ci_passed, gitlab_ci_failed,
        gitlab_ci_passed, gitlab_ci_pending, recommended_action, review_metadata_fingerprint,
        sync_mr_to_json, GithubCheck, GithubPr, GitlabMr, RecommendedAction, SyncMr,
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

    #[test]
    fn gitlab_ci_pending_on_missing_or_non_terminal_status() {
        // Issue #156: a missing or non-terminal pipeline must NOT collapse
        // into the same false/false bucket as a genuinely failed one -- it
        // needs its own distinct, observable classification.
        assert!(gitlab_ci_pending(None));
        assert!(gitlab_ci_pending(Some("running")));
        assert!(gitlab_ci_pending(Some("pending")));
        assert!(gitlab_ci_pending(Some("created")));
        assert!(gitlab_ci_pending(Some("skipped")));
        assert!(gitlab_ci_pending(Some("manual")));
        // Conclusive states are never "pending".
        assert!(!gitlab_ci_pending(Some("success")));
        assert!(!gitlab_ci_pending(Some("failed")));
        assert!(!gitlab_ci_pending(Some("canceled")));
        // The three states are mutually exclusive.
        let s = Some("running");
        assert!(gitlab_ci_pending(s) && !gitlab_ci_passed(s) && !gitlab_ci_failed(s));
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

        let with_source = r#"{"title":"Test","headRefName":"gah/test","headRefOid":"abc123"}"#;
        let pr: GithubPr = serde_json::from_str(with_source).unwrap();
        assert_eq!(pr.head_ref_oid.as_deref(), Some("abc123"));
    }

    #[test]
    fn gitlab_list_fixture_retains_source_sha_for_review_identity() {
        let mr: GitlabMr = serde_json::from_str(
            r#"{"title":"Test","source_branch":"gah/test","draft":true,"sha":"def456"}"#,
        )
        .unwrap();
        assert_eq!(mr.sha.as_deref(), Some("def456"));
        assert!(mr.draft);
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
            source_sha: None,
            merge_status: None,
            merged: false,
            updated_at: None,
            merged_at: None,
            ci_failed: false,
            ci_passed: false,
            ci_pending: false,
            work_id: None,
        }
    }

    fn set_review_identity(
        entry: &mut crate::ledger::LedgerEntry,
        mr: &SyncMr,
        fingerprint: Option<String>,
    ) {
        entry.review_source_sha = mr.source_sha.clone();
        entry.review_metadata_fingerprint = fingerprint;
        entry.review_contract_version = Some(crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION);
        entry.review_generation = crate::ledger::review_generation(
            entry.review_source_sha.as_deref(),
            entry.review_metadata_fingerprint.as_deref(),
        );
    }

    #[test]
    fn work_id_extracted_from_authoritative_title() {
        assert_eq!(
            extract_work_id_from_title("[GAH] Fix: TICKET-093 Derive PR titles"),
            Some("TICKET-093".to_string())
        );
    }

    #[test]
    fn work_id_extracted_from_native_issue_title() {
        assert_eq!(
            extract_work_id_from_title("[GAH] Fix: #319 Use native issue numbers"),
            Some("#319".to_string())
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
    fn provisional_review_escalation_label_maps_to_needs_review() {
        let mut mr = base_mr();
        mr.labels = vec!["gah-review-escalating".into()];
        assert_eq!(classify(&mr), "NEEDS_REVIEW");
    }

    #[test]
    fn sync_row_exposes_gate_reason_even_when_it_comes_from_review_entry() {
        let mut mr = base_mr();
        mr.source_sha = Some("source-sha".into());
        mr.work_id = Some("TICKET-295".into());
        let mut review = ledger_entry("review", "gah/test", Some("review"), Some(1));
        review.work_id = Some("TICKET-295".into());
        review.effective_backend = "claude".into();
        review.review_verdict = Some("HUMAN_REVIEW".into());
        review.review_gate_reason = Some("APPROVE omitted grounded evidence".into());
        set_review_identity(&mut review, &mr, Some(mr.review_metadata_fingerprint()));
        let mut ledger = crate::ledger::LedgerEntriesByWorkId::new();
        ledger.insert("TICKET-295".into(), vec![review]);

        let row = super::sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.review_verdict.as_deref(), Some("HUMAN_REVIEW"));
        assert_eq!(
            row.review_gate_reason.as_deref(),
            Some("APPROVE omitted grounded evidence")
        );
    }

    fn reviewed_needs_fix_row(
        fingerprint: Option<String>,
    ) -> (SyncMr, crate::ledger::LedgerEntriesByWorkId) {
        let mut mr = base_mr();
        mr.work_id = Some("#701".into());
        mr.source_sha = Some("source-sha".into());
        mr.body = Some("corrected metadata".into());
        mr.labels = vec!["gah-needs-fix".into()];

        let mut review = ledger_entry("review", "gah/test", Some("review"), Some(1));
        review.work_id = Some("#701".into());
        review.review_verdict = Some("NEEDS_FIX".into());
        review.validation_result = Some("NEEDS_FIX".into());
        set_review_identity(&mut review, &mr, fingerprint);
        review.review_blocking_findings = vec!["MR description omitted the finding".into()];

        let mut ledger = crate::ledger::LedgerEntriesByWorkId::new();
        ledger.insert("#701".into(), vec![review]);
        (mr, ledger)
    }

    #[test]
    fn metadata_only_correction_invalidates_needs_fix_and_schedules_review() {
        let old =
            review_metadata_fingerprint(Some("source-sha"), Some("x"), Some("old metadata"), false);
        let (mr, ledger) = reviewed_needs_fix_row(Some(old));

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "NEEDS_REVIEW");
        assert_eq!(row.recommended_action, RecommendedAction::RunReview);
        assert_eq!(row.review_verdict, None);
    }

    #[test]
    fn unchanged_review_metadata_preserves_needs_fix() {
        let (mr, mut ledger) = reviewed_needs_fix_row(None);
        set_review_identity(
            &mut ledger.get_mut("#701").unwrap()[0],
            &mr,
            Some(mr.review_metadata_fingerprint()),
        );

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "NEEDS_FIX");
        assert_eq!(row.recommended_action, RecommendedAction::ReuseBranch);
        assert_eq!(row.review_verdict.as_deref(), Some("NEEDS_FIX"));
    }

    #[test]
    fn backfilled_implementation_review_identity_is_valid_review_evidence() {
        let (mr, mut ledger) = reviewed_needs_fix_row(None);
        let entry = &mut ledger.get_mut("#701").unwrap()[0];
        entry.mode = "fix".into();
        set_review_identity(entry, &mr, Some(mr.review_metadata_fingerprint()));

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "NEEDS_FIX");
        assert_eq!(row.review_verdict.as_deref(), Some("NEEDS_FIX"));
    }

    #[test]
    fn stale_needs_fix_label_with_nonterminal_human_review_routes_to_review() {
        let (mr, mut ledger) = reviewed_needs_fix_row(None);
        let review = &mut ledger.get_mut("#701").unwrap()[0];
        review.review_verdict = Some("HUMAN_REVIEW".into());
        review.validation_result = Some("HUMAN_REVIEW".into());
        review.human_required = false;
        set_review_identity(review, &mr, Some(mr.review_metadata_fingerprint()));

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "NEEDS_REVIEW");
        assert_eq!(row.recommended_action, RecommendedAction::RunReview);
        assert_eq!(row.review_verdict.as_deref(), Some("HUMAN_REVIEW"));
    }

    #[test]
    fn invalid_review_output_cannot_select_fixmr_even_with_stale_needs_fix_label() {
        let (mr, mut ledger) = reviewed_needs_fix_row(None);
        let review = &mut ledger.get_mut("#701").unwrap()[0];
        review.review_verdict = Some("REVIEW_OUTPUT_INVALID".into());
        review.validation_result = Some("review_output_invalid".into());
        review.review_gate_reason = Some("finding explicitly withdrew itself".into());
        review.review_blocking_findings.clear();
        set_review_identity(review, &mr, Some(mr.review_metadata_fingerprint()));

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "NEEDS_REVIEW");
        assert_eq!(row.recommended_action, RecommendedAction::RunReview);
        assert_eq!(row.review_verdict.as_deref(), Some("REVIEW_OUTPUT_INVALID"));
        assert_eq!(
            row.review_gate_reason.as_deref(),
            Some("finding explicitly withdrew itself")
        );
    }

    #[test]
    fn terminal_human_review_hold_is_not_redispatched_or_auto_merged() {
        let (mr, mut ledger) = reviewed_needs_fix_row(None);
        let review = &mut ledger.get_mut("#701").unwrap()[0];
        review.review_verdict = Some("HUMAN_REVIEW".into());
        review.validation_result = Some("HUMAN_REVIEW".into());
        review.human_required = true;
        set_review_identity(review, &mr, Some(mr.review_metadata_fingerprint()));

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "READY_FOR_HUMAN");
        assert_eq!(
            row.recommended_action,
            RecommendedAction::HumanMergeDecision
        );
        assert_eq!(row.review_verdict.as_deref(), Some("HUMAN_REVIEW"));
    }

    #[test]
    fn metadata_rereview_supersedes_old_backfilled_verdict_without_losing_attribution() {
        let (mut mr, mut ledger) = reviewed_needs_fix_row(None);
        mr.labels = vec!["gah-ready-for-human".into()];
        mr.url = Some("https://example.test/mr/7".into());

        let implementation = &mut ledger.get_mut("#701").unwrap()[0];
        implementation.mode = "fix".into();
        implementation.mr_url = mr.url.clone();
        implementation.effective_backend = "hy3".into();
        implementation.effective_model = Some("worker-model".into());

        let mut rereview = ledger_entry("review", "gah/test", Some("review"), Some(2));
        rereview.work_id = Some("#701".into());
        rereview.mr_url = mr.url.clone();
        rereview.review_verdict = Some("APPROVE".into());
        rereview.validation_result = Some("APPROVE".into());
        set_review_identity(&mut rereview, &mr, Some(mr.review_metadata_fingerprint()));
        ledger.get_mut("#701").unwrap().push(rereview);

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "READY_FOR_HUMAN");
        assert_eq!(row.review_verdict.as_deref(), Some("APPROVE"));
        assert_eq!(row.effective_backend.as_deref(), Some("hy3"));
        assert_eq!(row.effective_model.as_deref(), Some("worker-model"));
    }

    #[test]
    fn historical_review_without_metadata_identity_fails_safe_to_rereview() {
        let (mr, ledger) = reviewed_needs_fix_row(None);

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "NEEDS_REVIEW");
        assert_eq!(row.review_verdict, None);
    }

    #[test]
    fn review_metadata_fingerprint_covers_source_title_body_and_draft_state() {
        let baseline = review_metadata_fingerprint(Some("sha"), Some("title"), Some("body"), false);
        assert_eq!(
            baseline,
            review_metadata_fingerprint(Some("sha"), Some("title"), Some("body"), false)
        );
        assert_ne!(
            baseline,
            review_metadata_fingerprint(Some("other"), Some("title"), Some("body"), false)
        );
        assert_ne!(
            baseline,
            review_metadata_fingerprint(Some("sha"), Some("other"), Some("body"), false)
        );
        assert_ne!(
            baseline,
            review_metadata_fingerprint(Some("sha"), Some("title"), Some("other"), false)
        );
        assert_ne!(
            baseline,
            review_metadata_fingerprint(Some("sha"), Some("title"), Some("body"), true)
        );
        assert_eq!(
            review_metadata_fingerprint(Some("sha"), Some("Draft: title"), Some("body"), true),
            review_metadata_fingerprint(Some("sha"), Some("title"), Some("body"), true)
        );
    }

    #[test]
    fn gah_draft_to_ready_transition_preserves_the_applicable_approval() {
        let (mut mr, mut ledger) = reviewed_needs_fix_row(None);
        mr.title = "approved title".into();
        mr.draft = false;
        mr.labels = vec!["gah-ready-for-human".into()];
        let review = &mut ledger.get_mut("#701").unwrap()[0];
        review.review_verdict = Some("APPROVE".into());
        review.validation_result = Some("APPROVE".into());
        set_review_identity(
            review,
            &mr,
            Some(review_metadata_fingerprint(
                mr.source_sha.as_deref(),
                Some("Draft: approved title"),
                mr.body.as_deref(),
                true,
            )),
        );

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "READY_FOR_HUMAN");
        assert_eq!(row.review_verdict.as_deref(), Some("APPROVE"));
    }

    #[test]
    fn redrafting_an_approved_mr_invalidates_the_old_review() {
        let (mut mr, mut ledger) = reviewed_needs_fix_row(None);
        mr.draft = true;
        let review = &mut ledger.get_mut("#701").unwrap()[0];
        review.review_verdict = Some("APPROVE".into());
        review.validation_result = Some("APPROVE".into());
        set_review_identity(
            review,
            &mr,
            Some(review_metadata_fingerprint(
                mr.source_sha.as_deref(),
                Some(&mr.title),
                mr.body.as_deref(),
                false,
            )),
        );

        let row = sync_mr_to_json(&mr, Some("test".into()), &ledger);

        assert_eq!(row.classification, "NEEDS_REVIEW");
        assert_eq!(row.review_verdict, None);
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

    const TEST_REVIEW_GENERATION: &str = "review-v1:source-sha:metadata-fingerprint";

    fn ledger_entry(
        mode: &str,
        branch: &str,
        dispatch_reason: Option<&str>,
        attempts_started: Option<u32>,
    ) -> crate::ledger::LedgerEntry {
        let tmp = tempfile::tempdir().unwrap();
        let prof = crate::config::Profile {
            manager_wake_autonomy: crate::config::WakeAutonomy::default(),
            prune_older_than_days: None,
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
            opencode_idle_timeout_seconds: None,
            opencode_idle_timeout_seconds_by_model: std::collections::HashMap::new(),
            max_concurrent_per_model: std::collections::HashMap::new(),
            openhands_idle_timeout_seconds: None,
            vibe_idle_timeout_seconds: None,
            codex_idle_timeout_seconds: None,
            claude_idle_timeout_seconds: None,
            max_parallel_workers: None,
            max_open_managed_mrs: None,
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
            review_hard_timeout_seconds: None,
            validation_timeout_seconds: None,
            routing: crate::config::RoutingPolicy::default(),
            pacing: crate::quota::PacingConfig::default(),
            publishing: Default::default(),
        };
        let mut entry =
            crate::ledger::LedgerEntry::new("test", &prof, "codex", mode, "x", None, None);
        entry.branch = Some(branch.into());
        entry.attempts_started = attempts_started;
        entry.dispatch_reason = dispatch_reason.map(|s| s.into());
        entry.review_contract_version = Some(crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION);
        entry.review_generation = Some(TEST_REVIEW_GENERATION.into());
        entry
    }

    fn count_current_fixes(
        entries: &[crate::ledger::LedgerEntry],
    ) -> std::collections::HashMap<String, usize> {
        let mut mr = super::sync_mr_to_json(
            &base_mr(),
            Some("test".into()),
            &crate::ledger::LedgerEntriesByWorkId::new(),
        );
        mr.branch = "branch-A".into();
        mr.review_generation = Some(TEST_REVIEW_GENERATION.into());
        super::count_current_fix_attempts_for_mrs(entries, "test", "test", &[mr])
    }

    fn clear_entry(work_id: &str) -> crate::ledger::LedgerEntry {
        let mut entry = ledger_entry("clear_attempts", "", None, None);
        entry.branch = None;
        entry.work_id = Some(work_id.into());
        entry
    }

    /// Two internal OpenHands attempts inside one initial dispatch consume
    /// zero post-review repair retries.
    #[test]
    fn internal_retries_in_initial_dispatch_count_zero() {
        let counts =
            count_current_fixes(&[ledger_entry("fix", "branch-A", Some("initial"), Some(2))]);
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
        let counts = count_current_fixes(&[
            ledger_entry("fix", "branch-A", Some("initial"), Some(2)),
            ledger_entry("fix", "branch-A", Some("post_review_repair"), Some(3)),
        ]);
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
        let counts = count_current_fixes(&[ledger_entry(
            "fix",
            "branch-A",
            Some("post_review_repair"),
            Some(5),
        )]);
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
        let counts = count_current_fixes(&[
            ledger_entry("fix", "branch-A", Some("initial"), Some(2)),
            ledger_entry("fix", "branch-A", Some("post_review_repair"), Some(1)),
            ledger_entry("fix", "branch-A", Some("post_review_repair"), Some(1)),
        ]);
        assert_eq!(
            counts.get("branch-A").copied().unwrap_or(0),
            2,
            "two post_review_repair entries = count 2 = AUTO_RETRY_CAP"
        );
    }

    #[test]
    fn clear_attempts_resets_repair_cycles_for_matching_work_item() {
        let mut before_one = ledger_entry("fix", "branch-A", Some("post_review_repair"), Some(1));
        before_one.work_id = Some("branch-A".into());
        let mut before_two = before_one.clone();
        before_two.timestamp = "2026-01-02T00:00:00Z".into();
        let mut attributed_review = ledger_entry("review", "branch-A", Some("review"), Some(1));
        attributed_review.work_id = Some("#437".into());
        attributed_review.timestamp = "2026-01-02T12:00:00Z".into();
        let mut clear = clear_entry("TICKET-437");
        clear.timestamp = "2026-01-03T00:00:00Z".into();
        let mut after = before_one.clone();
        after.work_id = Some("#437".into());
        after.timestamp = "2026-01-04T00:00:00Z".into();

        let counts =
            count_current_fixes(&[before_one, before_two, attributed_review, clear, after]);
        assert_eq!(counts.get("branch-A"), Some(&1));
    }

    #[test]
    fn clear_attempts_is_scoped_and_resets_merge_retry_budget() {
        let mut before = ledger_entry("merge", "branch-A", None, Some(2));
        before.work_id = Some("#437".into());
        let mut foreign_clear = clear_entry("#437");
        foreign_clear.profile = "other-profile".into();
        let matching_clear = clear_entry("#437");
        let mut after = before.clone();
        after.attempts_started = Some(1);

        let entries = [before, foreign_clear, matching_clear, after];
        let counts = super::count_merge_attempts_per_branch_for_scope(&entries, "test", "test");
        assert_eq!(counts.get("branch-A"), Some(&1));
        assert!(super::count_merge_attempts_per_branch_for_scope(
            &entries,
            "other-profile",
            "test"
        )
        .is_empty());
    }

    /// Entries with mode != "fix" (review, merge) never count.
    #[test]
    fn review_and_merge_entries_never_count() {
        let counts = count_current_fixes(&[
            ledger_entry("review", "branch-A", Some("review"), Some(1)),
            ledger_entry("merge", "branch-A", None, Some(1)),
        ]);
        assert!(
            counts.is_empty(),
            "review/merge entries must not count toward fix retry cap"
        );
    }

    /// Pre-existing ledger entries without dispatch_reason (legacy) must not
    /// count — they cannot be proven to be post-review repairs.
    #[test]
    fn legacy_entries_without_dispatch_reason_do_not_count() {
        let counts = count_current_fixes(&[ledger_entry("fix", "branch-A", None, Some(2))]);
        assert_eq!(
            counts.get("branch-A").copied().unwrap_or(0),
            0,
            "legacy entries without dispatch_reason must not count (unprovable)"
        );
    }

    #[test]
    fn pre_bump_post_review_repair_entries_do_not_count_toward_fix_retry_cap() {
        let mut old_entry = ledger_entry("fix", "branch-A", Some("post_review_repair"), Some(1));
        old_entry.review_contract_version = None; // Pre-bump
        let counts = count_current_fixes(&[old_entry]);
        assert_eq!(
            counts.get("branch-A").copied().unwrap_or(0),
            0,
            "pre-bump post_review_repair entries must not count under new contract"
        );
    }

    #[test]
    fn stale_review_generation_does_not_consume_current_fix_budget() {
        let mut stale = ledger_entry("fix", "branch-A", Some("post_review_repair"), Some(1));
        stale.review_generation = Some("review-v1:old-source:old-metadata".into());
        let current = ledger_entry("fix", "branch-A", Some("post_review_repair"), Some(4));

        let counts = count_current_fixes(&[stale, current]);
        assert_eq!(counts.get("branch-A"), Some(&1));
    }
}
