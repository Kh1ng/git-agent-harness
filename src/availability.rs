//! TICKET-065: durable backend/model availability state.
//!
//! GLOBAL, not per-profile: Claude/Codex subscription limits are shared
//! across every repo GAH touches, so the state file lives under
//! `$XDG_STATE_HOME/gah/availability.json` (falling back to
//! `~/.local/state/gah/availability.json`), not under any profile's
//! `artifact_root`.
//!
//! The file is an append-only list of records (mirroring the ledger's
//! append-only philosophy), not a keyed map that gets overwritten in place.
//! "Current state" for a given (backend, model) scope is derived by reading
//! the *last* record in the file matching that scope. This is what makes
//! concurrent updates from separate GAH processes safe to reason about:
//! two processes appending different records at the same time can never
//! erase each other's write, because neither ever rewrites an existing
//! record — they only ever add to the list, under an exclusive lock.
//!
//! Explicitly out of scope for this ticket (see TICKET-066/067/068/069):
//! parsing quota errors, wiring into routing, updating from runner
//! failures, and the `gah availability` CLI view. This module only
//! provides the durable store and the eligibility query.

// Not yet wired into routing/runner/CLI (TICKET-067/068/069) — the module
// is complete and tested but has no external callers yet, hence the
// blanket allow instead of one per item.
#![allow(dead_code)]

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    RateLimited,
    QuotaExhausted,
    AuthenticationError,
    BackendOutage,
    /// Explicit model context-window/context-length exhaustion. Distinct from
    /// `QuotaExhausted`: it is a property of the specific task+model
    /// combination, not of the backend account or quota pool, so it must
    /// never be recorded as a permanent, account- or pool-wide unavailability.
    ContextLimit,
    ManualDisable,
    Unknown,
}

impl Reason {
    /// For human/JSON display in the `cli` module. Kept in sync with the
    /// `#[serde(rename_all = "snake_case")]` wire format by a unit test.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::QuotaExhausted => "quota_exhausted",
            Self::AuthenticationError => "authentication_error",
            Self::BackendOutage => "backend_outage",
            Self::ContextLimit => "context_limit",
            Self::ManualDisable => "manual_disable",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    BackendError,
    Manual,
    Imported,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BackendError => "backend_error",
            Self::Manual => "manual",
            Self::Imported => "imported",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityRecord {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    pub status: Status,
    pub reason: Reason,
    /// RFC3339 timestamp.
    pub observed_at: String,
    /// RFC3339 timestamp. Absent means "does not expire" — the only way a
    /// record like that stops blocking eligibility is a later record in the
    /// same scope with `status: available`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_until: Option<String>,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityState {
    pub version: u32,
    #[serde(default)]
    pub records: Vec<AvailabilityRecord>,
}

impl Default for AvailabilityState {
    fn default() -> Self {
        AvailabilityState {
            version: CURRENT_VERSION,
            records: Vec::new(),
        }
    }
}

/// Which scope a blocking record matched, for callers that want to explain
/// *why* something was routed around (e.g. TICKET-067's routing log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockScope {
    BackendWide,
    ModelSpecific,
    QuotaPool,
}

#[derive(Debug, Clone)]
pub struct AvailabilityDecision {
    pub eligible: bool,
    pub reason: Option<Reason>,
    pub unavailable_until: Option<String>,
    pub scope: Option<BlockScope>,
}

impl AvailabilityDecision {
    fn eligible() -> Self {
        AvailabilityDecision {
            eligible: true,
            reason: None,
            unavailable_until: None,
            scope: None,
        }
    }
}

/// Resolve the state file path from explicit env values. Pure function (no
/// direct env reads) so path-resolution tests never touch process-global
/// environment — see the PATH-mutation lesson from provider.rs's test seam.
fn resolve_state_path_from_env(xdg_state_home: Option<&str>, home: Option<&str>) -> PathBuf {
    if let Some(xdg) = xdg_state_home.filter(|s| !s.is_empty()) {
        return PathBuf::from(xdg).join("gah").join("availability.json");
    }
    let home = home.unwrap_or("/root");
    PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("gah")
        .join("availability.json")
}

/// Resolve the state file path from the real environment. `GAH_AVAILABILITY_PATH`
/// is an explicit override, matching the existing `GAH_LEDGER_PATH` convention.
pub fn resolve_state_path() -> PathBuf {
    if let Ok(path) = std::env::var("GAH_AVAILABILITY_PATH") {
        return PathBuf::from(path);
    }
    resolve_state_path_from_env(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

fn lock_path_for(state_path: &Path) -> PathBuf {
    let mut lock_name = state_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "availability.json".into());
    lock_name.push(".lock");
    state_path.with_file_name(lock_name)
}

/// Read the state file, if present. A missing file is `Ok(default)` (no
/// state recorded yet = everything eligible). A present-but-malformed file
/// or an unsupported version is an actionable `Err`, never silently treated
/// as empty — the caller must not have durable history quietly discarded.
pub fn load_state(state_path: &Path) -> Result<AvailabilityState> {
    let text = match fs::read_to_string(state_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AvailabilityState::default())
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", state_path.display())),
    };
    let mut state: AvailabilityState = serde_json::from_str(&text)
        .with_context(|| format!("parsing availability state {}", state_path.display()))?;
    if state.version != CURRENT_VERSION {
        anyhow::bail!(
            "availability state {} has unsupported schema version {} (expected {}); refusing to read or overwrite it",
            state_path.display(),
            state.version,
            CURRENT_VERSION,
        );
    }
    // Canonicalize backend aliases (e.g. "cloud-coder" -> "openhands") at
    // the single load point so every consumer -- list_scopes,
    // availability_for, latest_for_scope/pool, and update_state's
    // read-modify-write cycle -- sees consistent scope keys. This also
    // self-heals historical records written under the old alias the next
    // time the file is rewritten, with no separate migration needed.
    for record in &mut state.records {
        if record.backend != crate::config::canonical_backend_name(&record.backend) {
            record.backend = crate::config::canonical_backend_name(&record.backend).to_string();
        }
    }
    Ok(state)
}

/// Read-modify-write under an exclusive advisory lock, with an atomic
/// write-temp-then-rename so readers never observe a partial file and a
/// crash mid-write can never corrupt the previous good state.
///
/// `mutate` is only called after the current state has been loaded
/// successfully; if loading fails (malformed file, bad version), this
/// returns that error without calling `mutate` or touching the file.
pub fn update_state<F>(state_path: &Path, mutate: F) -> Result<()>
where
    F: FnOnce(&mut AvailabilityState),
{
    let dir = state_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("creating directory {}", dir.display()))?;

    let lock_path = lock_path_for(state_path);
    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false) // content is unused; explicit so intent isn't ambiguous
        .open(&lock_path)
        .with_context(|| format!("opening lock file {}", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("locking {}", lock_path.display()))?;

    let mut state = load_state(state_path)?;
    mutate(&mut state);

    let mut value = serde_json::to_value(&state).context("serializing availability state")?;
    crate::redact::redact_json_value(&mut value);
    let json = serde_json::to_string_pretty(&value).context("serializing availability state")?;
    let tmp_path = dir.join(format!(
        "{}.tmp.{}",
        state_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("availability.json"),
        std::process::id()
    ));
    {
        let mut tmp = File::create(&tmp_path)
            .with_context(|| format!("creating temp file {}", tmp_path.display()))?;
        tmp.write_all(json.as_bytes())
            .with_context(|| format!("writing temp file {}", tmp_path.display()))?;
        tmp.sync_all().ok();
    }
    fs::rename(&tmp_path, state_path).with_context(|| {
        format!(
            "renaming {} to {}",
            tmp_path.display(),
            state_path.display()
        )
    })?;

    FileExt::unlock(&lock_file).ok();
    Ok(())
}

fn now_rfc3339(now: OffsetDateTime) -> String {
    now.format(&Rfc3339)
        .unwrap_or_else(|_| now.unix_timestamp().to_string())
}

/// Append a record marking (backend, model) unavailable.
///
/// 8 args trips clippy::too_many_arguments; not bundling them into a params
/// struct yet since this has no real callers until TICKET-066/067/068 wire
/// it up, and their actual call-site shape should decide that, not a guess
/// made now.
#[allow(clippy::too_many_arguments)]
pub fn record_unavailable(
    state_path: &Path,
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
    reason: Reason,
    source: Source,
    unavailable_until: Option<OffsetDateTime>,
    last_error_summary: Option<String>,
    now: OffsetDateTime,
) -> Result<()> {
    let record = AvailabilityRecord {
        backend: backend.to_string(),
        model: model.map(str::to_string),
        quota_pool: quota_pool.map(str::to_string),
        status: Status::Unavailable,
        reason,
        observed_at: now_rfc3339(now),
        unavailable_until: unavailable_until.map(now_rfc3339),
        source,
        last_error_summary,
    };
    update_state(state_path, |state| state.records.push(record))
}

/// Append a record explicitly marking (backend, model) available again.
/// This is the only way to clear a non-expiring record (e.g. manual_disable).
pub fn record_available(
    state_path: &Path,
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
    source: Source,
    now: OffsetDateTime,
) -> Result<()> {
    let record = AvailabilityRecord {
        backend: backend.to_string(),
        model: model.map(str::to_string),
        quota_pool: quota_pool.map(str::to_string),
        status: Status::Available,
        reason: Reason::Unknown,
        observed_at: now_rfc3339(now),
        unavailable_until: None,
        source,
        last_error_summary: None,
    };
    update_state(state_path, |state| state.records.push(record))
}

fn is_active(record: &AvailabilityRecord, now: OffsetDateTime) -> bool {
    if record.status != Status::Unavailable {
        return false;
    }
    match &record.unavailable_until {
        None => true, // no expiry: manual_disable-style records never auto-expire
        Some(until) => match OffsetDateTime::parse(until, &Rfc3339) {
            Ok(until) => until > now,
            // an unparsable expiry is treated conservatively as "still active"
            // rather than silently ignored.
            Err(_) => true,
        },
    }
}

/// The last record in file order matching this exact scope (backend, and
/// either "no model" for backend-wide scope or this specific model).
fn latest_for_scope<'a>(
    records: &'a [AvailabilityRecord],
    backend: &str,
    model: Option<&str>,
) -> Option<&'a AvailabilityRecord> {
    records
        .iter()
        .rev()
        .find(|r| r.backend == backend && r.model.as_deref() == model)
}

fn latest_for_pool<'a>(
    records: &'a [AvailabilityRecord],
    pool: &str,
) -> Option<&'a AvailabilityRecord> {
    records
        .iter()
        .rev()
        .find(|r| r.quota_pool.as_deref() == Some(pool))
}

/// Eligibility for `backend`/`model`/`quota_pool` at `now`. Precedence:
/// 1. an active pool-wide record (quota_pool matches) blocks any candidate
///    assigned to that pool;
/// 2. otherwise an active backend-wide record (model = None) blocks everything on
///    that backend, including manual_disable, which is just a backend-wide
///    record with no expiry;
/// 3. otherwise an active model-specific record blocks that model only;
/// 4. otherwise eligible.
pub fn availability_for(
    state_path: &Path,
    backend: &str,
    model: Option<&str>,
    quota_pool: Option<&str>,
    now: OffsetDateTime,
) -> Result<AvailabilityDecision> {
    let state = load_state(state_path)?;

    // Precedence is a single timeline per scope: the *last* record in file
    // order matching a scope decides. A newer `available` record (e.g. an
    // operator `gah availability clear`, which appends rather than rewriting)
    // therefore overrides any older `unavailable` in the same scope -- that's
    // why `clear` works without deleting history. An *active* `unavailable`
    // record still blocks, with the usual pool > backend-wide > model-specific
    // precedence when two active blocks coexist.
    if let Some(pool) = quota_pool {
        if let Some(record) = latest_for_pool(&state.records, pool) {
            if is_active(record, now) {
                return Ok(AvailabilityDecision {
                    eligible: false,
                    reason: Some(record.reason),
                    unavailable_until: record.unavailable_until.clone(),
                    scope: Some(BlockScope::QuotaPool),
                });
            }
            if record.status == Status::Available {
                return Ok(AvailabilityDecision::eligible());
            }
        }
    }

    if let Some(record) = latest_for_scope(&state.records, backend, None) {
        if is_active(record, now) {
            return Ok(AvailabilityDecision {
                eligible: false,
                reason: Some(record.reason),
                unavailable_until: record.unavailable_until.clone(),
                scope: Some(BlockScope::BackendWide),
            });
        }
        if record.status == Status::Available {
            return Ok(AvailabilityDecision::eligible());
        }
    }

    if let Some(model) = model {
        if let Some(record) = latest_for_scope(&state.records, backend, Some(model)) {
            if is_active(record, now) {
                return Ok(AvailabilityDecision {
                    eligible: false,
                    reason: Some(record.reason),
                    unavailable_until: record.unavailable_until.clone(),
                    scope: Some(BlockScope::ModelSpecific),
                });
            }
            if record.status == Status::Available {
                return Ok(AvailabilityDecision::eligible());
            }
        }
    }

    Ok(AvailabilityDecision::eligible())
}

/// TICKET-069: one row of `gah availability` output. Combines the
/// eligibility decision (which already correctly applies backend-wide
/// precedence over model-specific state) with the informational fields
/// (source, last error, when it was observed) pulled from whichever record
/// actually produced that decision.
#[derive(Debug, Clone)]
pub struct ScopeStatus {
    pub backend: String,
    pub model: Option<String>,
    pub quota_pool: Option<String>,
    pub eligible: bool,
    pub reason: Option<Reason>,
    pub unavailable_until: Option<String>,
    pub scope: Option<BlockScope>,
    pub source: Option<Source>,
    pub last_error_summary: Option<String>,
    pub observed_at: Option<String>,
}

/// One row per distinct (backend, model, quota_pool) scope that has ever appeared in
/// the state file, sorted by backend then model (backend-wide rows, i.e.
/// model = None, sort first for a given backend).
pub fn list_scopes(state_path: &Path, now: OffsetDateTime) -> Result<Vec<ScopeStatus>> {
    let state = load_state(state_path)?;

    let mut seen: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    for record in &state.records {
        let key = (
            record.backend.clone(),
            record.model.clone(),
            record.quota_pool.clone(),
        );
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen.sort();

    let mut out = Vec::with_capacity(seen.len());
    for (backend, model, quota_pool) in seen {
        let decision = availability_for(
            state_path,
            &backend,
            model.as_deref(),
            quota_pool.as_deref(),
            now,
        )?;
        let informative = match decision.scope {
            Some(BlockScope::BackendWide) => latest_for_scope(&state.records, &backend, None),
            Some(BlockScope::QuotaPool) => quota_pool
                .as_deref()
                .and_then(|p| latest_for_pool(&state.records, p)),
            _ => latest_for_scope(&state.records, &backend, model.as_deref()),
        };
        out.push(ScopeStatus {
            backend,
            model,
            quota_pool,
            eligible: decision.eligible,
            reason: decision.reason,
            unavailable_until: decision.unavailable_until,
            scope: decision.scope,
            source: informative.map(|r| r.source),
            last_error_summary: informative.and_then(|r| r.last_error_summary.clone()),
            observed_at: informative.map(|r| r.observed_at.clone()),
        });
    }
    Ok(out)
}

/// Format a remaining duration until an RFC3339 timestamp as e.g. "2h 14m",
/// "14m", or "less than a minute". Returns `None` if `until` can't be
/// parsed or has already passed (callers only show this for active blocks).
fn format_remaining(until: &str, now: OffsetDateTime) -> Option<String> {
    let until = OffsetDateTime::parse(until, &Rfc3339).ok()?;
    let remaining = until - now;
    if remaining.is_negative() {
        return None;
    }
    let total_minutes = remaining.whole_minutes();
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours > 0 {
        Some(format!("{}h {}m", hours, minutes))
    } else if minutes > 0 {
        Some(format!("{}m", minutes))
    } else {
        Some("less than a minute".to_string())
    }
}

/// TICKET-069: `gah availability` (human) and `gah availability --json`.
pub mod cli {
    use super::{
        format_remaining, list_scopes, now_rfc3339, resolve_state_path, AvailabilityRecord, Reason,
        ScopeStatus, Source, Status,
    };
    use anyhow::Result;
    use serde::Serialize;
    use time::OffsetDateTime;

    #[derive(Debug, Serialize)]
    struct Row {
        backend: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        quota_pool: Option<String>,
        eligible: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        unavailable_until: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        remaining_cooldown: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_error_summary: Option<String>,
    }

    impl Row {
        fn from_status(s: &ScopeStatus, now: OffsetDateTime) -> Self {
            Row {
                backend: s.backend.clone(),
                model: s.model.clone(),
                quota_pool: s.quota_pool.clone(),
                eligible: s.eligible,
                reason: s.reason.map(super::Reason::as_str),
                remaining_cooldown: s
                    .unavailable_until
                    .as_deref()
                    .and_then(|u| format_remaining(u, now)),
                unavailable_until: s.unavailable_until.clone(),
                source: s.source.map(super::Source::as_str),
                last_error_summary: s.last_error_summary.clone(),
            }
        }
    }

    pub fn run(json: bool) -> Result<()> {
        let state_path = resolve_state_path();
        let now = OffsetDateTime::now_utc();
        let statuses = list_scopes(&state_path, now)?;
        let rows: Vec<Row> = statuses.iter().map(|s| Row::from_status(s, now)).collect();

        if json {
            println!("{}", serde_json::to_string(&rows)?);
            return Ok(());
        }

        if rows.is_empty() {
            println!("No availability state recorded — everything is eligible by default.");
            return Ok(());
        }
        for row in &rows {
            let mut name = match &row.model {
                Some(model) => format!("{}/{}", row.backend, model),
                None => row.backend.clone(),
            };
            if let Some(pool) = &row.quota_pool {
                name = format!("{} (pool: {})", name, pool);
            }
            if row.eligible {
                println!("{:<28} available", name);
            } else {
                let reason = row.reason.unwrap_or("unknown");
                let cooldown = row
                    .remaining_cooldown
                    .as_deref()
                    .map(|c| format!("resets in {c}"))
                    .unwrap_or_else(|| "no expiry (manual or unresolved)".to_string());
                println!("{:<28} unavailable   {:<18} {}", name, reason, cooldown);
            }
        }
        Ok(())
    }

    /// Issue #179: operator override for "the tracked availability is wrong,
    /// an operator knows better" (confirmed live -- e.g. a codex
    /// `quota_exhausted` record surviving hours after the operator confirmed
    /// the account is actually healthy again). This appends a
    /// `status: available, source: manual` record for the given scope via the
    /// same lock-protected `update_state` read-modify-write every other
    /// availability mutation uses, so it's safe against concurrent parallel
    /// workers -- unlike hand-editing `availability.json` directly, which is
    /// read-modify-write racy.
    ///
    /// It appends (never rewrites history): eligibility is always derived from
    /// the *last* record in file order matching a scope, so a fresh
    /// `available` record immediately unblocks that scope on the next
    /// `availability_for` read. `model: None` marks the backend-wide scope
    /// (overrides any backend-wide block); `Some(m)` marks only that exact
    /// model; `quota_pool: Some(p)` marks the pool-wide scope. These are
    /// independent scopes in `availability_for`'s precedence, so a scoped
    /// clear never affects other scopes.
    pub fn clear(
        state_path: &std::path::Path,
        backend: &str,
        model: Option<&str>,
        quota_pool: Option<&str>,
    ) -> Result<()> {
        let backend = crate::config::canonical_backend_name(backend).to_string();
        let now = OffsetDateTime::now_utc();
        let record = AvailabilityRecord {
            backend: backend.clone(),
            model: model.map(str::to_string),
            quota_pool: quota_pool.map(str::to_string),
            status: Status::Available,
            reason: Reason::Unknown,
            observed_at: now_rfc3339(now),
            unavailable_until: None,
            source: Source::Manual,
            last_error_summary: None,
        };
        super::update_state(state_path, |state| {
            state.records.push(record);
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::TempDir;

    fn path(tmp: &TempDir) -> PathBuf {
        tmp.path().join("availability.json")
    }

    #[allow(clippy::too_many_arguments)]
    fn record_unavailable(
        state_path: &Path,
        backend: &str,
        model: Option<&str>,
        reason: Reason,
        source: Source,
        unavailable_until: Option<OffsetDateTime>,
        last_error_summary: Option<String>,
        now: OffsetDateTime,
    ) -> Result<()> {
        super::record_unavailable(
            state_path,
            backend,
            model,
            None,
            reason,
            source,
            unavailable_until,
            last_error_summary,
            now,
        )
    }

    fn record_available(
        state_path: &Path,
        backend: &str,
        model: Option<&str>,
        source: Source,
        now: OffsetDateTime,
    ) -> Result<()> {
        super::record_available(state_path, backend, model, None, source, now)
    }

    fn availability_for(
        state_path: &Path,
        backend: &str,
        model: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<AvailabilityDecision> {
        super::availability_for(state_path, backend, model, None, now)
    }

    #[test]
    fn missing_state_file_means_eligible() {
        let tmp = TempDir::new().unwrap();
        let decision =
            availability_for(&path(&tmp), "claude", None, OffsetDateTime::now_utc()).unwrap();
        assert!(decision.eligible);
    }

    #[test]
    fn backend_wide_block_blocks_all_models() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &p,
            "claude",
            None,
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::hours(1)),
            Some("quota exhausted".into()),
            now,
        )
        .unwrap();

        let d1 = availability_for(&p, "claude", None, now).unwrap();
        assert!(!d1.eligible);
        assert_eq!(d1.scope, Some(BlockScope::BackendWide));

        let d2 = availability_for(&p, "claude", Some("claude-sonnet-4"), now).unwrap();
        assert!(!d2.eligible);
        assert_eq!(d2.scope, Some(BlockScope::BackendWide));
    }

    #[test]
    fn model_specific_block_blocks_only_that_model() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &p,
            "openhands",
            Some("litellm_proxy/deepseek-v4"),
            Reason::RateLimited,
            Source::BackendError,
            Some(now + time::Duration::minutes(30)),
            None,
            now,
        )
        .unwrap();

        let blocked =
            availability_for(&p, "openhands", Some("litellm_proxy/deepseek-v4"), now).unwrap();
        assert!(!blocked.eligible);
        assert_eq!(blocked.scope, Some(BlockScope::ModelSpecific));

        let other_model =
            availability_for(&p, "openhands", Some("litellm_proxy/other"), now).unwrap();
        assert!(other_model.eligible);

        let backend_wide_query = availability_for(&p, "openhands", None, now).unwrap();
        assert!(
            backend_wide_query.eligible,
            "a model-specific block must not block the backend-wide (no model) query"
        );
    }

    #[test]
    fn expired_temporary_record_becomes_eligible() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let observed_at = OffsetDateTime::now_utc() - time::Duration::hours(2);
        record_unavailable(
            &p,
            "codex",
            None,
            Reason::RateLimited,
            Source::BackendError,
            Some(observed_at + time::Duration::hours(1)), // expired an hour ago
            None,
            observed_at,
        )
        .unwrap();

        let decision = availability_for(&p, "codex", None, OffsetDateTime::now_utc()).unwrap();
        assert!(decision.eligible);
    }

    #[test]
    fn manual_disable_without_expiry_remains_blocked() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let observed_at = OffsetDateTime::now_utc() - time::Duration::days(30);
        record_unavailable(
            &p,
            "claude",
            None,
            Reason::ManualDisable,
            Source::Manual,
            None,
            Some("disabled by operator".into()),
            observed_at,
        )
        .unwrap();

        // Even long after it was recorded, with no expiry it is still blocked.
        let decision = availability_for(&p, "claude", None, OffsetDateTime::now_utc()).unwrap();
        assert!(!decision.eligible);
        assert_eq!(decision.reason, Some(Reason::ManualDisable));
    }

    #[test]
    fn explicit_available_record_clears_manual_disable() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let t0 = OffsetDateTime::now_utc() - time::Duration::hours(1);
        record_unavailable(
            &p,
            "claude",
            None,
            Reason::ManualDisable,
            Source::Manual,
            None,
            None,
            t0,
        )
        .unwrap();
        record_available(
            &p,
            "claude",
            None,
            Source::Manual,
            OffsetDateTime::now_utc(),
        )
        .unwrap();

        let decision = availability_for(&p, "claude", None, OffsetDateTime::now_utc()).unwrap();
        assert!(decision.eligible);
    }

    #[test]
    fn backend_wide_block_takes_precedence_over_model_availability() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();
        // Model itself was explicitly marked available...
        record_available(&p, "claude", Some("claude-sonnet-4"), Source::Manual, now).unwrap();
        // ...but the whole backend is down.
        record_unavailable(
            &p,
            "claude",
            None,
            Reason::BackendOutage,
            Source::BackendError,
            Some(now + time::Duration::hours(1)),
            None,
            now,
        )
        .unwrap();

        let decision = availability_for(&p, "claude", Some("claude-sonnet-4"), now).unwrap();
        assert!(!decision.eligible);
        assert_eq!(decision.scope, Some(BlockScope::BackendWide));
    }

    #[test]
    fn two_sequential_updates_preserve_both_records() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &p,
            "claude",
            None,
            Reason::QuotaExhausted,
            Source::BackendError,
            None,
            None,
            now,
        )
        .unwrap();
        record_unavailable(
            &p,
            "codex",
            None,
            Reason::RateLimited,
            Source::BackendError,
            None,
            None,
            now,
        )
        .unwrap();

        let state = load_state(&p).unwrap();
        assert_eq!(state.records.len(), 2);
        assert!(!availability_for(&p, "claude", None, now).unwrap().eligible);
        assert!(!availability_for(&p, "codex", None, now).unwrap().eligible);
    }

    #[test]
    fn concurrent_updates_do_not_lose_records() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let backends = ["claude", "codex", "openhands", "b4", "b5", "b6", "b7", "b8"];
        let barrier = Arc::new(Barrier::new(backends.len()));
        let handles: Vec<_> = backends
            .into_iter()
            .map(|backend| {
                let p = p.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    record_unavailable(
                        &p,
                        backend,
                        None,
                        Reason::Unknown,
                        Source::BackendError,
                        None,
                        None,
                        OffsetDateTime::now_utc(),
                    )
                    .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let state = load_state(&p).unwrap();
        assert_eq!(
            state.records.len(),
            8,
            "concurrent appends must not clobber each other"
        );
    }

    #[test]
    fn malformed_state_file_returns_actionable_error_and_is_not_overwritten() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        fs::write(&p, "not json at all").unwrap();

        let err = load_state(&p).unwrap_err();
        assert!(format!("{:#}", err).contains("parsing availability state"));

        let update_err = update_state(&p, |_| {}).unwrap_err();
        assert!(format!("{:#}", update_err).contains("parsing availability state"));

        // The bad file must still be there, untouched.
        assert_eq!(fs::read_to_string(&p).unwrap(), "not json at all");
    }

    #[test]
    fn unsupported_schema_version_fails_clearly() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        fs::write(&p, r#"{"version":99,"records":[]}"#).unwrap();

        let err = load_state(&p).unwrap_err();
        assert!(format!("{:#}", err).contains("unsupported schema version"));
    }

    #[test]
    fn atomic_write_leaves_valid_json() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        record_unavailable(
            &p,
            "claude",
            None,
            Reason::Unknown,
            Source::Manual,
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();

        let text = fs::read_to_string(&p).unwrap();
        let _: AvailabilityState = serde_json::from_str(&text).unwrap();
        // No leftover temp files.
        let leftover: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftover.is_empty());
    }

    #[test]
    fn xdg_state_home_path_resolution() {
        let resolved = resolve_state_path_from_env(Some("/custom/xdg-state"), Some("/home/user"));
        assert_eq!(
            resolved,
            PathBuf::from("/custom/xdg-state/gah/availability.json")
        );
    }

    #[test]
    fn fallback_local_state_path_resolution() {
        let resolved = resolve_state_path_from_env(None, Some("/home/user"));
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/.local/state/gah/availability.json")
        );
    }

    #[test]
    fn empty_xdg_state_home_falls_back_like_unset() {
        let resolved = resolve_state_path_from_env(Some(""), Some("/home/user"));
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/.local/state/gah/availability.json")
        );
    }

    // ── TICKET-069: reason/source display strings match the wire format ────

    #[test]
    fn reason_as_str_matches_serde_snake_case_wire_format() {
        for reason in [
            Reason::RateLimited,
            Reason::QuotaExhausted,
            Reason::AuthenticationError,
            Reason::BackendOutage,
            Reason::ContextLimit,
            Reason::ManualDisable,
            Reason::Unknown,
        ] {
            let wire = serde_json::to_string(&reason).unwrap();
            assert_eq!(wire, format!("\"{}\"", reason.as_str()));
        }
    }

    #[test]
    fn source_as_str_matches_serde_snake_case_wire_format() {
        for source in [Source::BackendError, Source::Manual, Source::Imported] {
            let wire = serde_json::to_string(&source).unwrap();
            assert_eq!(wire, format!("\"{}\"", source.as_str()));
        }
    }

    // ── TICKET-069: list_scopes / format_remaining ──────────────────────────

    #[test]
    fn list_scopes_is_empty_for_missing_state_file() {
        let tmp = TempDir::new().unwrap();
        let scopes = list_scopes(&path(&tmp), OffsetDateTime::now_utc()).unwrap();
        assert!(scopes.is_empty());
    }

    #[test]
    fn list_scopes_covers_backend_wide_and_model_specific_rows() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &p,
            "openhands",
            None,
            Reason::BackendOutage,
            Source::BackendError,
            Some(now + time::Duration::hours(1)),
            Some("outage".into()),
            now,
        )
        .unwrap();
        record_unavailable(
            &p,
            "openhands",
            Some("litellm_proxy/deepseek-v4"),
            Reason::RateLimited,
            Source::BackendError,
            Some(now + time::Duration::minutes(5)),
            None,
            now,
        )
        .unwrap();
        record_available(&p, "codex", None, Source::Manual, now).unwrap();

        let scopes = list_scopes(&p, now).unwrap();
        assert_eq!(scopes.len(), 3);

        let openhands_backend = scopes
            .iter()
            .find(|s| s.backend == "openhands" && s.model.is_none())
            .unwrap();
        assert!(!openhands_backend.eligible);
        assert_eq!(openhands_backend.reason, Some(Reason::BackendOutage));
        assert_eq!(openhands_backend.source, Some(Source::BackendError));
        assert_eq!(
            openhands_backend.last_error_summary.as_deref(),
            Some("outage")
        );

        let openhands_model = scopes
            .iter()
            .find(|s| s.backend == "openhands" && s.model.is_some())
            .unwrap();
        // Backend-wide outage takes precedence, so this row is also
        // ineligible, and its *reported* reason is the backend-wide one --
        // the model-specific rate-limit is masked, same as availability_for.
        assert!(!openhands_model.eligible);
        assert_eq!(openhands_model.reason, Some(Reason::BackendOutage));

        let codex = scopes.iter().find(|s| s.backend == "codex").unwrap();
        assert!(codex.eligible);
    }

    #[test]
    fn format_remaining_renders_hours_and_minutes() {
        let now = OffsetDateTime::now_utc();
        let until = now_rfc3339(now + time::Duration::minutes(134)); // 2h 14m
        assert_eq!(format_remaining(&until, now).as_deref(), Some("2h 14m"));
    }

    #[test]
    fn format_remaining_renders_minutes_only_under_an_hour() {
        let now = OffsetDateTime::now_utc();
        let until = now_rfc3339(now + time::Duration::minutes(9));
        assert_eq!(format_remaining(&until, now).as_deref(), Some("9m"));
    }

    #[test]
    fn format_remaining_returns_none_for_a_past_timestamp() {
        let now = OffsetDateTime::now_utc();
        let until = now_rfc3339(now - time::Duration::minutes(5));
        assert_eq!(format_remaining(&until, now), None);
    }

    #[test]
    fn quota_pool_blocks_associated_candidates() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();

        // 1. Initially it should be eligible
        let d = super::availability_for(
            &p,
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            now,
        )
        .unwrap();
        assert!(d.eligible);

        // 2. Mark the pool unavailable
        super::record_unavailable(
            &p,
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::hours(1)),
            None,
            now,
        )
        .unwrap();

        // 3. Now the candidate with that pool is blocked
        let d1 = super::availability_for(
            &p,
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            now,
        )
        .unwrap();
        assert!(!d1.eligible);
        assert_eq!(d1.scope, Some(BlockScope::QuotaPool));

        // 4. A different candidate sharing the pool is also blocked!
        let d2 =
            super::availability_for(&p, "claude", Some("claude-haiku"), Some("claude-main"), now)
                .unwrap();
        assert!(!d2.eligible);
        assert_eq!(d2.scope, Some(BlockScope::QuotaPool));

        // 5. A candidate NOT sharing the pool is eligible
        let d3 = super::availability_for(
            &p,
            "claude",
            Some("claude-haiku"),
            Some("claude-other"),
            now,
        )
        .unwrap();
        assert!(d3.eligible);

        // 6. Clearing the pool with record_available
        super::record_available(
            &p,
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            Source::Manual,
            now,
        )
        .unwrap();
        let d4 = super::availability_for(
            &p,
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            now,
        )
        .unwrap();
        assert!(d4.eligible);
    }

    #[test]
    fn cli_clear_marks_backend_and_model_available_without_touching_others() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &p,
            "codex",
            Some("gpt-5.4-mini"),
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::hours(5)),
            Some("quota exhausted".into()),
            now,
        )
        .unwrap();
        record_unavailable(
            &p,
            "vibe",
            Some("default"),
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::hours(5)),
            Some("unrelated".into()),
            now,
        )
        .unwrap();

        // Pre-condition: codex model is blocked.
        assert!(
            !availability_for(&p, "codex", Some("gpt-5.4-mini"), now)
                .unwrap()
                .eligible
        );

        cli::clear(&p, "codex", Some("gpt-5.4-mini"), None).unwrap();

        let d_codex = availability_for(&p, "codex", Some("gpt-5.4-mini"), now).unwrap();
        assert!(d_codex.eligible, "cleared scope must be eligible again");
        let d_vibe = availability_for(&p, "vibe", Some("default"), now).unwrap();
        assert!(!d_vibe.eligible, "clear must not touch other backends");

        // Append-only: the original blocking record is preserved, with a
        // newer manual `available` record now winning the scope.
        let state = load_state(&p).unwrap();
        assert!(state.records.iter().any(|r| r.backend == "codex"
            && r.model.as_deref() == Some("gpt-5.4-mini")
            && r.status == Status::Unavailable));
        assert!(state.records.iter().any(|r| r.backend == "codex"
            && r.model.as_deref() == Some("gpt-5.4-mini")
            && r.status == Status::Available
            && r.source == Source::Manual));
    }

    #[test]
    fn cli_clear_without_model_marks_backend_wide_available() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();
        record_unavailable(
            &p,
            "agy",
            Some("Gemini 3.5 Flash (Medium)"),
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::hours(5)),
            None,
            now,
        )
        .unwrap();
        record_unavailable(
            &p,
            "agy",
            Some("Claude Sonnet 4.6 (Thinking)"),
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::hours(5)),
            None,
            now,
        )
        .unwrap();

        cli::clear(&p, "agy", None, None).unwrap();
        assert!(
            availability_for(&p, "agy", Some("Gemini 3.5 Flash (Medium)"), now)
                .unwrap()
                .eligible
        );
        assert!(
            availability_for(&p, "agy", Some("Claude Sonnet 4.6 (Thinking)"), now)
                .unwrap()
                .eligible
        );
    }

    #[test]
    fn cli_clear_with_quota_pool_marks_only_that_pool() {
        let tmp = TempDir::new().unwrap();
        let p = path(&tmp);
        let now = OffsetDateTime::now_utc();
        super::record_unavailable(
            &p,
            "claude",
            Some("claude-sonnet"),
            Some("claude-main"),
            Reason::QuotaExhausted,
            Source::BackendError,
            Some(now + time::Duration::hours(1)),
            None,
            now,
        )
        .unwrap();
        assert!(
            !super::availability_for(
                &p,
                "claude",
                Some("claude-sonnet"),
                Some("claude-main"),
                now
            )
            .unwrap()
            .eligible
        );

        cli::clear(&p, "claude", None, Some("claude-main")).unwrap();

        assert!(
            super::availability_for(
                &p,
                "claude",
                Some("claude-sonnet"),
                Some("claude-main"),
                now
            )
            .unwrap()
            .eligible,
            "clearing the pool must unblock candidates sharing it"
        );

        // Append-only: exactly one new manual record added.
        let state = load_state(&p).unwrap();
        let manual: Vec<_> = state
            .records
            .iter()
            .filter(|r| r.source == Source::Manual && r.status == Status::Available)
            .collect();
        assert_eq!(manual.len(), 1);
        assert_eq!(manual[0].quota_pool.as_deref(), Some("claude-main"));
    }
}
