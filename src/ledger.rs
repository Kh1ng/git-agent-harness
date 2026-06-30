use crate::config::{GahConfig, Profile};
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Serialize, Default, Clone)]
pub struct LedgerUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub usage_source: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
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

#[cfg(test)]
mod tests {
    use super::{append, LedgerEntry};
    use crate::config::{Defaults, GahConfig, Profile};
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
