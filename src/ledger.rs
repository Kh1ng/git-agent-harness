use crate::config::{GahConfig, Profile};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Coarse attribution for why a dispatch failed. Deliberately not
/// exhaustively wired everywhere yet (TICKET-063): only the least ambiguous
/// boundaries in dispatch.rs set this. Everything else stays `None` rather
/// than guess. Persisted as a plain lowercase string, matching this
/// codebase's existing convention for enum-like ledger fields (e.g.
/// `validation_result`) rather than a serde-tagged enum, so the wire format
/// never breaks if variants are renamed internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // unwired variants are the schema for future tickets, not unused code
pub enum FailureClass {
    HarnessError,
    EnvironmentError,
    BackendError,
    AgentNoProgress,
    AgentFailure,
    ValidationFailure,
    HumanBlocked,
    Unknown,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HarnessError => "harness_error",
            Self::EnvironmentError => "environment_error",
            Self::BackendError => "backend_error",
            Self::AgentNoProgress => "agent_no_progress",
            Self::AgentFailure => "agent_failure",
            Self::ValidationFailure => "validation_failure",
            Self::HumanBlocked => "human_blocked",
            Self::Unknown => "unknown",
        }
    }
}

/// Where in the dispatch pipeline a failure occurred. See `FailureClass` for
/// the "not exhaustively wired yet" caveat — same applies here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // unwired variants are the schema for future tickets, not unused code
pub enum FailureStage {
    Preflight,
    BaselineValidation,
    Route,
    BackendLaunch,
    AgentRun,
    PostValidation,
    Commit,
    Push,
    MrCreate,
    Review,
    Sync,
}

impl FailureStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::BaselineValidation => "baseline_validation",
            Self::Route => "route",
            Self::BackendLaunch => "backend_launch",
            Self::AgentRun => "agent_run",
            Self::PostValidation => "post_validation",
            Self::Commit => "commit",
            Self::Push => "push",
            Self::MrCreate => "mr_create",
            Self::Review => "review",
            Self::Sync => "sync",
        }
    }
}

/// TICKET-064: one record per retry-loop attempt within a single dispatch.
/// Embedded in LedgerEntry (not a separate append-only stream) — a
/// deliberate scope reduction from the ticket's stated preference, chosen
/// for simplicity (one file, one read path). The tradeoff: if the process
/// crashes mid-retry, in-progress attempts are lost along with the rest of
/// the not-yet-appended ledger line, same as every other field on this
/// struct today.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct AttemptRecord {
    pub attempt_number: u32,
    pub backend: String,
    pub effective_model: Option<String>,
    pub exit_code: Option<i32>,
    pub validation_result: Option<String>,
    pub failure_class: Option<String>,
    pub failure_stage: Option<String>,
    pub duration_seconds: Option<f64>,
    pub diff_path: Option<String>,
    /// TICKET-101: provider-reported usage for exactly this attempt, not
    /// the whole dispatch. Same "unknown stays unknown, never zero"
    /// discipline as `LedgerEntry.usage` -- an empty `LedgerUsage` (all
    /// `None`) means "the backend didn't report it," not "zero usage."
    /// `#[serde(default)]` so historical ledger entries without this field
    /// still deserialize.
    #[serde(default)]
    pub usage: LedgerUsage,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct LedgerUsage {
    pub usage_source: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub requests_count: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub actual_cost_usd: Option<f64>,
    pub quota_window: Option<String>,
    pub quota_used_percent: Option<f64>,
    pub quota_remaining_percent: Option<f64>,
    pub quota_reset_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct RoutingDiagnostics {
    #[serde(default)]
    pub policy_reordered_candidates: bool,
    #[serde(default)]
    pub selected_backend: Option<String>,
    #[serde(default)]
    pub selected_model: Option<String>,
    #[serde(default)]
    pub selected_quota_pool: Option<String>,
    #[serde(default)]
    pub selected_pace_band: Option<String>,
    #[serde(default)]
    pub selected_cost_class: Option<String>,
    #[serde(default)]
    pub selected_over: Vec<String>,
    #[serde(default)]
    pub candidates: Vec<RoutingCandidateDiagnostic>,
    #[serde(default)]
    pub human_summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct RoutingCandidateDiagnostic {
    pub backend: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quota_pool: Option<String>,
    #[serde(default)]
    pub default_order: Option<usize>,
    #[serde(default)]
    pub consideration_order: Option<usize>,
    #[serde(default)]
    pub pace_band: Option<String>,
    #[serde(default)]
    pub cost_class: Option<String>,
    #[serde(default)]
    pub skip_reason: Option<String>,
    #[serde(default)]
    pub unavailable_until: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LedgerEntry {
    pub timestamp: String,
    pub session_id: Option<String>,
    pub profile: String,
    pub display_name: String,
    pub repo_id: String,
    pub repo: String,
    pub local_path: String,
    pub provider: String,
    pub backend: String,
    pub requested_backend: String,
    pub effective_backend: String,
    pub requested_model: Option<String>,
    pub effective_model: Option<String>,
    pub routing_reason: Option<String>,
    pub fallback_used: bool,
    pub confidence_impact: Option<String>,
    pub human_required: bool,
    #[serde(default)]
    pub routing_diagnostics: Option<RoutingDiagnostics>,
    pub mode: String,
    pub target_summary: Option<String>,
    #[serde(default)]
    pub work_id: Option<String>,
    #[serde(default)]
    pub work_title: Option<String>,
    pub branch: Option<String>,
    pub session_dir: Option<String>,
    pub duration_seconds: Option<f64>,
    pub backend_exit_code: Option<i32>,
    pub validation_result: Option<String>,
    /// TICKET-125: review verdict/confidence/reviewer identity for associating
    /// review entries back to implementation entries. `#[serde(default)]` for
    /// pre-existing ledger lines.
    #[serde(default)]
    pub review_verdict: Option<String>,
    #[serde(default)]
    pub review_confidence: Option<String>,
    #[serde(default)]
    pub reviewer_backend: Option<String>,
    #[serde(default)]
    pub reviewer_model: Option<String>,
    pub commit_attempted: bool,
    pub commit_created: bool,
    pub push_attempted: bool,
    pub push_succeeded: bool,
    pub mr_attempted: bool,
    pub mr_created: bool,
    pub mr_url: Option<String>,
    pub files_changed: Option<u32>,
    pub insertions: Option<u32>,
    pub deletions: Option<u32>,
    pub error_summary: Option<String>,
    /// TICKET-063: coarse failure attribution, populated at only the
    /// clearest boundaries so far. `#[serde(default)]` so pre-existing
    /// JSONL ledger lines without these keys still deserialize.
    #[serde(default)]
    pub failure_class: Option<String>,
    #[serde(default)]
    pub failure_stage: Option<String>,
    /// TICKET-064: how many retry-loop iterations were entered vs. ran
    /// their backend to completion (launched and exited, regardless of
    /// whether validation then passed). `#[serde(default)]` for pre-existing
    /// ledger lines.
    #[serde(default)]
    pub attempts_started: u32,
    #[serde(default)]
    pub attempts_completed: u32,
    #[serde(default)]
    pub attempts: Vec<AttemptRecord>,
    pub usage: LedgerUsage,
}

impl LedgerEntry {
    pub fn new(
        profile_name: &str,
        profile: &Profile,
        backend: &str,
        mode: &str,
        target: &str,
        session_id: Option<String>,
        session_dir: Option<&Path>,
    ) -> Self {
        Self {
            timestamp: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string()),
            session_id,
            profile: profile_name.to_string(),
            display_name: profile.display_name.clone(),
            repo_id: profile.repo_id.clone(),
            repo: profile.repo.clone(),
            local_path: profile.local_path.clone(),
            provider: profile.provider.clone(),
            backend: backend.to_string(),
            requested_backend: backend.to_string(),
            effective_backend: backend.to_string(),
            requested_model: None,
            effective_model: None,
            routing_reason: None,
            fallback_used: false,
            confidence_impact: None,
            human_required: false,
            routing_diagnostics: None,
            mode: mode.to_string(),
            target_summary: summarize_target(target),
            work_id: None,
            work_title: None,
            branch: None,
            session_dir: session_dir.map(|p| p.display().to_string()),
            duration_seconds: None,
            backend_exit_code: None,
            validation_result: None,
            review_verdict: None,
            review_confidence: None,
            reviewer_backend: None,
            reviewer_model: None,
            commit_attempted: false,
            commit_created: false,
            push_attempted: false,
            push_succeeded: false,
            mr_attempted: false,
            mr_created: false,
            mr_url: None,
            files_changed: None,
            insertions: None,
            deletions: None,
            error_summary: None,
            failure_class: None,
            failure_stage: None,
            attempts_started: 0,
            attempts_completed: 0,
            attempts: Vec::new(),
            usage: LedgerUsage::default(),
        }
    }

    /// Set failure attribution. Call this at the specific error site, not
    /// generically in the top-level error handler — the whole point is to
    /// know *which* boundary failed, and that context is only available
    /// where the error actually originates.
    pub fn set_failure(&mut self, class: FailureClass, stage: FailureStage) {
        self.failure_class = Some(class.as_str().to_string());
        self.failure_stage = Some(stage.as_str().to_string());
    }
}

pub fn append(cfg: &GahConfig, entry: &LedgerEntry) -> Result<PathBuf> {
    let path = cfg.defaults.ledger_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating ledger directory {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening ledger {}", path.display()))?;
    serde_json::to_writer(&mut file, entry).context("serializing ledger entry")?;
    file.write_all(b"\n").context("writing ledger newline")?;
    Ok(path)
}

pub fn read_entries(cfg: &GahConfig) -> Result<Vec<LedgerEntry>> {
    let path = cfg.defaults.ledger_path();
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut entries = vec![];
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<LedgerEntry>(line)
            .with_context(|| format!("parsing ledger entry {} from {}", idx + 1, path.display()))?;
        entries.push(entry);
    }
    Ok(entries)
}

/// TICKET-125: review mode's own ledger entry records the reviewer's
/// backend/model, not the implementation's -- grouping/cost-vs-quality
/// reporting needs the verdict attributed back to whichever backend
/// actually wrote the code being reviewed. Finds the most recent fix/improve
/// entry for `branch` that doesn't already have a verdict and updates it
/// in place (the ledger has no other mutation path today; this is the one
/// exception, and it's rare enough -- once per review completion -- not to
/// need more than a full read-modify-write of the file).
pub fn backfill_review_verdict(
    cfg: &GahConfig,
    branch: &str,
    verdict: &str,
    confidence: &str,
    reviewer_backend: &str,
    reviewer_model: Option<&str>,
) -> Result<bool> {
    let mut entries = read_entries(cfg)?;
    let target_idx = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.branch.as_deref() == Some(branch)
                && matches!(e.mode.as_str(), "fix" | "improve")
                && e.review_verdict.is_none()
        })
        .max_by_key(|(_, e)| e.timestamp.clone())
        .map(|(idx, _)| idx);

    let Some(idx) = target_idx else {
        return Ok(false);
    };
    entries[idx].review_verdict = Some(verdict.to_string());
    entries[idx].review_confidence = Some(confidence.to_string());
    entries[idx].reviewer_backend = Some(reviewer_backend.to_string());
    entries[idx].reviewer_model = reviewer_model.map(|m| m.to_string());

    let path = cfg.defaults.ledger_path();
    let mut out = String::new();
    for entry in &entries {
        out.push_str(&serde_json::to_string(entry).context("serializing ledger entry")?);
        out.push('\n');
    }
    fs::write(&path, out).with_context(|| format!("rewriting ledger {}", path.display()))?;
    Ok(true)
}

/// TICKET-096: the query sync/reconciliation needs to associate a
/// `SyncMr.work_id` (extracted from a PR/MR title) back to the ledger
/// entries that dispatched it. No new sync-side structure required.
pub fn entries_for_work_id(cfg: &GahConfig, work_id: &str) -> Result<Vec<LedgerEntry>> {
    Ok(read_entries(cfg)?
        .into_iter()
        .filter(|e| e.work_id.as_deref() == Some(work_id))
        .collect())
}

pub type LedgerEntriesByWorkId = BTreeMap<String, Vec<LedgerEntry>>;

pub fn index_entries_by_work_id(entries: &[LedgerEntry]) -> LedgerEntriesByWorkId {
    let mut index = BTreeMap::new();
    for entry in entries {
        if let Some(work_id) = entry.work_id.as_ref() {
            index
                .entry(work_id.clone())
                .or_insert_with(Vec::new)
                .push(entry.clone());
        }
    }
    index
}

/// TICKET-072: append-only reconciliation of dispatched work with later
/// provider outcomes (MR merged, closed unmerged, state changed). A
/// separate log from `ledger.jsonl` -- never rewrites dispatch history,
/// only ever appends a new entry when a work item's classified state
/// actually changed since the last known reconciliation.
pub mod reconcile {
    use super::{read_entries, LedgerEntry};
    use crate::config::{self, GahConfig};
    use crate::sync;
    use anyhow::{Context, Result};
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    #[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
    pub struct ReconciliationEntry {
        pub timestamp: String,
        pub work_id: String,
        pub branch: Option<String>,
        pub mr_url: Option<String>,
        pub previous_state: Option<String>,
        pub new_state: String,
        pub source: String,
    }

    pub fn read_reconciliation_entries(cfg: &GahConfig) -> Result<Vec<ReconciliationEntry>> {
        let path = cfg.defaults.reconciliation_path();
        if !path.exists() {
            return Ok(vec![]);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut entries = vec![];
        for (idx, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str::<ReconciliationEntry>(line).with_context(|| {
                format!(
                    "parsing reconciliation entry {} from {}",
                    idx + 1,
                    path.display()
                )
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }

    fn append_reconciliation_entry(cfg: &GahConfig, entry: &ReconciliationEntry) -> Result<()> {
        let path = cfg.defaults.reconciliation_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("creating reconciliation directory {}", parent.display())
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening reconciliation log {}", path.display()))?;
        serde_json::to_writer(&mut file, entry).context("serializing reconciliation entry")?;
        file.write_all(b"\n")
            .context("writing reconciliation newline")?;
        Ok(())
    }

    /// Most recent recorded state per work_id (reconciliation log is
    /// chronological, so the last entry for a given work_id wins).
    fn last_known_states(entries: &[ReconciliationEntry]) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        for entry in entries {
            map.insert(entry.work_id.clone(), entry.new_state.clone());
        }
        map
    }

    /// Most recent branch/mr_url per work_id from dispatch history (ledger
    /// is chronological, so the last matching entry wins).
    fn latest_dispatch_identity(
        entries: &[LedgerEntry],
    ) -> BTreeMap<String, (Option<String>, Option<String>)> {
        let mut map = BTreeMap::new();
        for entry in entries {
            let Some(work_id) = &entry.work_id else {
                continue;
            };
            map.insert(
                work_id.clone(),
                (entry.branch.clone(), entry.mr_url.clone()),
            );
        }
        map
    }

    pub fn run(cfg: &GahConfig, profile_name: &str, json: bool) -> Result<()> {
        let profile = config::get_profile(cfg, profile_name)?;
        let ledger_entries = read_entries(cfg)?;
        let history = read_reconciliation_entries(cfg)?;
        let mut last_known = last_known_states(&history);
        let dispatch_identity = latest_dispatch_identity(&ledger_entries);

        let mrs = sync::fetch_mrs(profile)?;

        let mut new_entries = vec![];
        for (work_id, (branch, mr_url)) in &dispatch_identity {
            let matching_mr = mrs.iter().find(|mr| {
                branch.as_deref() == Some(mr.branch.as_str())
                    || (mr_url.is_some() && mr_url.as_deref() == mr.url.as_deref())
            });
            let Some(mr) = matching_mr else { continue };
            let new_state = sync::classify(mr).to_string();
            let previous = last_known.get(work_id).cloned();
            if previous.as_deref() == Some(new_state.as_str()) {
                continue;
            }
            let entry = ReconciliationEntry {
                timestamp: OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .unwrap_or_default(),
                work_id: work_id.clone(),
                branch: branch.clone(),
                mr_url: mr.url.clone(),
                previous_state: previous,
                new_state: new_state.clone(),
                source: "sync".to_string(),
            };
            append_reconciliation_entry(cfg, &entry)?;
            last_known.insert(work_id.clone(), new_state);
            new_entries.push(entry);
        }

        if json {
            println!("{}", serde_json::to_string(&new_entries)?);
        } else {
            println!(
                "Reconciliation log: {}",
                cfg.defaults.reconciliation_path().display()
            );
            println!("New entries: {}", new_entries.len());
            for entry in &new_entries {
                println!(
                    "  {} {} -> {} ({})",
                    entry.work_id,
                    entry.previous_state.as_deref().unwrap_or("none"),
                    entry.new_state,
                    entry.branch.as_deref().unwrap_or("")
                );
            }
        }
        Ok(())
    }
}

fn summarize_target(target: &str) -> Option<String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return None;
    }
    let single_line = trimmed.lines().next().unwrap_or(trimmed).trim();
    let mut summary = single_line.to_string();
    if summary.len() > 240 {
        summary.truncate(240);
        summary.push_str("...");
    }
    Some(summary)
}

/// TICKET-125: GroupBy option for ledger summary
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    None,
    Backend,
    Model,
}

impl std::str::FromStr for GroupBy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(GroupBy::None),
            "backend" => Ok(GroupBy::Backend),
            "model" => Ok(GroupBy::Model),
            _ => Err(format!(
                "Invalid group-by value: '{}'. Expected 'none', 'backend' or 'model'",
                s
            )),
        }
    }
}

pub mod summary {
    use super::{read_entries, GroupBy};
    use crate::config;
    use anyhow::Result;
    use serde::Serialize;
    use std::collections::BTreeMap;
    use time::{Duration, OffsetDateTime};

    /// TICKET-071: stable machine-readable aggregate ledger data. Shared by
    /// both the human-readable and `--json` output paths so they can never
    /// drift apart -- no speculative economics logic, just the counts the
    /// human view already computed.
    /// TICKET-125: Grouped summary data for a specific backend or model
    #[derive(Debug, Serialize)]
    pub struct GroupSummary {
        pub group_key: String,
        pub entries: usize,
        pub attempts: usize,
        pub validation_pass: usize,
        pub review_verdict_distribution: BTreeMap<String, usize>,
        pub total_cost_usd: Option<f64>,
        pub average_cost_usd: Option<f64>,
        pub cost_per_approve_strong: Option<f64>,
    }

    #[derive(Debug, Serialize)]
    pub struct SummaryData {
        pub ledger_path: String,
        pub entries: usize,
        pub success: usize,
        pub failed: usize,
        pub by_mode: BTreeMap<String, usize>,
        pub by_requested_backend: BTreeMap<String, usize>,
        pub by_backend: BTreeMap<String, usize>,
        pub by_model: BTreeMap<String, usize>,
        pub by_failure_class: BTreeMap<String, usize>,
        pub fallback_count: usize,
        pub validation_pass: usize,
        pub push_success: usize,
        pub mr_count: usize,
        pub human_required_count: usize,
        pub average_duration_seconds: Option<f64>,
        pub usage_input_tokens: u64,
        pub usage_output_tokens: u64,
        pub usage_total_tokens: u64,
        pub usage_requests_count: u64,
        pub estimated_cost_usd: Option<f64>,
        pub actual_cost_usd: Option<f64>,
        pub last_run: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub grouped_by_backend: Option<Vec<GroupSummary>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub grouped_by_model: Option<Vec<GroupSummary>>,
    }

    pub fn run_with_json(
        since: &str,
        profile: Option<&str>,
        config_path: Option<&str>,
        json: bool,
        group_by: GroupBy,
    ) -> Result<()> {
        let cfg = config::load(config_path)?;
        let data = build_summary(&cfg, since, profile, group_by)?;

        if json {
            println!("{}", serde_json::to_string(&data)?);
            return Ok(());
        }

        println!("Ledger: {}", data.ledger_path);
        println!("Entries: {}", data.entries);
        if data.entries == 0 {
            return Ok(());
        }
        println!("Success: {}", data.success);
        println!("Failed:  {}", data.failed);
        println!("By mode:");
        print_counts(&data.by_mode);
        println!("Requested backend:");
        print_counts(&data.by_requested_backend);
        println!("By backend:");
        print_counts(&data.by_backend);
        println!("By model:");
        print_counts(&data.by_model);
        if !data.by_failure_class.is_empty() {
            println!("By failure class:");
            print_counts(&data.by_failure_class);
        }
        println!("Fallbacks: {}", data.fallback_count);
        println!(
            "Validation pass rate: {}/{}",
            data.validation_pass, data.entries
        );
        println!("Push success rate: {}/{}", data.push_success, data.entries);
        println!("MR count: {}", data.mr_count);
        println!("Human required: {}", data.human_required_count);
        if let Some(avg) = data.average_duration_seconds {
            println!("Average duration: {:.1}s", avg);
        }
        println!(
            "Usage totals: input={} output={} total={} requests={}",
            data.usage_input_tokens,
            data.usage_output_tokens,
            data.usage_total_tokens,
            data.usage_requests_count
        );
        if let Some(cost) = data.estimated_cost_usd {
            println!("Estimated cost total: ${:.4}", cost);
        }
        if let Some(cost) = data.actual_cost_usd {
            println!("Actual cost total: ${:.4}", cost);
        }
        if let Some(last) = data.last_run {
            println!("Last run: {}", last);
        }

        // TICKET-125: Display grouped data if requested
        if let Some(grouped) = &data.grouped_by_backend {
            println!("\nGrouped by backend:");
            for group in grouped {
                println!("  Backend: {}", group.group_key);
                println!("    Entries: {}", group.entries);
                println!("    Attempts: {}", group.attempts);
                println!(
                    "    Validation pass: {}/ {}",
                    group.validation_pass, group.entries
                );
                println!("    Review verdict distribution:");
                for (verdict, count) in &group.review_verdict_distribution {
                    println!("      {}: {}", verdict, count);
                }
                if let Some(cost) = group.total_cost_usd {
                    println!("    Total cost: ${:.4}", cost);
                }
                if let Some(cost) = group.average_cost_usd {
                    println!("    Average cost: ${:.4}", cost);
                }
                if let Some(cost) = group.cost_per_approve_strong {
                    println!("    Cost per APPROVE_STRONG: ${:.4}", cost);
                }
            }
        }

        if let Some(grouped) = &data.grouped_by_model {
            println!("\nGrouped by model:");
            for group in grouped {
                println!("  Model: {}", group.group_key);
                println!("    Entries: {}", group.entries);
                println!("    Attempts: {}", group.attempts);
                println!(
                    "    Validation pass: {}/ {}",
                    group.validation_pass, group.entries
                );
                println!("    Review verdict distribution:");
                for (verdict, count) in &group.review_verdict_distribution {
                    println!("      {}: {}", verdict, count);
                }
                if let Some(cost) = group.total_cost_usd {
                    println!("    Total cost: ${:.4}", cost);
                }
                if let Some(cost) = group.average_cost_usd {
                    println!("    Average cost: ${:.4}", cost);
                }
                if let Some(cost) = group.cost_per_approve_strong {
                    println!("    Cost per APPROVE_STRONG: ${:.4}", cost);
                }
            }
        }

        Ok(())
    }

    fn build_summary(
        cfg: &config::GahConfig,
        since: &str,
        profile: Option<&str>,
        group_by: GroupBy,
    ) -> Result<SummaryData> {
        let cutoff = parse_since(since)?;
        let mut entries = read_entries(cfg)?;
        if let Some(profile) = profile {
            entries.retain(|entry| entry.profile == profile);
        }
        entries.retain(|entry| entry.timestamp >= cutoff);

        let mut by_mode: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_backend: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_requested_backend: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_model: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_failure_class: BTreeMap<String, usize> = BTreeMap::new();
        let mut success = 0usize;
        let mut failed = 0usize;
        let mut fallback = 0usize;
        let mut validation_pass = 0usize;
        let mut push_success = 0usize;
        let mut mr_count = 0usize;
        let mut human_required_count = 0usize;
        let mut duration_total = 0.0f64;
        let mut duration_count = 0usize;
        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;
        let mut total_tokens = 0u64;
        let mut requests_count = 0u64;
        let mut estimated_cost = 0.0f64;
        let mut actual_cost = 0.0f64;
        let mut estimated_cost_seen = false;
        let mut actual_cost_seen = false;
        for entry in &entries {
            *by_mode.entry(entry.mode.clone()).or_default() += 1;
            *by_backend
                .entry(entry.effective_backend.clone())
                .or_default() += 1;
            *by_requested_backend
                .entry(entry.requested_backend.clone())
                .or_default() += 1;
            if let Some(model) = &entry.effective_model {
                *by_model.entry(model.clone()).or_default() += 1;
            }
            if let Some(class) = &entry.failure_class {
                *by_failure_class.entry(class.clone()).or_default() += 1;
            }
            if entry.error_summary.is_some() {
                failed += 1;
            } else {
                success += 1;
            }
            if entry.fallback_used {
                fallback += 1;
            }
            if matches!(
                entry.validation_result.as_deref(),
                Some("passed") | Some("APPROVE_STRONG") | Some("APPROVE_WEAK")
            ) {
                validation_pass += 1;
            }
            if entry.push_succeeded {
                push_success += 1;
            }
            if entry.mr_created {
                mr_count += 1;
            }
            if entry.human_required {
                human_required_count += 1;
            }
            if let Some(duration) = entry.duration_seconds {
                duration_total += duration;
                duration_count += 1;
            }
            input_tokens += entry.usage.input_tokens.unwrap_or(0);
            output_tokens += entry.usage.output_tokens.unwrap_or(0);
            total_tokens += entry.usage.total_tokens.unwrap_or(0);
            requests_count += entry.usage.requests_count.unwrap_or(0);
            if let Some(cost) = entry.usage.estimated_cost_usd {
                estimated_cost += cost;
                estimated_cost_seen = true;
            }
            if let Some(cost) = entry.usage.actual_cost_usd {
                actual_cost += cost;
                actual_cost_seen = true;
            }
        }

        let last_run = entries.last().map(|last| {
            format!(
                "{} {} {} {}",
                last.timestamp, last.profile, last.mode, last.effective_backend
            )
        });

        // TICKET-125: Build grouped data if requested
        let grouped_by_backend = if group_by == GroupBy::Backend {
            build_grouped_summary(&entries, |entry| entry.effective_backend.clone())
        } else {
            None
        };

        let grouped_by_model = if group_by == GroupBy::Model {
            build_grouped_summary(&entries, |entry| {
                entry.effective_model.clone().unwrap_or_default()
            })
        } else {
            None
        };

        Ok(SummaryData {
            ledger_path: cfg.defaults.ledger_path().display().to_string(),
            entries: entries.len(),
            success,
            failed,
            by_mode,
            by_requested_backend,
            by_backend,
            by_model,
            by_failure_class,
            fallback_count: fallback,
            validation_pass,
            push_success,
            mr_count,
            human_required_count,
            average_duration_seconds: (duration_count > 0)
                .then_some(duration_total / duration_count as f64),
            usage_input_tokens: input_tokens,
            usage_output_tokens: output_tokens,
            usage_total_tokens: total_tokens,
            usage_requests_count: requests_count,
            estimated_cost_usd: estimated_cost_seen.then_some(estimated_cost),
            actual_cost_usd: actual_cost_seen.then_some(actual_cost),
            last_run,
            grouped_by_backend,
            grouped_by_model,
        })
    }

    /// TICKET-125: Build grouped summary data for a specific grouping key
    pub fn build_grouped_summary<F>(
        entries: &[super::LedgerEntry],
        group_key_fn: F,
    ) -> Option<Vec<GroupSummary>>
    where
        F: Fn(&super::LedgerEntry) -> String,
    {
        if entries.is_empty() {
            return None;
        }

        use std::collections::BTreeMap;

        // Group entries by the grouping key
        let mut groups: BTreeMap<String, Vec<&super::LedgerEntry>> = BTreeMap::new();
        for entry in entries {
            let key = group_key_fn(entry);
            groups.entry(key).or_default().push(entry);
        }

        let mut summaries = Vec::new();
        for (group_key, group_entries) in groups {
            let group_entry_count = group_entries.len();
            let mut attempts = 0usize;
            let mut validation_pass = 0usize;
            let mut review_verdict_distribution: BTreeMap<String, usize> = BTreeMap::new();
            let mut total_cost_usd = 0.0f64;
            let mut cost_seen = false;
            let mut approve_strong_count = 0usize;

            for entry in &group_entries {
                // Count attempts from the attempts vector
                attempts += entry.attempts.len();

                // Count validation passes
                if matches!(
                    entry.validation_result.as_deref(),
                    Some("passed") | Some("APPROVE_STRONG") | Some("APPROVE_WEAK")
                ) {
                    validation_pass += 1;
                }

                // Count review verdict distribution
                if let Some(verdict) = &entry.review_verdict {
                    *review_verdict_distribution
                        .entry(verdict.clone())
                        .or_default() += 1;
                    if verdict == "APPROVE_STRONG" {
                        approve_strong_count += 1;
                    }
                }

                // Sum up costs
                if let Some(cost) = entry.usage.actual_cost_usd {
                    total_cost_usd += cost;
                    cost_seen = true;
                } else if let Some(cost) = entry.usage.estimated_cost_usd {
                    total_cost_usd += cost;
                    cost_seen = true;
                }
            }

            let average_cost_usd = if cost_seen && group_entry_count > 0 {
                Some(total_cost_usd / group_entry_count as f64)
            } else {
                None
            };

            // Calculate cost per APPROVE_STRONG outcome
            let cost_per_approve_strong = if approve_strong_count > 0 && cost_seen {
                Some(total_cost_usd / approve_strong_count as f64)
            } else {
                None
            };

            summaries.push(GroupSummary {
                group_key,
                entries: group_entry_count,
                attempts,
                validation_pass,
                review_verdict_distribution,
                total_cost_usd: cost_seen.then_some(total_cost_usd),
                average_cost_usd,
                cost_per_approve_strong,
            });
        }

        Some(summaries)
    }

    fn print_counts(counts: &BTreeMap<String, usize>) {
        for (key, count) in counts {
            println!("  {:<16} {}", key, count);
        }
    }

    pub(crate) fn parse_since(input: &str) -> Result<String> {
        let now = OffsetDateTime::now_utc();
        let trimmed = input.trim();
        if let Some(days) = trimmed.strip_suffix('d') {
            let days = days.parse::<i64>()?;
            return Ok((now - Duration::days(days))
                .format(&time::format_description::well_known::Rfc3339)?);
        }
        if let Some(hours) = trimmed.strip_suffix('h') {
            let hours = hours.parse::<i64>()?;
            return Ok((now - Duration::hours(hours))
                .format(&time::format_description::well_known::Rfc3339)?);
        }
        anyhow::bail!(
            "invalid --since value '{}'; use forms like 7d or 24h",
            input
        )
    }
}

#[derive(Debug, Default, Clone)]
pub struct BackendUsageSummary {
    pub runs_this_week: u64,
    pub runs_this_session: u64,
    pub estimated_cost_this_week: f64,
    pub actual_cost_this_week: f64,
    pub strong_runs_this_week: u64,
    pub strong_runs_this_session: u64,
}

/// Returns `true` if the model name indicates a strong (capable) model.
///
/// Checks the final path segment of the model name (after the last `/`)
/// for weak-model substrings: "flash", "mini", "tiny", "lite".
/// This is a heuristic — it assumes models without these substrings are strong.
pub fn is_strong_model(model: &str) -> bool {
    let segment = model.rsplit('/').next().unwrap_or(model);
    let lower = segment.to_lowercase();
    !(lower.contains("flash")
        || lower.contains("mini")
        || lower.contains("tiny")
        || lower.contains("lite"))
}

/// Resolve the best available model name from a ledger entry.
/// Prefers effective_model, falls back to requested_model.
/// Returns None if both are missing, empty, or whitespace-only.
fn ledger_entry_model_name(entry: &LedgerEntry) -> Option<&str> {
    entry
        .effective_model
        .as_deref()
        .or(entry.requested_model.as_deref())
        .map(str::trim)
        .filter(|model| !model.is_empty())
}

pub fn usage_summary_for_backend(
    cfg: &GahConfig,
    backend: &str,
    model: Option<&str>,
    session_id: Option<&str>,
) -> Result<BackendUsageSummary> {
    let entries = read_entries(cfg)?;
    let cutoff = (OffsetDateTime::now_utc() - time::Duration::days(7))
        .format(&Rfc3339)
        .unwrap_or_default();
    let mut out = BackendUsageSummary::default();
    for entry in &entries {
        let same_backend = entry.effective_backend == backend;
        let same_model = model
            .map(|m| entry.effective_model.as_deref() == Some(m))
            .unwrap_or(true);
        let this_week = entry.timestamp >= cutoff;
        let this_session = session_id
            .map(|s| entry.session_id.as_deref() == Some(s))
            .unwrap_or(false);
        if same_backend && same_model && this_week {
            out.runs_this_week += 1;
            out.estimated_cost_this_week += entry.usage.estimated_cost_usd.unwrap_or(0.0);
            out.actual_cost_this_week += entry.usage.actual_cost_usd.unwrap_or(0.0);
        }
        if same_backend && same_model && this_session {
            out.runs_this_session += 1;
        }
        if same_backend
            && same_model
            && entry.confidence_impact.as_deref() != Some("low")
            && ledger_entry_model_name(entry)
                .map(is_strong_model)
                .unwrap_or(false)
        {
            if this_week {
                out.strong_runs_this_week += 1;
            }
            if this_session {
                out.strong_runs_this_session += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{
        append, backfill_review_verdict, entries_for_work_id, index_entries_by_work_id,
        is_strong_model, read_entries, reconcile, usage_summary_for_backend, FailureClass,
        FailureStage, GroupBy, LedgerEntry, RoutingCandidateDiagnostic, RoutingDiagnostics,
    };
    use crate::config::{Defaults, GahConfig, Profile, RoutingPolicy};
    use std::collections::HashMap;
    use std::fs;

    fn profile() -> Profile {
        Profile {
            display_name: "Repo".into(),
            repo_id: "repo".into(),
            provider: "github".into(),
            repo: "owner/repo".into(),
            local_path: "/tmp/repo".into(),
            artifact_root: "/tmp/artifacts".into(),
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
            agy_second_home: None,
            agy_print_timeout_seconds: std::collections::HashMap::new(),
            agy_idle_timeout_seconds: None,
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
            notify_command: None,
            routing: RoutingPolicy::default(),
            pacing: Default::default(),
        }
    }

    fn test_config() -> (tempfile::TempDir, GahConfig) {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GahConfig {
            defaults: Defaults {
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: String::new(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: RoutingPolicy::default(),
            },
            profiles: HashMap::new(),
        };
        (tmp, cfg)
    }

    #[test]
    fn target_summary_is_trimmed_to_first_line() {
        let entry = LedgerEntry::new(
            "test",
            &profile(),
            "claude",
            "pm",
            "first line\nsecond line",
            Some("123".into()),
            None,
        );
        assert_eq!(entry.target_summary.as_deref(), Some("first line"));
    }

    #[test]
    fn ledger_append_writes_jsonl() {
        let (_tmp, cfg) = test_config();
        let entry = LedgerEntry::new(
            "test",
            &profile(),
            "claude",
            "pm",
            "hello",
            Some("123".into()),
            None,
        );
        let path = append(&cfg, &entry).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"profile\":\"test\""));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn entries_for_work_id_filters_by_exact_match() {
        // TICKET-096: this is the query sync/reconciliation uses to match
        // a SyncMr.work_id back to the ledger entries that dispatched it.
        let (_tmp, cfg) = test_config();
        let mut matching = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        matching.work_id = Some("TICKET-096".into());
        append(&cfg, &matching).unwrap();

        let mut other = LedgerEntry::new("test", &profile(), "claude", "pm", "y", None, None);
        other.work_id = Some("TICKET-097".into());
        append(&cfg, &other).unwrap();

        let found = entries_for_work_id(&cfg, "TICKET-096").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].work_id.as_deref(), Some("TICKET-096"));
    }

    #[test]
    fn index_entries_by_work_id_groups_only_tagged_entries() {
        let mut first = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        first.work_id = Some("TICKET-096".into());
        let mut second = LedgerEntry::new("test", &profile(), "claude", "pm", "y", None, None);
        second.work_id = Some("TICKET-096".into());
        let untagged = LedgerEntry::new("test", &profile(), "claude", "pm", "z", None, None);

        let index = index_entries_by_work_id(&[first, second, untagged]);
        assert_eq!(index.len(), 1);
        assert_eq!(index["TICKET-096"].len(), 2);
    }

    #[test]
    fn reconciliation_log_is_empty_when_file_does_not_exist() {
        let (_tmp, cfg) = test_config();
        let entries = reconcile::read_reconciliation_entries(&cfg).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn reconciliation_entries_round_trip_through_jsonl() {
        use reconcile::ReconciliationEntry;
        // test_config() gives a fresh tempdir-backed artifact_root per test,
        // so reconciliation_path() resolves there without touching any
        // process-global env var (avoids the GAH_LEDGER_PATH-style test
        // race this project's own docs call out as known technical debt).
        let (_tmp, cfg) = test_config();
        let path = cfg.defaults.reconciliation_path();

        let entry = ReconciliationEntry {
            timestamp: "2026-07-05T00:00:00Z".into(),
            work_id: "TICKET-072".into(),
            branch: Some("gah/real-1".into()),
            mr_url: Some("https://github.com/owner/repo/pull/1".into()),
            previous_state: None,
            new_state: "NEEDS_REVIEW".into(),
            source: "sync".into(),
        };
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&entry).unwrap()),
        )
        .unwrap();

        let entries = reconcile::read_reconciliation_entries(&cfg).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], entry);
    }

    #[test]
    fn reconciliation_malformed_line_fails_loudly() {
        let (_tmp, cfg) = test_config();
        let path = cfg.defaults.reconciliation_path();
        fs::write(&path, "not valid json\n").unwrap();

        let result = reconcile::read_reconciliation_entries(&cfg);
        assert!(result.is_err());
    }

    #[test]
    fn routing_diagnostics_round_trip_through_json() {
        let mut entry = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        entry.routing_diagnostics = Some(RoutingDiagnostics {
            policy_reordered_candidates: true,
            selected_backend: Some("codex".into()),
            selected_model: Some("gpt-5.4".into()),
            selected_quota_pool: Some("codex-main".into()),
            selected_pace_band: Some("aggressive_burn".into()),
            selected_cost_class: Some("included_quota".into()),
            selected_over: vec!["openhands/gpt-5.4 (paid $0.2500)".into()],
            candidates: vec![RoutingCandidateDiagnostic {
                backend: "codex".into(),
                model: Some("gpt-5.4".into()),
                quota_pool: Some("codex-main".into()),
                default_order: Some(1),
                consideration_order: Some(0),
                pace_band: Some("aggressive_burn".into()),
                cost_class: Some("included_quota".into()),
                skip_reason: None,
                unavailable_until: None,
            }],
            human_summary: Some("selected codex/gpt-5.4".into()),
        });
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed
                .routing_diagnostics
                .as_ref()
                .unwrap()
                .selected_backend
                .as_deref(),
            Some("codex")
        );
    }

    // ── TICKET-063: structured failure_class / failure_stage ───────────────

    #[test]
    fn new_entry_has_no_failure_attribution_by_default() {
        let entry = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        assert_eq!(entry.failure_class, None);
        assert_eq!(entry.failure_stage, None);
        assert_eq!(entry.work_id, None);
        assert_eq!(entry.work_title, None);
    }

    #[test]
    fn set_failure_populates_both_fields_as_lowercase_strings() {
        let mut entry = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        entry.set_failure(FailureClass::BackendError, FailureStage::AgentRun);
        assert_eq!(entry.failure_class.as_deref(), Some("backend_error"));
        assert_eq!(entry.failure_stage.as_deref(), Some("agent_run"));
    }

    #[test]
    fn failure_attribution_round_trips_through_json() {
        let mut entry = LedgerEntry::new("test", &profile(), "claude", "pm", "x", None, None);
        entry.set_failure(FailureClass::AgentNoProgress, FailureStage::PostValidation);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"failure_class\":\"agent_no_progress\""));
        assert!(json.contains("\"failure_stage\":\"post_validation\""));
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.failure_class.as_deref(), Some("agent_no_progress"));
        assert_eq!(parsed.failure_stage.as_deref(), Some("post_validation"));
    }

    /// TICKET-063 requirement: existing historical JSONL entries — written
    /// before failure_class/failure_stage existed — must still deserialize.
    /// This is the exact fixture line used in
    /// tests/gah_cli.rs::ledger_summary_reports_recent_counts, which has no
    /// failure_class/failure_stage keys at all.
    #[test]
    fn pre_existing_ledger_line_without_failure_fields_still_deserializes() {
        let old_line = "{\"timestamp\":\"2099-01-01T00:00:00Z\",\"session_id\":\"1\",\"profile\":\"real\",\"display_name\":\"Real\",\"repo_id\":\"real\",\"repo\":\"owner/real\",\"local_path\":\"/tmp/repo\",\"provider\":\"github\",\"backend\":\"claude\",\"requested_backend\":\"claude\",\"effective_backend\":\"claude\",\"requested_model\":null,\"effective_model\":null,\"routing_reason\":\"explicit\",\"fallback_used\":false,\"confidence_impact\":null,\"human_required\":false,\"mode\":\"pm\",\"target_summary\":\"x\",\"branch\":null,\"session_dir\":null,\"duration_seconds\":1.0,\"backend_exit_code\":0,\"validation_result\":\"not_run\",\"commit_attempted\":false,\"commit_created\":false,\"push_attempted\":false,\"push_succeeded\":false,\"mr_attempted\":false,\"mr_created\":false,\"mr_url\":null,\"files_changed\":null,\"insertions\":null,\"deletions\":null,\"error_summary\":null,\"usage\":{\"input_tokens\":null,\"output_tokens\":null,\"total_tokens\":null,\"estimated_cost_usd\":null,\"usage_source\":null}}";
        let parsed: LedgerEntry = serde_json::from_str(old_line).unwrap();
        assert_eq!(parsed.failure_class, None);
        assert_eq!(parsed.failure_stage, None);
        assert_eq!(parsed.routing_diagnostics, None);
        assert_eq!(parsed.work_id, None);
        assert_eq!(parsed.work_title, None);
        assert_eq!(parsed.profile, "real");
    }

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
        let entries = super::read_entries(&cfg).unwrap();
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
        let mut entry =
            LedgerEntry::new("test", &profile(), "codex", "improve", "task", None, None);
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

    // TICKET-125: Tests for the new grouping functionality

    #[test]
    fn backfill_review_verdict_attributes_to_implementation_entry_not_reviewer() {
        let (_tmp, cfg) = test_config();
        let mut impl_entry =
            LedgerEntry::new("test", &profile(), "vibe", "improve", "test1", None, None);
        impl_entry.effective_backend = "vibe".to_string();
        impl_entry.branch = Some("gah/gah-123".to_string());
        append(&cfg, &impl_entry).unwrap();

        // The review dispatch's own entry -- a different backend (the
        // reviewer), must not be the one that ends up carrying the verdict.
        let mut review_entry =
            LedgerEntry::new("test", &profile(), "claude", "review", "test1", None, None);
        review_entry.effective_backend = "claude".to_string();
        review_entry.branch = Some("gah/gah-123".to_string());
        append(&cfg, &review_entry).unwrap();

        let found = backfill_review_verdict(
            &cfg,
            "gah/gah-123",
            "NEEDS_FIX",
            "high",
            "claude",
            Some("claude-sonnet-4"),
        )
        .unwrap();
        assert!(found);

        let entries = read_entries(&cfg).unwrap();
        let updated_impl = entries
            .iter()
            .find(|e| e.mode == "improve")
            .expect("implementation entry still present");
        assert_eq!(updated_impl.effective_backend, "vibe");
        assert_eq!(updated_impl.review_verdict.as_deref(), Some("NEEDS_FIX"));
        assert_eq!(updated_impl.reviewer_backend.as_deref(), Some("claude"));

        let review_entry_after = entries
            .iter()
            .find(|e| e.mode == "review")
            .expect("review entry still present");
        assert_eq!(
            review_entry_after.review_verdict, None,
            "the reviewer's own entry must not be the one carrying the verdict"
        );
    }

    #[test]
    fn backfill_review_verdict_returns_false_when_no_matching_branch() {
        let (_tmp, cfg) = test_config();
        let found = backfill_review_verdict(
            &cfg,
            "gah/no-such-branch",
            "APPROVE_STRONG",
            "high",
            "codex",
            None,
        )
        .unwrap();
        assert!(!found);
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
        let mut entry1 =
            LedgerEntry::new("test", &profile(), "codex", "improve", "test1", None, None);
        entry1.effective_backend = "codex".to_string();
        entry1.effective_model = Some("claude-sonnet".to_string());
        entry1.review_verdict = Some("APPROVE_STRONG".to_string());
        entry1.review_confidence = Some("high".to_string());
        entry1.reviewer_backend = Some("codex".to_string());
        entry1.reviewer_model = Some("claude-sonnet".to_string());
        entry1.validation_result = Some("passed".to_string());
        entry1.usage.actual_cost_usd = Some(0.5);

        let mut entry2 =
            LedgerEntry::new("test", &profile(), "vibe", "improve", "test2", None, None);
        entry2.effective_backend = "vibe".to_string();
        entry2.effective_model = Some("mistral-medium".to_string());
        entry2.review_verdict = Some("NEEDS_FIX".to_string());
        entry2.review_confidence = Some("medium".to_string());
        entry2.reviewer_backend = Some("vibe".to_string());
        entry2.reviewer_model = Some("mistral-medium".to_string());
        entry2.validation_result = Some("failed".to_string());
        entry2.usage.actual_cost_usd = Some(0.3);

        let entries = vec![entry1, entry2];
        let grouped = super::summary::build_grouped_summary(&entries, |entry| {
            entry.effective_backend.clone()
        });

        assert!(grouped.is_some());
        let grouped = grouped.unwrap();
        assert_eq!(grouped.len(), 2);

        // Find codex group
        let codex_group = grouped.iter().find(|g| g.group_key == "codex").unwrap();
        assert_eq!(codex_group.entries, 1);
        assert_eq!(codex_group.validation_pass, 1);
        assert_eq!(
            codex_group
                .review_verdict_distribution
                .get("APPROVE_STRONG"),
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
    fn build_grouped_summary_by_model() {
        let (_tmp, _cfg) = test_config();
        let mut entry1 =
            LedgerEntry::new("test", &profile(), "codex", "improve", "test1", None, None);
        entry1.effective_backend = "codex".to_string();
        entry1.effective_model = Some("gpt-4".to_string());
        entry1.review_verdict = Some("APPROVE_STRONG".to_string());
        entry1.validation_result = Some("passed".to_string());
        entry1.usage.actual_cost_usd = Some(1.0);

        let mut entry2 =
            LedgerEntry::new("test", &profile(), "codex", "improve", "test2", None, None);
        entry2.effective_backend = "codex".to_string();
        entry2.effective_model = Some("gpt-4".to_string());
        entry2.review_verdict = Some("APPROVE_STRONG".to_string());
        entry2.validation_result = Some("passed".to_string());
        entry2.usage.actual_cost_usd = Some(2.0);

        let mut entry3 =
            LedgerEntry::new("test", &profile(), "vibe", "improve", "test3", None, None);
        entry3.effective_backend = "vibe".to_string();
        entry3.effective_model = Some("mistral-medium".to_string());
        entry3.review_verdict = Some("REJECT".to_string());
        entry3.validation_result = Some("failed".to_string());
        entry3.usage.actual_cost_usd = Some(0.5);

        let entries = vec![entry1, entry2, entry3];
        let grouped = super::summary::build_grouped_summary(&entries, |entry| {
            entry.effective_model.clone().unwrap_or_default()
        });

        assert!(grouped.is_some());
        let grouped = grouped.unwrap();
        assert_eq!(grouped.len(), 2); // gpt-4 and mistral-medium

        // Find gpt-4 group
        let gpt4_group = grouped.iter().find(|g| g.group_key == "gpt-4").unwrap();
        assert_eq!(gpt4_group.entries, 2);
        assert_eq!(gpt4_group.validation_pass, 2);
        assert_eq!(
            gpt4_group.review_verdict_distribution.get("APPROVE_STRONG"),
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
    fn build_grouped_summary_empty_entries() {
        let entries: Vec<LedgerEntry> = vec![];
        let grouped = super::summary::build_grouped_summary(&entries, |entry| {
            entry.effective_backend.clone()
        });
        assert!(grouped.is_none());
    }
}
