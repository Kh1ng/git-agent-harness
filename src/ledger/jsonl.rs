use crate::config::GahConfig;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{LazyLock, Mutex};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::entry::LedgerEntry;

#[cfg(test)]
static READ_ENTRIES_CALLS: LazyLock<Mutex<std::collections::HashMap<PathBuf, usize>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Resolve current paid-route grants for one work item. Later grant/revoke
/// entries supersede earlier ones for the same exact backend/model identity.
#[allow(dead_code)]
pub fn active_paid_route_approvals(
    cfg: &GahConfig,
    profile_name: &str,
    work_id: &str,
) -> Result<HashSet<(String, Option<String>)>> {
    let entries = read_entries(cfg)?;
    Ok(active_paid_route_approvals_from_entries(
        &entries,
        profile_name,
        work_id,
    ))
}

pub fn active_paid_route_approvals_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    work_id: &str,
) -> HashSet<(String, Option<String>)> {
    let mut active = HashSet::new();
    let aliases = work_id_aliases(work_id);
    for entry in entries {
        if entry.profile != profile_name
            || !entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
        {
            continue;
        }
        let identity = (
            entry.effective_backend.clone(),
            entry.effective_model.clone(),
        );
        match entry.mode.as_str() {
            "paid_route_approval_grant" => {
                active.insert(identity);
            }
            "paid_route_approval_revoke" => {
                active.remove(&identity);
            }
            _ => {}
        }
    }
    active
}

/// A manager review is a much shorter-lived action than a full dispatch
/// attempt (`CLAIM_STALE_AFTER_HOURS` in dispatch.rs is 6h) -- 2h is a
/// generous margin for a real review session while still releasing a hold
/// left behind by a crashed/forgotten manager session same-day.
pub const REVIEW_HOLD_STALE_AFTER_HOURS: i64 = 2;

fn is_review_hold_stale(entry: &LedgerEntry) -> bool {
    let entry_time = if let Ok(parsed) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
        parsed
    } else if let Ok(secs) = entry.timestamp.parse::<i64>() {
        if let Ok(dt) = OffsetDateTime::from_unix_timestamp(secs) {
            dt
        } else {
            return true;
        }
    } else {
        return true;
    };
    let now = OffsetDateTime::now_utc();
    now - entry_time > time::Duration::hours(REVIEW_HOLD_STALE_AFTER_HOURS)
}

/// Scans this profile's ledger entries for active review holds (`gah hold
/// set` with no later `gah hold clear`, and not yet stale) so
/// `decide_next_action` can skip auto-merging a work_id a manager session is
/// actively reviewing out of band. Entries are appended in chronological
/// order, so a single forward pass where each hold/release overwrites the
/// previous verdict for its work_id naturally lands on the latest one.
#[allow(dead_code)]
pub fn active_review_hold_work_ids(
    cfg: &GahConfig,
    profile_name: &str,
) -> std::collections::HashSet<String> {
    let entries = match read_entries(cfg) {
        Ok(entries) => entries,
        Err(_) => return std::collections::HashSet::new(),
    };
    active_review_hold_work_ids_from_entries(&entries, profile_name)
}

pub fn active_review_hold_work_ids_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
) -> std::collections::HashSet<String> {
    let mut held: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for entry in entries.iter().filter(|e| e.profile == profile_name) {
        let Some(work_id) = entry.work_id.as_deref() else {
            continue;
        };
        match entry.mode.as_str() {
            "review_hold" => {
                held.insert(work_id.to_string(), !is_review_hold_stale(entry));
            }
            "review_hold_release" => {
                held.insert(work_id.to_string(), false);
            }
            _ => {}
        }
    }

    held.into_iter()
        .filter_map(|(work_id, active)| active.then_some(work_id))
        .collect()
}

fn lock_ledger(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating ledger directory {}", parent.display()))?;
    }
    // Dispatch workers append and review backfills concurrently. Serialize
    // both operations with a sidecar lock so a backfill cannot rewrite a
    // stale snapshot and erase another worker's append.
    let lock_path = path.with_extension("lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening ledger lock {}", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("locking ledger {}", lock_path.display()))?;
    Ok(lock_file)
}

fn append_locked(cfg: &GahConfig, path: &Path, entry: &LedgerEntry) -> Result<()> {
    if let Some(offset) = truncated_tail_offset(&fs::read(path).unwrap_or_default()) {
        anyhow::bail!(
            "ledger {} has an unterminated invalid final record at byte {}; run `gah ledger repair-tail` before appending",
            path.display(),
            offset
        );
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening ledger {}", path.display()))?;
    let mut value = serde_json::to_value(entry).context("serializing ledger entry")?;
    crate::redact::redact_json_value(&mut value);
    serde_json::to_writer(&mut file, &value).context("serializing ledger entry")?;
    file.write_all(b"\n").context("writing ledger newline")?;
    drop(file);

    // Redundant SQLite mirror, kept in lockstep with the JSONL file (still
    // the sole source of truth). Best-effort: a mirror failure must never
    // fail the real dispatch write path.
    if let Err(err) = super::sqlite_store::sync_from_jsonl(cfg) {
        eprintln!("warning: failed to sync sqlite ledger mirror: {err:#}");
    }

    Ok(())
}

pub fn append(cfg: &GahConfig, entry: &LedgerEntry) -> Result<PathBuf> {
    let path = cfg.defaults.ledger_path();
    let _lock = lock_ledger(&path)?;
    append_locked(cfg, &path, entry)?;

    Ok(path)
}

/// Atomically append a human gate only when the same work item has no active
/// effective gate. The check and append share the ledger's cross-process lock,
/// so concurrent controller slots cannot both observe "ungated" and write
/// duplicate transitions.
pub fn append_human_gate_if_transition(cfg: &GahConfig, entry: &LedgerEntry) -> Result<bool> {
    anyhow::ensure!(
        entry.human_required,
        "conditional gate entry must require human action"
    );
    let work_id = entry
        .work_id
        .as_deref()
        .context("conditional gate entry must have a work_id")?;
    let path = cfg.defaults.ledger_path();
    let _lock = lock_ledger(&path)?;
    let existing = if path.exists() {
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        parse_jsonl_entries(&text, &path, 0)?
    } else {
        Vec::new()
    };
    if effective_human_gate_from_entries(&existing, &entry.profile, &entry.repo_id, work_id)
        .is_some()
    {
        return Ok(false);
    }
    append_locked(cfg, &path, entry)?;
    Ok(true)
}

/// Result of the deliberately narrow JSONL tail-repair operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailRepair {
    pub backup_path: Option<PathBuf>,
    pub dropped_bytes: usize,
}

/// Back up and remove a single unterminated, invalid final JSONL record.
///
/// A full disk or abrupt process death can leave the last `write_all` only
/// partially persisted. We never repair an invalid record followed by a
/// newline (that is structural corruption, not a torn append), and we never
/// touch any valid final record. The rejected bytes are retained beside the
/// ledger before truncation for audit/recovery.
pub fn repair_truncated_tail(cfg: &GahConfig, dry_run: bool) -> Result<TailRepair> {
    let path = cfg.defaults.ledger_path();
    if !path.exists() {
        return Ok(TailRepair {
            backup_path: None,
            dropped_bytes: 0,
        });
    }
    let lock_path = path.with_extension("lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening ledger lock {}", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("locking ledger {}", lock_path.display()))?;
    repair_truncated_tail_at(&path, dry_run)
}

fn repair_truncated_tail_at(path: &Path, dry_run: bool) -> Result<TailRepair> {
    let bytes = fs::read(path).with_context(|| format!("reading ledger {}", path.display()))?;
    let Some(offset) = truncated_tail_offset(&bytes) else {
        return Ok(TailRepair {
            backup_path: None,
            dropped_bytes: 0,
        });
    };
    let tail = &bytes[offset..];
    let backup_path = path.with_file_name(format!(
        "{}.corrupt-tail-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("ledger"),
        OffsetDateTime::now_utc().unix_timestamp_nanos(),
    ));
    if !dry_run {
        fs::write(&backup_path, tail).with_context(|| {
            format!(
                "backing up truncated ledger tail to {}",
                backup_path.display()
            )
        })?;
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .with_context(|| format!("opening ledger {} for tail repair", path.display()))?;
        file.set_len(offset as u64)
            .with_context(|| format!("truncating ledger {}", path.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing repaired ledger {}", path.display()))?;
    }
    Ok(TailRepair {
        backup_path: Some(backup_path),
        dropped_bytes: tail.len(),
    })
}

/// Returns the byte offset of a physically torn final JSONL record. A final
/// newline means the record was fully framed, so any invalid content is not
/// safe to discard automatically.
fn truncated_tail_offset(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return None;
    }
    let offset = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |idx| idx + 1);
    let tail = &bytes[offset..];
    if serde_json::from_slice::<serde_json::Value>(tail).is_err() {
        Some(offset)
    } else {
        None
    }
}

pub(super) fn parse_jsonl_entries(
    text: &str,
    path: &Path,
    starting_line: usize,
) -> Result<Vec<LedgerEntry>> {
    let mut entries = vec![];
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<LedgerEntry>(line).with_context(|| {
            format!(
                "parsing ledger entry {} from {}",
                starting_line + idx + 1,
                path.display()
            )
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

pub fn read_entries(cfg: &GahConfig) -> Result<Vec<LedgerEntry>> {
    let path = cfg.defaults.ledger_path();
    #[cfg(test)]
    {
        *READ_ENTRIES_CALLS
            .lock()
            .expect("ledger read counter lock poisoned")
            .entry(path.clone())
            .or_default() += 1;
    }
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    parse_jsonl_entries(&text, &path, 0)
}

#[cfg(test)]
pub(crate) fn reset_read_entries_call_count(cfg: &GahConfig) {
    READ_ENTRIES_CALLS
        .lock()
        .expect("ledger read counter lock poisoned")
        .remove(&cfg.defaults.ledger_path());
}

#[cfg(test)]
pub(crate) fn read_entries_call_count(cfg: &GahConfig) -> usize {
    READ_ENTRIES_CALLS
        .lock()
        .expect("ledger read counter lock poisoned")
        .get(&cfg.defaults.ledger_path())
        .copied()
        .unwrap_or(0)
}

/// TICKET-125: review mode's own ledger entry records the reviewer's
/// backend/model, not the implementation's -- grouping/cost-vs-quality
/// reporting needs the verdict attributed back to whichever backend
/// actually wrote the code being reviewed. Finds the most recent fix/improve
/// entry for `branch` that doesn't already have a verdict and updates it
/// in place (the ledger has no other mutation path today; this is the one
/// exception, and it's rare enough -- once per review completion -- not to
/// need more than a full read-modify-write of the file).
pub struct ReviewVerdictBackfill<'a> {
    pub verdict: &'a str,
    pub confidence: &'a str,
    pub reviewer_backend: &'a str,
    pub reviewer_model: Option<&'a str>,
    pub reviewer_tier: Option<&'a str>,
    pub review_gate_reason: Option<&'a str>,
    pub review_source_sha: Option<&'a str>,
    pub review_metadata_fingerprint: Option<&'a str>,
    pub blocking_findings: &'a [String],
    pub actionable_findings: &'a [crate::models::ActionableReviewFinding],
    pub non_blocking_findings: &'a [String],
    pub risk_notes: &'a [String],
    pub evidence: &'a [String],
    pub compatibility_evidence: &'a [String],
}

pub fn backfill_review_verdict(
    cfg: &GahConfig,
    branch: &str,
    backfill: ReviewVerdictBackfill<'_>,
) -> Result<bool> {
    let path = cfg.defaults.ledger_path();
    let lock_path = path.with_extension("lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening ledger lock {}", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("locking ledger {}", lock_path.display()))?;

    let mut entries = read_entries(cfg)?;
    let target_idx = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.branch.as_deref() == Some(branch) && matches!(e.mode.as_str(), "fix" | "improve")
        })
        .max_by_key(|(_, e)| e.timestamp.clone())
        .map(|(idx, _)| idx);

    let Some(idx) = target_idx else {
        return Ok(false);
    };
    entries[idx].review_verdict = Some(backfill.verdict.to_string());
    entries[idx].review_confidence = Some(backfill.confidence.to_string());
    entries[idx].reviewer_backend = Some(backfill.reviewer_backend.to_string());
    entries[idx].reviewer_model = backfill.reviewer_model.map(str::to_string);
    entries[idx].reviewer_tier = backfill.reviewer_tier.map(str::to_string);
    entries[idx].review_gate_reason = backfill.review_gate_reason.map(str::to_string);
    entries[idx].review_source_sha = backfill.review_source_sha.map(str::to_string);
    entries[idx].review_metadata_fingerprint =
        backfill.review_metadata_fingerprint.map(str::to_string);
    entries[idx].review_blocking_findings = backfill.blocking_findings.to_vec();
    entries[idx].review_actionable_findings = backfill.actionable_findings.to_vec();
    entries[idx].review_non_blocking_findings = backfill.non_blocking_findings.to_vec();
    entries[idx].review_risk_notes = backfill.risk_notes.to_vec();
    entries[idx].review_evidence = backfill.evidence.to_vec();
    entries[idx].review_compatibility_evidence = backfill.compatibility_evidence.to_vec();

    let mut out = String::new();
    for entry in &entries {
        let mut value = serde_json::to_value(entry).context("serializing ledger entry")?;
        crate::redact::redact_json_value(&mut value);
        out.push_str(&serde_json::to_string(&value).context("serializing ledger entry")?);
        out.push('\n');
    }
    fs::write(&path, out).with_context(|| format!("rewriting ledger {}", path.display()))?;

    if let Err(err) = super::sqlite_store::rebuild_from_jsonl(cfg) {
        eprintln!("warning: failed to sync sqlite ledger mirror: {err:#}");
    }

    Ok(true)
}

/// TICKET-096: the query sync/reconciliation needs to associate a
/// `SyncMr.work_id` (extracted from a PR/MR title) back to the ledger
/// entries that dispatched it. No new sync-side structure required.
pub fn entries_for_work_id(cfg: &GahConfig, work_id: &str) -> Result<Vec<LedgerEntry>> {
    let aliases = work_id_aliases(work_id);
    Ok(read_entries(cfg)?
        .into_iter()
        .filter(|e| {
            e.work_id
                .as_deref()
                .is_some_and(|entry_id| aliases.iter().any(|alias| alias == entry_id))
        })
        .collect())
}

/// True only when this profile/repository's exact work item and immutable
/// source commit already received a completed review from the same authority
/// class after its latest operator reset. Missing legacy attribution fails
/// open: it may cost a review, but never suppresses one.
pub fn review_already_exists(
    cfg: &GahConfig,
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
    source_sha: &str,
    metadata_fingerprint: &str,
    reviewer_class: &str,
) -> Result<bool> {
    let aliases = work_id_aliases(work_id);
    let entries = read_entries(cfg)?;
    let reset_index = entries.iter().rposition(|entry| {
        entry.profile == profile_name
            && entry.repo_id == repo_id
            && entry.mode == "clear_attempts"
            && entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
    });
    let active_entries = reset_index.map_or(entries.as_slice(), |index| &entries[index + 1..]);
    Ok(active_entries.iter().any(|entry| {
        entry.profile == profile_name
            && entry.repo_id == repo_id
            && entry.mode == "review"
            && entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
            && entry.review_source_sha.as_deref() == Some(source_sha)
            && entry.review_metadata_fingerprint.as_deref() == Some(metadata_fingerprint)
            && entry.reviewer_class.as_deref() == Some(reviewer_class)
            && review_is_dedup_eligible(entry)
    }))
}

fn review_is_dedup_eligible(entry: &LedgerEntry) -> bool {
    // The reviewer may have completed while publication of its comment or
    // controller labels failed. That is not a completed review from the
    // controller's perspective: suppressing the next dispatch would leave
    // the provider state requesting review forever.
    if entry.failure_class.is_some() {
        return false;
    }
    match entry.review_verdict.as_deref() {
        Some("NEEDS_FIX" | "REJECT") => !entry.review_blocking_findings.is_empty(),
        Some("REVIEW_OUTPUT_INVALID") => false,
        Some(_) => true,
        None => false,
    }
}

pub type LedgerEntriesByWorkId = BTreeMap<String, Vec<LedgerEntry>>;

/// The current work-item-scoped human gate derived from append-only ledger
/// history. Consumers must use this shared projection instead of inferring the
/// state from whichever tickets happened to survive issue discovery: an open
/// PR/MR remains gated even when its source issue is dependency-blocked,
/// closed, or otherwise absent from the dispatchable ticket list.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveHumanGate {
    pub reason_code: Option<String>,
    pub dispatch_reason: Option<String>,
    pub message: Option<String>,
    pub mode: String,
    pub timestamp: String,
    pub routing_diagnostics: Option<super::entry::RoutingDiagnostics>,
}

const LEDGER_ENTRY_STALE_AFTER_DAYS: i64 = 14;

pub fn is_entry_stale(entry: &LedgerEntry) -> bool {
    let entry_time = if let Ok(parsed) = OffsetDateTime::parse(&entry.timestamp, &Rfc3339) {
        parsed
    } else if let Ok(secs) = entry.timestamp.parse::<i64>() {
        if let Ok(dt) = OffsetDateTime::from_unix_timestamp(secs) {
            dt
        } else {
            return false;
        }
    } else {
        return false;
    };
    OffsetDateTime::now_utc() - entry_time > time::Duration::days(LEDGER_ENTRY_STALE_AFTER_DAYS)
}

/// Resolve the effective human gate for one work item using the same
/// transition semantics as ticket discovery. A completed review may clear its
/// own provisional hold; review no-ops and unrelated completions may not clear
/// a hold. Paid-route grants stay in the history so status can verify that the
/// grant matches the exact blocked route before releasing it. `clear-attempts`
/// remains the unconditional release transition. Control-only records never
/// create a gate.
pub fn effective_human_gate_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
) -> Option<EffectiveHumanGate> {
    effective_human_gate_for_scope(entries, Some(profile_name), repo_id, work_id)
}

fn effective_human_gate_for_scope(
    entries: &[LedgerEntry],
    profile_name: Option<&str>,
    repo_id: &str,
    work_id: &str,
) -> Option<EffectiveHumanGate> {
    let aliases = work_id_aliases(work_id);
    let mut gate = None;
    for entry in entries.iter().filter(|entry| {
        profile_name.is_none_or(|profile_name| entry.profile == profile_name)
            && entry.repo_id == repo_id
            && entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
            && !is_entry_stale(entry)
    }) {
        match entry.mode.as_str() {
            "clear_attempts" => {
                gate = None;
                continue;
            }
            "paid_route_approval_grant" => {
                // Modern policy gates carry exact route diagnostics and must
                // be released only after status verifies this grant against
                // that route. Preserve legacy behavior for pre-reason-code
                // handoffs, whose requested identity cannot be reconstructed.
                if gate
                    .as_ref()
                    .and_then(|gate: &EffectiveHumanGate| gate.reason_code.as_deref())
                    != Some("policy_approval")
                {
                    gate = None;
                }
                continue;
            }
            "claim" | "paid_route_approval_revoke" | "review_hold" | "review_hold_release" => {
                continue;
            }
            _ if entry.validation_result.as_deref() == Some("deferred_capacity") => continue,
            "review" if !entry.human_required => {
                // Only a real terminal review supersedes a prior review gate.
                // Duplicate/no-op and failed publication records carry no
                // verdict and must not erase a pending policy approval.
                if entry.review_verdict.is_some() && entry.failure_class.is_none() {
                    gate = None;
                }
                continue;
            }
            _ if !entry.human_required => continue,
            _ => {}
        }

        gate = Some(EffectiveHumanGate {
            reason_code: entry.human_required_reason_code.clone(),
            dispatch_reason: entry.dispatch_reason.clone(),
            message: entry.error_summary.clone(),
            mode: entry.mode.clone(),
            timestamp: entry.timestamp.clone(),
            routing_diagnostics: entry.routing_diagnostics.clone(),
        });
    }
    gate
}

pub fn effective_human_gate_from_index(
    entries: &LedgerEntriesByWorkId,
    repo_id: &str,
    work_id: &str,
) -> Option<EffectiveHumanGate> {
    entries
        .get(work_id)
        .and_then(|entries| effective_human_gate_for_scope(entries, None, repo_id, work_id))
}

/// Native tracker issues use their provider-visible `#123` identity. Older
/// GAH records used `TICKET-123`; retain that as a read alias so migrating to
/// the tracker identity never forks history or re-dispatches completed work.
pub fn work_id_aliases(work_id: &str) -> Vec<String> {
    let legacy_number = work_id.strip_prefix("TICKET-").and_then(|rest| {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        (!digits.is_empty()).then_some(digits)
    });
    let issue_number = work_id
        .strip_prefix('#')
        .filter(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()));

    match (legacy_number.as_deref(), issue_number) {
        (Some(number), _) => vec![work_id.to_string(), format!("#{number}")],
        (_, Some(number)) => vec![work_id.to_string(), format!("TICKET-{number}")],
        _ => vec![work_id.to_string()],
    }
}

pub fn index_entries_by_work_id(entries: &[LedgerEntry]) -> LedgerEntriesByWorkId {
    let mut index = BTreeMap::new();
    for entry in entries {
        if let Some(work_id) = entry.work_id.as_ref() {
            for alias in work_id_aliases(work_id) {
                index
                    .entry(alias)
                    .or_insert_with(Vec::new)
                    .push(entry.clone());
            }
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::{
        active_paid_route_approvals, active_review_hold_work_ids, append, backfill_review_verdict,
        effective_human_gate_from_entries, entries_for_work_id, index_entries_by_work_id,
        read_entries, repair_truncated_tail_at, review_already_exists, review_is_dedup_eligible,
        ReviewVerdictBackfill,
    };
    use crate::ledger::test_util as ledger_tests;
    use std::fs;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    fn dedup_exists(
        cfg: &crate::config::GahConfig,
        profile_name: &str,
        repo_id: &str,
        work_id: &str,
        source_sha: &str,
        reviewer_class: &str,
    ) -> bool {
        let metadata_fingerprint = format!("metadata-for-{source_sha}");
        review_already_exists(
            cfg,
            profile_name,
            repo_id,
            work_id,
            source_sha,
            &metadata_fingerprint,
            reviewer_class,
        )
        .unwrap()
    }

    #[test]
    fn repair_truncated_tail_backs_up_only_an_unterminated_invalid_last_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        let valid_prefix = b"{\"record\":1}\n";
        let torn_tail = b"{\"record\":";
        let mut bytes = valid_prefix.to_vec();
        bytes.extend_from_slice(torn_tail);
        fs::write(&path, bytes).unwrap();

        let repaired = repair_truncated_tail_at(&path, false).unwrap();
        assert_eq!(repaired.dropped_bytes, torn_tail.len());
        let backup = repaired.backup_path.expect("torn tail must be backed up");
        assert_eq!(fs::read(&backup).unwrap(), torn_tail);
        assert_eq!(fs::read(&path).unwrap(), valid_prefix);
    }

    #[test]
    fn ledger_append_writes_jsonl() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let entry = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
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
    fn ledger_append_redacts_secret_like_strings_before_persisting() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let mut entry = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "claude",
            "pm",
            "hello",
            Some("123".into()),
            None,
        );
        entry.error_summary = Some("Authorization: Bearer abcdefghijklmnopqrstuvwxyz".into());
        let path = append(&cfg, &entry).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(text.contains("[REDACTED:TOKEN]"));
    }

    #[test]
    fn effective_human_gate_is_latched_until_an_explicit_release_transition() {
        let profile = ledger_tests::profile();
        let mut gate =
            super::super::LedgerEntry::new("test", &profile, "auto", "fix", "#639", None, None);
        gate.work_id = Some("#639".into());
        gate.human_required = true;
        gate.human_required_reason_code = Some("stuck_loop_gate".into());
        gate.dispatch_reason = Some("stuck_loop_gate".into());

        let mut unrelated_completion = gate.clone();
        unrelated_completion.mode = "fix".into();
        unrelated_completion.human_required = false;
        unrelated_completion.human_required_reason_code = None;
        unrelated_completion.dispatch_reason = Some("post_review_repair".into());

        let active = effective_human_gate_from_entries(
            &[gate.clone(), unrelated_completion],
            "test",
            &profile.repo_id,
            "TICKET-639",
        )
        .expect("a non-review completion must not clear the gate");
        assert_eq!(active.reason_code.as_deref(), Some("stuck_loop_gate"));

        let mut review_release = gate;
        review_release.mode = "review".into();
        review_release.human_required = false;
        review_release.human_required_reason_code = None;
        assert!(effective_human_gate_from_entries(
            &[review_release],
            "test",
            &profile.repo_id,
            "#639",
        )
        .is_none());
    }

    #[test]
    fn paid_route_grant_releases_only_the_matching_profile_and_repository_gate() {
        let profile = ledger_tests::profile();
        let mut gate =
            super::super::LedgerEntry::new("test", &profile, "auto", "review", "#650", None, None);
        gate.work_id = Some("#650".into());
        gate.human_required = true;
        gate.human_required_reason_code = Some("policy_approval".into());
        let grant = super::super::LedgerEntry::new_paid_route_approval(
            "test",
            &profile,
            "#650",
            "opencode",
            Some("nous/glm-5.2"),
            true,
        );

        let active = effective_human_gate_from_entries(
            &[gate.clone(), grant],
            "test",
            &profile.repo_id,
            "#650",
        )
        .expect("the raw policy gate stays latched until exact-route projection");
        assert_eq!(active.reason_code.as_deref(), Some("policy_approval"));
        assert!(effective_human_gate_from_entries(
            &[gate],
            "other-profile",
            &profile.repo_id,
            "#650",
        )
        .is_none());
    }

    #[test]
    fn review_noop_does_not_clear_a_paid_route_policy_gate() {
        let profile = ledger_tests::profile();
        let mut gate =
            super::super::LedgerEntry::new("test", &profile, "auto", "review", "#650", None, None);
        gate.work_id = Some("#650".into());
        gate.human_required = true;
        gate.human_required_reason_code = Some("policy_approval".into());

        let mut duplicate =
            super::super::LedgerEntry::new("test", &profile, "auto", "review", "#650", None, None);
        duplicate.work_id = Some("#650".into());
        duplicate.validation_result = Some("skipped_duplicate_review".into());

        let active =
            effective_human_gate_from_entries(&[gate, duplicate], "test", &profile.repo_id, "#650")
                .expect("an unchanged duplicate review must not release the approval hold");
        assert_eq!(active.reason_code.as_deref(), Some("policy_approval"));
    }

    #[test]
    fn review_dedup_requires_exact_work_sha_metadata_and_reviewer_class() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let profile = ledger_tests::profile();
        let mut entry =
            super::super::LedgerEntry::new("test", &profile, "claude", "review", "x", None, None);
        entry.work_id = Some("#109".into());
        entry.review_source_sha = Some("abc123".into());
        entry.review_metadata_fingerprint = Some("metadata-for-abc123".into());
        entry.reviewer_class = Some("strong".into());
        entry.review_verdict = Some("APPROVE".into());
        append(&cfg, &entry).unwrap();

        assert!(dedup_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#109",
            "abc123",
            "strong"
        ));
        assert!(!review_already_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#109",
            "abc123",
            "metadata-changed-without-new-commit",
            "strong",
        )
        .unwrap());
        assert!(!dedup_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#109",
            "def456",
            "strong"
        ));
        assert!(!dedup_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#109",
            "abc123",
            "weak"
        ));
        assert!(!dedup_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#110",
            "abc123",
            "strong"
        ));
        assert!(!dedup_exists(
            &cfg,
            "other",
            &profile.repo_id,
            "#109",
            "abc123",
            "strong"
        ));
        assert!(!dedup_exists(
            &cfg,
            "test",
            "other-repo",
            "#109",
            "abc123",
            "strong"
        ));
    }

    #[test]
    fn review_dedup_ignores_reviews_before_matching_clear_attempts() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let profile = ledger_tests::profile();
        let mut review =
            super::super::LedgerEntry::new("test", &profile, "claude", "review", "x", None, None);
        review.work_id = Some("#109".into());
        review.review_source_sha = Some("abc123".into());
        review.review_metadata_fingerprint = Some("metadata-for-abc123".into());
        review.reviewer_class = Some("strong".into());
        review.review_verdict = Some("APPROVE".into());
        append(&cfg, &review).unwrap();

        let mut unrelated_profile_clear = super::super::LedgerEntry::new(
            "other-profile",
            &profile,
            "auto",
            "clear_attempts",
            "unrelated reset",
            None,
            None,
        );
        unrelated_profile_clear.work_id = Some("#109".into());
        append(&cfg, &unrelated_profile_clear).unwrap();
        assert!(dedup_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#109",
            "abc123",
            "strong"
        ));

        let mut other_repo = profile.clone();
        other_repo.repo_id = "other-repo".into();
        let mut unrelated_repo_clear = super::super::LedgerEntry::new(
            "test",
            &other_repo,
            "auto",
            "clear_attempts",
            "unrelated reset",
            None,
            None,
        );
        unrelated_repo_clear.work_id = Some("#109".into());
        append(&cfg, &unrelated_repo_clear).unwrap();
        assert!(dedup_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#109",
            "abc123",
            "strong"
        ));

        let mut clear = super::super::LedgerEntry::new(
            "test",
            &profile,
            "auto",
            "clear_attempts",
            "operator reset",
            None,
            None,
        );
        clear.work_id = Some("TICKET-109".into());
        append(&cfg, &clear).unwrap();

        assert!(!dedup_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#109",
            "abc123",
            "strong"
        ));

        append(&cfg, &review).unwrap();
        assert!(dedup_exists(
            &cfg,
            "test",
            &profile.repo_id,
            "#109",
            "abc123",
            "strong"
        ));
    }

    #[test]
    fn review_dedup_retries_legacy_repairs_but_suppresses_structured_ones() {
        let profile = ledger_tests::profile();
        let mut entry =
            super::super::LedgerEntry::new("test", &profile, "claude", "review", "x", None, None);
        entry.review_verdict = Some("NEEDS_FIX".into());
        assert!(!review_is_dedup_eligible(&entry));

        entry.review_blocking_findings = vec!["src/lib.rs: broken retry".into()];
        assert!(review_is_dedup_eligible(&entry));

        entry.review_verdict = Some("APPROVE".into());
        entry.review_blocking_findings.clear();
        assert!(review_is_dedup_eligible(&entry));

        entry.failure_class = Some("harness_error".into());
        assert!(!review_is_dedup_eligible(&entry));
    }

    #[test]
    fn entries_for_work_id_reads_legacy_ticket_alias_for_native_issue() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let mut matching = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "claude",
            "pm",
            "x",
            None,
            None,
        );
        matching.work_id = Some("TICKET-096".into());
        append(&cfg, &matching).unwrap();

        let mut other = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "claude",
            "pm",
            "y",
            None,
            None,
        );
        other.work_id = Some("TICKET-097".into());
        append(&cfg, &other).unwrap();

        let found = entries_for_work_id(&cfg, "#096").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].work_id.as_deref(), Some("TICKET-096"));
    }

    #[test]
    fn index_entries_by_work_id_groups_only_tagged_entries() {
        let mut first = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "claude",
            "pm",
            "x",
            None,
            None,
        );
        first.work_id = Some("TICKET-096".into());
        let mut second = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "claude",
            "pm",
            "y",
            None,
            None,
        );
        second.work_id = Some("TICKET-096".into());
        let untagged = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "claude",
            "pm",
            "z",
            None,
            None,
        );

        let index = index_entries_by_work_id(&[first, second, untagged]);
        assert_eq!(index.len(), 2);
        assert_eq!(index["TICKET-096"].len(), 2);
        assert_eq!(index["#096"].len(), 2);
    }

    #[test]
    fn backfill_review_verdict_attributes_to_implementation_entry_not_reviewer() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let mut impl_entry = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "vibe",
            "improve",
            "test1",
            None,
            None,
        );
        impl_entry.effective_backend = "vibe".to_string();
        impl_entry.branch = Some("gah/gah-123".to_string());
        append(&cfg, &impl_entry).unwrap();

        let mut review_entry = super::super::LedgerEntry::new(
            "test",
            &ledger_tests::profile(),
            "claude",
            "review",
            "test1",
            None,
            None,
        );
        review_entry.effective_backend = "claude".to_string();
        review_entry.branch = Some("gah/gah-123".to_string());
        append(&cfg, &review_entry).unwrap();

        let found = backfill_review_verdict(
            &cfg,
            "gah/gah-123",
            ReviewVerdictBackfill {
                verdict: "NEEDS_FIX",
                confidence: "high",
                reviewer_backend: "claude",
                reviewer_model: Some("claude-sonnet-4"),
                reviewer_tier: None,
                review_gate_reason: Some("test review evidence gate"),
                review_source_sha: Some("abc123"),
                review_metadata_fingerprint: Some("sha256:test"),
                blocking_findings: &["src/lib.rs: broken retry".to_string()],
                actionable_findings: &[],
                non_blocking_findings: &["consider a smaller helper".to_string()],
                risk_notes: &["retry state can be lost".to_string()],
                evidence: &[
                    "file:src/lib.rs".to_string(),
                    "ghp_abcdefghijklmnopqrstuvwxyz".to_string(),
                ],
                compatibility_evidence: &[],
            },
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
        assert_eq!(
            updated_impl.review_gate_reason.as_deref(),
            Some("test review evidence gate")
        );
        assert_eq!(updated_impl.review_source_sha.as_deref(), Some("abc123"));
        assert_eq!(
            updated_impl.review_metadata_fingerprint.as_deref(),
            Some("sha256:test")
        );
        assert_eq!(
            updated_impl.review_blocking_findings,
            ["src/lib.rs: broken retry"]
        );
        assert_eq!(updated_impl.review_evidence[1], "[REDACTED:GITHUB_TOKEN]");

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
        let (_tmp, cfg) = ledger_tests::test_config();
        let found = backfill_review_verdict(
            &cfg,
            "gah/no-such-branch",
            ReviewVerdictBackfill {
                verdict: "APPROVE",
                confidence: "high",
                reviewer_backend: "codex",
                reviewer_model: None,
                reviewer_tier: None,
                review_gate_reason: None,
                review_source_sha: None,
                review_metadata_fingerprint: None,
                blocking_findings: &[],
                actionable_findings: &[],
                non_blocking_findings: &[],
                risk_notes: &[],
                evidence: &[],
                compatibility_evidence: &[],
            },
        )
        .unwrap();
        assert!(!found);
    }

    #[test]
    fn active_review_hold_work_ids_hold_with_no_release_is_active() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let prof = ledger_tests::profile();
        let hold = super::super::LedgerEntry::new_review_hold("test", &prof, "TICKET-600", None);
        append(&cfg, &hold).unwrap();

        let held = active_review_hold_work_ids(&cfg, "test");
        assert!(held.contains("TICKET-600"));
    }

    #[test]
    fn active_review_hold_work_ids_hold_then_release_is_not_active() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let prof = ledger_tests::profile();
        let hold = super::super::LedgerEntry::new_review_hold("test", &prof, "TICKET-600", None);
        append(&cfg, &hold).unwrap();
        let release =
            super::super::LedgerEntry::new_review_hold_release("test", &prof, "TICKET-600");
        append(&cfg, &release).unwrap();

        let held = active_review_hold_work_ids(&cfg, "test");
        assert!(!held.contains("TICKET-600"));
    }

    #[test]
    fn active_review_hold_work_ids_stale_hold_is_not_active_even_without_release() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let prof = ledger_tests::profile();
        let mut hold =
            super::super::LedgerEntry::new_review_hold("test", &prof, "TICKET-600", None);
        hold.timestamp = (OffsetDateTime::now_utc() - time::Duration::hours(3))
            .format(&Rfc3339)
            .unwrap();
        append(&cfg, &hold).unwrap();

        let held = active_review_hold_work_ids(&cfg, "test");
        assert!(!held.contains("TICKET-600"));
    }

    #[test]
    fn active_review_hold_work_ids_rehold_after_release_is_active_again() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let prof = ledger_tests::profile();
        append(
            &cfg,
            &super::super::LedgerEntry::new_review_hold("test", &prof, "TICKET-600", None),
        )
        .unwrap();
        append(
            &cfg,
            &super::super::LedgerEntry::new_review_hold_release("test", &prof, "TICKET-600"),
        )
        .unwrap();
        append(
            &cfg,
            &super::super::LedgerEntry::new_review_hold("test", &prof, "TICKET-600", None),
        )
        .unwrap();

        let held = active_review_hold_work_ids(&cfg, "test");
        assert!(held.contains("TICKET-600"));
    }

    #[test]
    fn paid_route_approval_is_exact_and_revocable() {
        let (_tmp, cfg) = ledger_tests::test_config();
        let prof = ledger_tests::profile();
        append(
            &cfg,
            &super::super::LedgerEntry::new_paid_route_approval(
                "test",
                &prof,
                "ISSUE-42",
                "opencode",
                Some("openai/gpt-paid"),
                true,
            ),
        )
        .unwrap();

        let active = active_paid_route_approvals(&cfg, "test", "ISSUE-42").unwrap();
        assert!(active.contains(&("opencode".to_string(), Some("openai/gpt-paid".to_string()))));
        assert!(!active.contains(&("opencode".to_string(), Some("different-model".to_string()))));

        append(
            &cfg,
            &super::super::LedgerEntry::new_paid_route_approval(
                "test",
                &prof,
                "ISSUE-42",
                "opencode",
                Some("openai/gpt-paid"),
                false,
            ),
        )
        .unwrap();
        assert!(active_paid_route_approvals(&cfg, "test", "ISSUE-42")
            .unwrap()
            .is_empty());
    }
}
