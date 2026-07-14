//! Append-only reconciliation of dispatched work with later provider outcomes.
//!
//! This module records merge, close, and state-change observations separately
//! from `ledger.jsonl`. It never rewrites dispatch history and only appends when
//! a work item's classified state changes from its last known reconciliation.

use crate::config::{self, GahConfig};
use crate::ledger::{read_entries, LedgerEntry};
use crate::models::PolicyConfig;
use crate::sync;
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn default_record_type() -> String {
    "mr_state".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ReconciliationEntry {
    pub timestamp: String,
    #[serde(default = "default_record_type")]
    pub record_type: String,
    pub work_id: String,
    pub branch: Option<String>,
    pub mr_url: Option<String>,
    pub previous_state: Option<String>,
    pub new_state: String,
    pub source: String,
    #[serde(default)]
    pub mr_id: Option<String>,
    #[serde(default)]
    pub source_issue_number: Option<String>,
    #[serde(default)]
    pub previous_issue_state: Option<String>,
    #[serde(default)]
    pub resulting_issue_state: Option<String>,
    #[serde(default)]
    pub issue_closure_mode: Option<String>,
    #[serde(default)]
    pub issue_closure_classification: Option<String>,
    #[serde(default)]
    pub issue_closure_reason: Option<String>,
}

#[derive(Debug, Serialize, Default)]
struct ReconciliationIssueClosureReport {
    already_closed: Vec<String>,
    would_close: Vec<String>,
    closed: Vec<String>,
    ambiguous: Vec<String>,
    unmapped: Vec<String>,
    leave_open: Vec<String>,
    observation_failed: Vec<String>,
    policy_blocked: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ReconciliationReport {
    new_entries: Vec<ReconciliationEntry>,
    issue_closure: ReconciliationIssueClosureReport,
}

#[derive(Debug)]
enum MappingResolution {
    Proven {
        issue_number: String,
        reason: String,
    },
    Ambiguous {
        reason: String,
    },
    Unmapped,
}

#[derive(Debug)]
struct IssueClosureDecision {
    issue_number: Option<String>,
    mode: &'static str,
    classification: &'static str,
    reason: Option<String>,
    previous_issue_state: Option<String>,
    resulting_issue_state: Option<String>,
}

type IssueClosureKey = (String, String);
type IssueClosureSnapshot = (Option<String>, Option<String>, Option<String>);

pub fn read_reconciliation_entries(cfg: &GahConfig) -> Result<Vec<ReconciliationEntry>> {
    let path = cfg.defaults.reconciliation_path();
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
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
        fs::create_dir_all(parent)
            .with_context(|| format!("creating reconciliation directory {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening reconciliation log {}", path.display()))?;
    let mut value = serde_json::to_value(entry).context("serializing reconciliation entry")?;
    crate::redact::redact_json_value(&mut value);
    serde_json::to_writer(&mut file, &value).context("serializing reconciliation entry")?;
    file.write_all(b"\n")
        .context("writing reconciliation newline")?;
    Ok(())
}

/// Most recent recorded state per work_id (reconciliation log is
/// chronological, so the last entry for a given work_id wins).
fn last_known_states(entries: &[ReconciliationEntry]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for entry in entries {
        if entry.record_type != "mr_state" {
            continue;
        }
        map.insert(entry.work_id.clone(), entry.new_state.clone());
    }
    map
}

fn last_known_issue_closure(
    entries: &[ReconciliationEntry],
) -> BTreeMap<IssueClosureKey, IssueClosureSnapshot> {
    let mut map = BTreeMap::new();
    for entry in entries {
        if entry.record_type != "issue_closure" {
            continue;
        }
        let Some(issue_number) = entry.source_issue_number.clone() else {
            continue;
        };
        map.insert(
            (entry.work_id.clone(), issue_number),
            (
                entry.issue_closure_mode.clone(),
                entry.resulting_issue_state.clone(),
                entry.issue_closure_classification.clone(),
            ),
        );
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

pub fn run(cfg: &GahConfig, profile_name: &str, json: bool, dry_run: bool) -> Result<()> {
    let profile = config::get_profile(cfg, profile_name)?;
    let ledger_entries = read_entries(cfg)?;
    let history = read_reconciliation_entries(cfg)?;
    let mut last_known = last_known_states(&history);
    let mut last_issue_closure = last_known_issue_closure(&history);
    let dispatch_identity = latest_dispatch_identity(&ledger_entries);
    let entries_by_work_id = crate::ledger::index_entries_by_work_id(&ledger_entries);

    let mrs = sync::fetch_mrs(profile)?;

    let mut new_entries = vec![];
    let mut issue_closure = ReconciliationIssueClosureReport::default();
    for (work_id, (branch, mr_url)) in &dispatch_identity {
        let matching_mr = mrs.iter().find(|mr| {
            branch.as_deref() == Some(mr.branch.as_str())
                || (mr_url.is_some() && mr_url.as_deref() == mr.url.as_deref())
        });
        let Some(mr) = matching_mr else { continue };
        let new_state = sync::classify(mr).to_string();
        let previous = last_known.get(work_id).cloned();
        if previous.as_deref() != Some(new_state.as_str()) {
            let entry = ReconciliationEntry {
                timestamp: OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .unwrap_or_default(),
                record_type: "mr_state".to_string(),
                work_id: work_id.clone(),
                branch: branch.clone(),
                mr_url: mr.url.clone(),
                previous_state: previous,
                new_state: new_state.clone(),
                source: "sync".to_string(),
                mr_id: mr.id.clone(),
                source_issue_number: None,
                previous_issue_state: None,
                resulting_issue_state: None,
                issue_closure_mode: None,
                issue_closure_classification: None,
                issue_closure_reason: None,
            };
            if !dry_run {
                append_reconciliation_entry(cfg, &entry)?;
            }
            last_known.insert(work_id.clone(), new_state.clone());
            new_entries.push(entry);
        }

        if new_state.as_str() == "MERGED" {
            let dispatch_entries = entries_by_work_id
                .get(work_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let decision = reconcile_source_issue_closure(
                cfg,
                profile,
                work_id,
                branch.clone(),
                mr,
                dispatch_entries,
                dry_run,
            )?;
            record_issue_closure_report(&mut issue_closure, &decision);
            if let Some(issue_number) = decision.issue_number.clone() {
                let prior = last_issue_closure
                    .get(&(work_id.clone(), issue_number.clone()))
                    .cloned();
                let current = (
                    Some(decision.mode.to_string()),
                    decision.resulting_issue_state.clone(),
                    Some(decision.classification.to_string()),
                );
                if prior != Some(current.clone())
                    && matches!(
                        decision.mode,
                        "provider_already_closed" | "gah_reconciliation_write"
                    )
                {
                    let entry = ReconciliationEntry {
                        timestamp: OffsetDateTime::now_utc()
                            .format(&Rfc3339)
                            .unwrap_or_default(),
                        record_type: "issue_closure".to_string(),
                        work_id: work_id.clone(),
                        branch: branch.clone(),
                        mr_url: mr.url.clone(),
                        previous_state: Some(new_state.clone()),
                        new_state: new_state.clone(),
                        source: "issue_closure".to_string(),
                        mr_id: mr.id.clone(),
                        source_issue_number: Some(issue_number.clone()),
                        previous_issue_state: decision.previous_issue_state.clone(),
                        resulting_issue_state: decision.resulting_issue_state.clone(),
                        issue_closure_mode: Some(decision.mode.to_string()),
                        issue_closure_classification: Some(decision.classification.to_string()),
                        issue_closure_reason: decision.reason.clone(),
                    };
                    if !dry_run {
                        append_reconciliation_entry(cfg, &entry)?;
                    }
                    new_entries.push(entry);
                    last_issue_closure.insert((work_id.clone(), issue_number), current);
                }
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string(&ReconciliationReport {
                new_entries,
                issue_closure,
            })?
        );
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
        println!(
            "Issue closure: already_closed={} would_close={} closed={} ambiguous={} unmapped={} leave_open={} observation_failed={} policy_blocked={}",
            issue_closure.already_closed.len(),
            issue_closure.would_close.len(),
            issue_closure.closed.len(),
            issue_closure.ambiguous.len(),
            issue_closure.unmapped.len(),
            issue_closure.leave_open.len(),
            issue_closure.observation_failed.len(),
            issue_closure.policy_blocked.len(),
        );
    }
    Ok(())
}

fn reconcile_source_issue_closure(
    cfg: &GahConfig,
    profile: &crate::config::Profile,
    work_id: &str,
    branch: Option<String>,
    mr: &sync::SyncMr,
    dispatch_entries: &[LedgerEntry],
    dry_run: bool,
) -> Result<IssueClosureDecision> {
    if !mr.merged {
        return Ok(IssueClosureDecision {
            issue_number: None,
            mode: "leave_open",
            classification: "UNMAPPED",
            reason: Some("mr_not_merged".to_string()),
            previous_issue_state: None,
            resulting_issue_state: None,
        });
    }

    let mapping = resolve_source_issue_mapping(mr.body.as_deref(), dispatch_entries);
    let (issue_number, classification, reason) = match mapping {
        MappingResolution::Proven {
            issue_number,
            reason,
        } => (Some(issue_number), "RESOLVED_BY_MERGE", Some(reason)),
        MappingResolution::Ambiguous { reason } => {
            return Ok(IssueClosureDecision {
                issue_number: None,
                mode: "ambiguous",
                classification: "AMBIGUOUS",
                reason: Some(reason),
                previous_issue_state: None,
                resulting_issue_state: None,
            });
        }
        MappingResolution::Unmapped => {
            return Ok(IssueClosureDecision {
                issue_number: None,
                mode: "unmapped",
                classification: "UNMAPPED",
                reason: None,
                previous_issue_state: None,
                resulting_issue_state: None,
            });
        }
    };
    let issue_number = issue_number.expect("proven mapping must include issue_number");

    let state = match profile.provider.as_str() {
        "github" => crate::provider::github_get_issue_state(profile, &issue_number).ok(),
        "gitlab" => crate::provider::gitlab_get_issue_state(profile, &issue_number).ok(),
        _ => None,
    };

    let Some(previous_issue_state) = state else {
        // If we can't observe the issue state (unsupported provider or observation failure),
        // treat it as observation_failed and leave the issue open
        return Ok(IssueClosureDecision {
            issue_number: Some(issue_number),
            mode: "observation_failed",
            classification,
            reason: Some("issue_state_observation_failed".to_string()),
            previous_issue_state: None,
            resulting_issue_state: None,
        });
    };

    if !matches!(
        previous_issue_state.as_deref(),
        Some("open") | Some("opened")
    ) {
        return Ok(IssueClosureDecision {
            issue_number: Some(issue_number),
            mode: "provider_already_closed",
            classification,
            reason,
            previous_issue_state: previous_issue_state.clone(),
            resulting_issue_state: previous_issue_state.clone(),
        });
    }

    if dry_run {
        return Ok(IssueClosureDecision {
            issue_number: Some(issue_number),
            mode: "dry_run",
            classification,
            reason,
            previous_issue_state: previous_issue_state.clone(),
            resulting_issue_state: Some("closed".to_string()),
        });
    }

    if !source_issue_closure_allowed(profile)? {
        return Ok(IssueClosureDecision {
            issue_number: Some(issue_number),
            mode: "policy_blocked",
            classification,
            reason,
            previous_issue_state: previous_issue_state.clone(),
            resulting_issue_state: Some("open".to_string()),
        });
    }

    match profile.provider.as_str() {
        "github" => crate::provider::github_close_issue(profile, &issue_number)?,
        "gitlab" => crate::provider::gitlab_close_issue(profile, &issue_number)?,
        _ => {}
    }

    let _ = (cfg, work_id, branch);
    Ok(IssueClosureDecision {
        issue_number: Some(issue_number),
        mode: "gah_reconciliation_write",
        classification,
        reason,
        previous_issue_state: previous_issue_state.clone(),
        resulting_issue_state: Some("closed".to_string()),
    })
}

fn source_issue_closure_allowed(profile: &crate::config::Profile) -> Result<bool> {
    if !profile.publishing.allow_source_issue_closure {
        return Ok(false);
    }
    let Some(policy_path) = &profile.policy_path else {
        return Ok(true);
    };
    let text = fs::read_to_string(policy_path)
        .with_context(|| format!("reading policy file: {}", policy_path))?;
    let cfg: PolicyConfig =
        toml::from_str(&text).with_context(|| format!("parsing policy file: {}", policy_path))?;
    let repo = cfg.repo;
    let allowed = match repo.trust_mode.as_str() {
        "read_only" => false,
        "draft_pr_allowed" => repo.allow_issue_write,
        // For any other trust mode, defer to the general issue write permission
        // This future-proofs the function for new trust modes
        _ => repo.allow_issue_write,
    };
    Ok(allowed)
}

fn resolve_source_issue_mapping(
    mr_body: Option<&str>,
    dispatch_entries: &[LedgerEntry],
) -> MappingResolution {
    let explicit = extract_closing_references(mr_body.unwrap_or_default());
    let structured: BTreeSet<String> = dispatch_entries
        .iter()
        .filter_map(|entry| entry.source_issue_number.clone())
        .collect();

    if structured.len() > 1 {
        return MappingResolution::Ambiguous {
            reason: "conflicting_structured_source_identity".to_string(),
        };
    }

    let structured_issue = structured.iter().next();

    // For GAH workflow semantics, multiple explicit closing references should be
    // intentionally classified as ambiguous to maintain one-to-one issue-to-PR mapping.
    // However, if there's a structured source and one explicit reference matches it,
    // we allow it to support the common case where the agent correctly closes the target issue.
    if explicit.len() > 1 {
        if let Some(expected_issue) = structured_issue {
            // If one of the explicit references matches the structured source, use it
            // This handles the case: dispatched for #42, PR body says "Closes #42, fixes #43"
            if explicit.contains(expected_issue) {
                return MappingResolution::Proven {
                    issue_number: expected_issue.clone(),
                    reason: "explicit_closing_reference_matching_structured_source".to_string(),
                };
            }
        }
        // Multiple explicit references with no matching structured source is ambiguous
        // This maintains GAH's one-to-one invariant for multi-issue PRs
        return MappingResolution::Ambiguous {
            reason: "multiple_explicit_closing_references".to_string(),
        };
    }

    match (explicit.iter().next(), structured_issue) {
        (Some(explicit_issue), Some(structured_issue)) if explicit_issue != structured_issue => {
            MappingResolution::Ambiguous {
                reason: "explicit_and_structured_issue_conflict".to_string(),
            }
        }
        (Some(explicit_issue), Some(_)) => MappingResolution::Proven {
            issue_number: explicit_issue.clone(),
            reason: "explicit_closing_reference+structured_source_identity".to_string(),
        },
        (Some(explicit_issue), None) => MappingResolution::Proven {
            issue_number: explicit_issue.clone(),
            reason: "explicit_closing_reference".to_string(),
        },
        (None, Some(structured_issue)) => MappingResolution::Proven {
            issue_number: structured_issue.clone(),
            reason: "structured_source_identity".to_string(),
        },
        (None, None) => MappingResolution::Unmapped,
    }
}

fn extract_closing_references(text: &str) -> BTreeSet<String> {
    let reference_re = Regex::new(
        r"(?i)\b(close|closes|closed|fix|fixes|fixed|resolve|resolves|resolved)\b\s+#([0-9]+)\b",
    )
    .expect("closing reference regex must compile");
    let mut found = BTreeSet::new();
    for line in text.lines() {
        for caps in reference_re.captures_iter(line) {
            let Some(matched) = caps.get(0) else { continue };
            let prefix = line[..matched.start()].trim_end();
            let prev_token = prefix
                .rsplit(|c: char| c.is_whitespace() || matches!(c, '(' | '[' | ':' | ';' | ','))
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '\'')
                .to_ascii_lowercase();
            if prev_token == "not" || prev_token == "doesn't" || prev_token == "doesnt" {
                continue;
            }
            if let Some(issue_number) = caps.get(2) {
                found.insert(issue_number.as_str().to_string());
            }
        }
    }
    found
}

fn record_issue_closure_report(
    report: &mut ReconciliationIssueClosureReport,
    decision: &IssueClosureDecision,
) {
    let issue = decision
        .issue_number
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    match decision.mode {
        "provider_already_closed" => report.already_closed.push(issue),
        "dry_run" => report.would_close.push(issue),
        "gah_reconciliation_write" => report.closed.push(issue),
        "ambiguous" => report.ambiguous.push(issue),
        "unmapped" => report.unmapped.push(issue),
        "observation_failed" => report.observation_failed.push(issue),
        "policy_blocked" => report.policy_blocked.push(issue),
        _ => report.leave_open.push(issue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::test_util::test_config;
    use std::fs;

    #[test]
    fn reconciliation_log_is_empty_when_file_does_not_exist() {
        let (_tmp, cfg) = test_config();
        let entries = read_reconciliation_entries(&cfg).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn reconciliation_entries_round_trip_through_jsonl() {
        use ReconciliationEntry;
        // test_config() gives a fresh tempdir-backed artifact_root per test,
        // so reconciliation_path() resolves there without touching any
        // process-global env var (avoids the GAH_LEDGER_PATH-style test
        // race this project's own docs call out as known technical debt).
        let (_tmp, cfg) = test_config();
        let path = cfg.defaults.reconciliation_path();

        let entry = ReconciliationEntry {
            timestamp: "2026-07-05T00:00:00Z".into(),
            record_type: "mr_state".into(),
            work_id: "TICKET-072".into(),
            branch: Some("gah/real-1".into()),
            mr_url: Some("https://github.com/owner/repo/pull/1".into()),
            previous_state: None,
            new_state: "NEEDS_REVIEW".into(),
            source: "sync".into(),
            mr_id: None,
            source_issue_number: None,
            previous_issue_state: None,
            resulting_issue_state: None,
            issue_closure_mode: None,
            issue_closure_classification: None,
            issue_closure_reason: None,
        };
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&entry).unwrap()),
        )
        .unwrap();

        let entries = read_reconciliation_entries(&cfg).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], entry);
    }

    #[test]
    fn reconciliation_malformed_line_fails_loudly() {
        let (_tmp, cfg) = test_config();
        let path = cfg.defaults.reconciliation_path();
        fs::write(&path, "not valid json\n").unwrap();

        let result = read_reconciliation_entries(&cfg);
        assert!(result.is_err());
    }
}
