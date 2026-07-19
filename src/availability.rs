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
//! Routing, runner failure classification, the availability CLI, and quota
//! reporting all consume this store; keep their scope semantics centralized
//! here so they cannot drift.

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

/// Resolve or derive the quota pool for a candidate given its backend, model,
/// and optional configured pool tag.
pub fn resolve_candidate_quota_pool(
    backend: &str,
    model: Option<&str>,
    configured_pool: Option<&str>,
) -> Option<String> {
    if let Some(pool) = configured_pool {
        // Only reinterpret the legacy account-wide tag for this AGY
        // instance. Custom pools (including names beginning with `agy-`) and
        // fully-qualified pools are explicit operator intent.
        let legacy_account_pool =
            agy_account(backend).is_some_and(|account| pool == account || pool == backend);
        if !pool.contains(':') && legacy_account_pool && model.is_some() {
            if let Some(derived) = derive_quota_pool(backend, model) {
                return Some(derived);
            }
        }
        return Some(pool.to_string());
    }
    derive_quota_pool(backend, model)
}

/// Canonical quota account for an AGY backend instance. `agy-main` is the
/// wrapper for the same default-HOME account as backward-compatible `agy`;
/// `agy-second` and future named instances have independent account state.
pub(crate) fn agy_account(backend: &str) -> Option<&str> {
    match backend {
        "agy" | "agy-main" => Some("agy"),
        b if b.starts_with("agy-") => Some(b),
        _ => None,
    }
}

/// Derive a per-account, per-pool quota-pool tag for AGY (`agy:google-native`, etc.).
pub fn derive_quota_pool(backend: &str, model: Option<&str>) -> Option<String> {
    let account = agy_account(backend)?;
    let model = model?.trim();
    if model.is_empty() {
        return None;
    }
    let pool = if model.to_ascii_lowercase().starts_with("gemini") {
        "google-native"
    } else {
        "external"
    };
    Some(format!("{account}:{pool}"))
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
/// Kept as explicit fields because callers construct this record from several
/// independently observed backend signals.
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
#[path = "availability/tests.rs"]
mod tests;
