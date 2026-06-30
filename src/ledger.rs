use crate::config::{GahConfig, Profile};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

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
    pub mode: String,
    pub target_summary: Option<String>,
    pub branch: Option<String>,
    pub session_dir: Option<String>,
    pub duration_seconds: Option<f64>,
    pub backend_exit_code: Option<i32>,
    pub validation_result: Option<String>,
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
            mode: mode.to_string(),
            target_summary: summarize_target(target),
            branch: None,
            session_dir: session_dir.map(|p| p.display().to_string()),
            duration_seconds: None,
            backend_exit_code: None,
            validation_result: None,
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
            usage: LedgerUsage::default(),
        }
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

pub mod summary {
    use super::{read_entries, LedgerEntry};
    use crate::config;
    use anyhow::Result;
    use std::collections::HashMap;
    use time::{Duration, OffsetDateTime};

    pub fn run(since: &str, profile: Option<&str>, config_path: Option<&str>) -> Result<()> {
        let cfg = config::load(config_path)?;
        let cutoff = parse_since(since)?;
        let mut entries = read_entries(&cfg)?;
        if let Some(profile) = profile {
            entries.retain(|entry| entry.profile == profile);
        }
        entries.retain(|entry| entry.timestamp >= cutoff);

        println!("Ledger: {}", cfg.defaults.ledger_path().display());
        println!("Entries: {}", entries.len());
        if entries.is_empty() {
            return Ok(());
        }

        let mut by_mode: HashMap<String, usize> = HashMap::new();
        let mut by_backend: HashMap<String, usize> = HashMap::new();
        let mut by_requested_backend: HashMap<String, usize> = HashMap::new();
        let mut success = 0usize;
        let mut failed = 0usize;
        let mut fallback = 0usize;
        let mut validation_pass = 0usize;
        let mut push_success = 0usize;
        let mut mr_count = 0usize;
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

        println!("Success: {}", success);
        println!("Failed:  {}", failed);
        println!("By mode:");
        print_counts(&by_mode);
        println!("Requested backend:");
        print_counts(&by_requested_backend);
        println!("By backend:");
        print_counts(&by_backend);
        println!("Fallbacks: {}", fallback);
        println!(
            "Validation pass rate: {}/{}",
            validation_pass,
            entries.len()
        );
        println!("Push success rate: {}/{}", push_success, entries.len());
        println!("MR count: {}", mr_count);
        if duration_count > 0 {
            println!(
                "Average duration: {:.1}s",
                duration_total / duration_count as f64
            );
        }
        println!(
            "Usage totals: input={} output={} total={} requests={}",
            input_tokens, output_tokens, total_tokens, requests_count
        );
        if estimated_cost_seen {
            println!("Estimated cost total: ${:.4}", estimated_cost);
        }
        if actual_cost_seen {
            println!("Actual cost total: ${:.4}", actual_cost);
        }
        if let Some(last) = entries.last() {
            println!(
                "Last run: {} {} {} {}",
                last.timestamp, last.profile, last.mode, last.effective_backend
            );
        }
        Ok(())
    }

    fn print_counts(counts: &HashMap<String, usize>) {
        let mut pairs: Vec<_> = counts.iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        for (key, count) in pairs {
            println!("  {:<16} {}", key, count);
        }
    }

    fn parse_since(input: &str) -> Result<String> {
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

    #[allow(dead_code)]
    fn _success(entry: &LedgerEntry) -> bool {
        entry.error_summary.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::{append, LedgerEntry};
    use crate::config::{Defaults, GahConfig, Profile, RoutingPolicy};
    use std::collections::HashMap;

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
            claude_args: vec![],
            policy_path: None,
            env_file: None,
            env_file_prod: None,
            validation_commands: vec![],
            test_file_patterns: vec![],
            model_improve: None,
            model_pm: None,
            model_review: None,
            routing: RoutingPolicy::default(),
        }
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
}
