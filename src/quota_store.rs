//! #166 (within #151): durable store for *account-level* quota observations.
//!
//! Per-attempt usage (tokens, per-attempt cost) is already recorded in the
//! ledger via `usage.rs`'s structured parsers. This module holds the separate
//! *account-level* quota picture that only surfaces through a backend's own
//! status/quota endpoint — e.g. Codex app-server rate limits — which is not part
//! of any single attempt's log.
//!
//! The store mirrors `availability.rs`'s design philosophy: append-only JSONL
//! (not an in-place keyed map), so concurrent GAH processes can never erase
//! each other's writes, and "current state" for a (backend, model) scope is
//! derived by reading the latest record in its scope. A missing file is a
//! clean empty state, never an error.

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
mod identity;
mod instances;
pub(crate) use identity::current_source_records;
pub use identity::{
    latest_windows_for_backend, latest_windows_for_identity,
    latest_windows_for_identity_and_credential,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaObservationRecord {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_used_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_remaining_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_reset_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mistral_admin: Option<MistralAdminObservationRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_usage: Option<crate::usage::account_usage::AccountUsageObservation>,
}

/// Persisted Mistral Admin API payloads associated with a single refresh.
/// These stay optional so a refresh with only one successful endpoint never
/// fabricates the others.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralAdminObservationRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_usage: Option<crate::ledger::LedgerUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing: Option<crate::ledger::LedgerUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<crate::usage::AdminRateLimits>,
}

/// Global, not per-profile (like `availability.rs`): Codex/Claude/AGY
/// subscription limits are shared across every repo GAH touches.
/// `GAH_QUOTA_STORE_PATH` is an explicit override, matching the existing
/// `GAH_AVAILABILITY_PATH`/`GAH_LEDGER_PATH` convention -- lets
/// `refresh_stale_quota_observations`'s callers (routing, in particular)
/// call this internally without every caller threading a path parameter
/// down through several layers just for test isolation.
pub fn store_path() -> PathBuf {
    if let Ok(path) = std::env::var("GAH_QUOTA_STORE_PATH") {
        return PathBuf::from(path);
    }
    // Unit tests must never read the operator's real store: the factory's
    // validation gate runs `cargo test` with the service's XDG_STATE_HOME,
    // and real quota observations there change routing decisions. Tests
    // that need observations set GAH_QUOTA_STORE_PATH (QuotaStoreEnvGuard).
    if cfg!(test) {
        return std::env::temp_dir()
            .join(format!("gah-unit-test-quota-store-{}", std::process::id()))
            .join("quota_observations.jsonl");
    }
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        Path::new(&dir).join("gah").join("quota_observations.jsonl")
    } else {
        Path::new(&std::env::var("HOME").unwrap_or_default())
            .join(".local")
            .join("state")
            .join("gah")
            .join("quota_observations.jsonl")
    }
}

/// Load readable records, skipping malformed JSONL lines. A missing file is
/// an empty list; other read failures are returned to the caller.
pub fn load(state_path: &Path) -> Result<Vec<QuotaObservationRecord>> {
    let content = match fs::read_to_string(state_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error).context("read quota store"),
    };
    let mut records = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Skip only the malformed line, not the whole file: one corrupt JSONL
        // record (e.g. a partial write) must not discard every valid
        // observation before/after it. Mirrors availability.rs's resilience.
        match serde_json::from_str::<QuotaObservationRecord>(line) {
            Ok(mut rec) => {
                rec.backend = crate::config::canonical_backend_name(&rec.backend).to_string();
                records.push(rec);
            }
            Err(_) => continue,
        }
    }
    Ok(records)
}

/// Load from the canonical global path, swallowing any error to an empty list.
pub fn load_account_observations() -> Vec<QuotaObservationRecord> {
    load(&store_path()).unwrap_or_default()
}

/// Validate secret-free provider observations supplied by a local collector.
/// Explicit instance identity is required so another account cannot inherit them.
pub fn parse_external_observation(input: &str) -> Result<QuotaObservationRecord> {
    let value: serde_json::Value =
        serde_json::from_str(input).context("parse quota observation JSON")?;
    let fields = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("quota observation must be an object"))?;
    let allowed = [
        "backend",
        "backend_instance",
        "credential_id",
        "model",
        "quota_pool",
        "quota_window",
        "quota_used_percent",
        "quota_remaining_percent",
        "quota_reset_at",
        "observed_at",
        "checked_at",
        "check_error",
        "usage_source",
        "account_usage",
    ];
    if fields.keys().any(|key| !allowed.contains(&key.as_str())) {
        anyhow::bail!("unsupported quota observation field");
    }
    let mut record: QuotaObservationRecord = serde_json::from_value(value)
        .map_err(|_| anyhow::anyhow!("invalid quota observation schema"))?;
    if let Some(usage) = &record.account_usage {
        usage.validate()?;
    }
    for (name, value) in [
        ("backend", Some(record.backend.as_str())),
        ("backend instance", record.backend_instance.as_deref()),
        ("credential ID", record.credential_id.as_deref()),
        ("quota pool", record.quota_pool.as_deref()),
        ("usage source", record.usage_source.as_deref()),
    ] {
        if let Some(value) = value {
            if crate::execution_identity::validate_operator_label(name, value)? != value {
                anyhow::bail!("{name} must not contain surrounding whitespace");
            }
        }
    }
    if record.backend_instance.is_none()
        || record.checked_at.is_none()
        || record.usage_source.is_none()
    {
        anyhow::bail!("backend_instance, checked_at and usage_source are required");
    }
    for value in [record.quota_used_percent, record.quota_remaining_percent]
        .into_iter()
        .flatten()
    {
        if !value.is_finite() || !(0.0..=100.0).contains(&value) {
            anyhow::bail!("quota percentages must be between 0 and 100");
        }
    }
    for value in [
        record.checked_at.as_deref(),
        record.observed_at.as_deref(),
        record.quota_reset_at.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        OffsetDateTime::parse(value, &Rfc3339).context("quota timestamp must be RFC 3339")?;
    }
    for (name, value, limit) in [
        ("model", record.model.as_deref(), 512),
        ("quota window", record.quota_window.as_deref(), 128),
        ("check error", record.check_error.as_deref(), 1024),
    ] {
        if let Some(value) = value {
            if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
                anyhow::bail!("invalid {name}");
            }
        }
    }
    if has_quota_data(&record) && record.observed_at.is_none() {
        anyhow::bail!("quota data requires observed_at");
    }
    record.backend = crate::config::canonical_backend_name(&record.backend).to_string();
    record.check_error = record.check_error.as_deref().map(crate::redact::redact);
    Ok(record)
}

fn has_ledger_usage_data(usage: &crate::ledger::LedgerUsage) -> bool {
    usage.usage_source.is_some()
        || usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.reasoning_tokens.is_some()
        || usage.cache_read_tokens.is_some()
        || usage.cache_write_tokens.is_some()
        || usage.total_tokens.is_some()
        || usage.requests_count.is_some()
        || usage.estimated_cost_usd.is_some()
        || usage.actual_cost_usd.is_some()
        || usage.quota_window.is_some()
        || usage.quota_used_percent.is_some()
        || usage.quota_remaining_percent.is_some()
        || usage.quota_reset_at.is_some()
}

fn has_admin_rate_limits_data(limits: &crate::usage::AdminRateLimits) -> bool {
    limits.requests_per_second.is_some() || !limits.model_limits.is_empty()
}

fn mistral_admin_observation(
    refresh: &crate::usage::AdminRefresh,
) -> Option<MistralAdminObservationRecord> {
    let workspace_usage =
        has_ledger_usage_data(&refresh.workspace_usage).then_some(refresh.workspace_usage.clone());
    let billing = has_ledger_usage_data(&refresh.billing).then_some(refresh.billing.clone());
    let rate_limits =
        has_admin_rate_limits_data(&refresh.rate_limits).then_some(refresh.rate_limits.clone());
    if workspace_usage.is_none() && billing.is_none() && rate_limits.is_none() {
        None
    } else {
        Some(MistralAdminObservationRecord {
            workspace_usage,
            billing,
            rate_limits,
        })
    }
}

/// Append one record under an exclusive lock. Missing parent dirs are created.
pub fn append(state_path: &Path, rec: &QuotaObservationRecord) -> Result<()> {
    for (field, value) in [
        ("backend instance", rec.backend_instance.as_deref()),
        ("quota pool", rec.quota_pool.as_deref()),
    ] {
        if let Some(value) = value {
            let normalized = crate::execution_identity::validate_operator_label(field, value)?;
            if normalized != value {
                anyhow::bail!("{field} must not contain surrounding whitespace");
            }
        }
    }
    if let Some(parent) = state_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_path)
        .context("open quota store")?;
    file.lock_exclusive()
        .with_context(|| format!("locking {}", state_path.display()))?;
    let line = serde_json::to_string(rec).context("serialize quota observation")?;
    writeln!(file, "{line}").context("write quota observation")?;
    let _ = file.unlock();
    Ok(())
}

/// Most-recent observation for an exact execution identity, with a
/// deterministic fallback to legacy instance-unknown rows. A row for a
/// different explicit instance never matches, even when backend/model agree.
pub fn latest_for_identity<'a>(
    records: &'a [QuotaObservationRecord],
    identity: &crate::execution_identity::ExecutionIdentity,
) -> Option<&'a QuotaObservationRecord> {
    latest_windows_for_identity(records, identity)
        .into_iter()
        .max_by(|left, right| left.observed_at.cmp(&right.observed_at))
}

fn has_quota_data(record: &QuotaObservationRecord) -> bool {
    record.quota_used_percent.is_some()
        || record.quota_remaining_percent.is_some()
        || record.quota_window.is_some()
        || record.quota_reset_at.is_some()
        || record.account_usage.is_some()
}

/// #166: read every Codex app-server account window and append one row per
/// window. `identity` scopes the rows to an explicit instance and pool;
/// without it they are ambient account readings. `environment` isolates a
/// named instance's login. Errors when the request fails or has no quota data.
pub fn refresh_codex_and_store(
    codex_cmd: &str,
    model: Option<&str>,
    identity: Option<&crate::execution_identity::ExecutionIdentity>,
    environment: &[(String, String)],
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let records = crate::usage::refresh_codex_quota(codex_cmd, model, environment)
        .map_err(|error| anyhow::anyhow!("Codex app-server quota check failed: {error}"))?;
    append_scoped(state_path, records, identity)
}

/// Append a check's windows, scoped to `identity` when the source belongs to
/// one explicit instance. Returns the first window for callers that report it.
pub(crate) fn append_scoped(
    state_path: &Path,
    mut records: Vec<QuotaObservationRecord>,
    identity: Option<&crate::execution_identity::ExecutionIdentity>,
) -> Result<Option<QuotaObservationRecord>> {
    for record in &mut records {
        if let Some(identity) = identity {
            record.backend = identity.logical_backend.clone();
            record.backend_instance = Some(identity.backend_instance.clone());
            record.quota_pool = identity.quota_pool.clone();
        }
        append(state_path, record)?;
    }
    Ok(records.into_iter().next())
}

/// #154: refresh account-level Mistral Admin API data (aggregate usage,
/// billing, rate-limit ceilings, spend-limit percent) and persist the
/// spend-limit reading -- the only piece of that refresh that collapses
/// into this store's (backend, model) -> quota-percent shape; aggregate
/// token/billing figures have no durable sink of their own yet. Returns
/// Missing credentials are an `auth_required` error. `Ok(None)` means the
/// Admin API yielded no persisted observation at all; partial aggregate data still
/// gets stored and returned as `Some(...)` so callers can distinguish
/// "nothing recorded" from "no spend-limit reading, but other admin data was
/// captured."
pub fn refresh_vibe_admin_and_store(
    model: Option<&str>,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let api_key = crate::usage::admin_api_key()
        .ok_or_else(|| anyhow::anyhow!("auth_required: MISTRAL_ADMIN_API_KEY is not configured"))?;
    let record = refresh_vibe_admin_record(&api_key, model)?;
    if let Some(record) = &record {
        append(state_path, record)?;
    }
    Ok(record)
}

pub(crate) fn refresh_vibe_admin_record(
    api_key: &str,
    model: Option<&str>,
) -> Result<Option<QuotaObservationRecord>> {
    let end_time = time::OffsetDateTime::now_utc().unix_timestamp();
    let thirty_days_secs = 30 * 24 * 60 * 60;
    let refresh = crate::usage::refresh_admin_data(
        api_key,
        (end_time - thirty_days_secs, end_time),
        "vibe",
        model,
    );
    let admin_refresh = mistral_admin_observation(&refresh);
    let Some(obs) = refresh.spend_limit else {
        if let Some(admin_refresh) = admin_refresh {
            let rec = QuotaObservationRecord {
                backend: "vibe".to_string(),
                backend_instance: None,
                model: model.map(str::to_string),
                quota_pool: None,
                quota_window: None,
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: None,
                observed_at: time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .ok(),
                checked_at: OffsetDateTime::now_utc().format(&Rfc3339).ok(),
                check_error: refresh.spend_limit_error,
                usage_source: Some("mistral_admin_refresh".to_string()),
                mistral_admin: Some(admin_refresh),
                account_usage: None,
                credential_id: None,
            };
            return Ok(Some(rec));
        }
        if let Some(error) = refresh.spend_limit_error {
            anyhow::bail!(error);
        }
        return Ok(None);
    };
    let rec = QuotaObservationRecord {
        checked_at: OffsetDateTime::now_utc().format(&Rfc3339).ok(),
        mistral_admin: admin_refresh,
        ..obs
    };
    Ok(Some(rec))
}

/// Persist both native Claude allowance windows under the default account's
/// explicit instance. Named Claude accounts cannot inherit these readings.
pub fn refresh_claude_and_store(state_path: &Path) -> Result<Option<QuotaObservationRecord>> {
    let records = crate::usage::claude::refresh()?;
    for record in &records {
        append(state_path, record)?;
    }
    Ok(records.into_iter().next())
}

/// Issue #761: nothing refreshed this store periodically -- only a human
/// running `gah quota refresh` by hand did, so account-level quota data
/// went stale for days even while dispatch itself was active. Called by
/// `gah quota auto-refresh`, which the systemd timer and the dashboard server's
/// scheduler both run every 15 minutes (not from `gah loop`; see
/// `controller::runtime::probe`). Throttled to
/// `QUOTA_REFRESH_INTERVAL` per source so this can't hammer either
/// endpoint: Codex's app-server is a local CLI call, but
/// vibe's is a real network call against the Mistral Admin API with its own
/// rate limits. Best-effort: a refresh failure (backend not installed, no
/// `MISTRAL_ADMIN_API_KEY`) must not abort the auto-refresh oneshot run, so errors are
/// recorded per source rather than propagated.
///
/// Returns the spawned refresh threads (one per backend that was actually
/// due and not already in flight) so a caller that must outlive them -- the
/// dedicated `gah quota auto-refresh` oneshot CLI -- can join them.
pub fn refresh_stale_quota_observations(
    profile: &crate::config::Profile,
    now: OffsetDateTime,
    store_path: &Path,
) -> Vec<std::thread::JoinHandle<()>> {
    let codex_cmd = profile
        .codex_path
        .clone()
        .unwrap_or_else(|| "codex".to_string());
    let mut handles = Vec::new();
    handles.extend(instances::refresh(profile, now, store_path));
    if let Some(handle) =
        maybe_refresh_backend_instance(store_path, "claude", Some("claude"), now, {
            let path = store_path.to_path_buf();
            move || refresh_claude_and_store(&path)
        })
    {
        handles.push(handle);
    }
    if crate::usage::nous::configured() {
        if let Some(handle) = maybe_refresh_backend_instance(
            store_path,
            "opencode",
            Some("opencode:nous-portal-api"),
            now,
            {
                let path = store_path.to_path_buf();
                move || {
                    let record = match crate::usage::nous::refresh() {
                        Ok(record) => record,
                        Err(error) => {
                            let mut record =
                                crate::usage::nous::parse(b"{}", OffsetDateTime::now_utc())?;
                            record.check_error = Some(crate::redact::redact(&error.to_string()));
                            record.observed_at = None;
                            record
                        }
                    };
                    append(&path, &record)?;
                    Ok(Some(record))
                }
            },
        ) {
            handles.push(handle);
        }
    }
    if let Some(handle) = maybe_refresh_backend(store_path, "codex", now, {
        let codex_cmd = codex_cmd.clone();
        let path = store_path.to_path_buf();
        move || refresh_codex_and_store(&codex_cmd, None, None, &[], &path)
    }) {
        handles.push(handle);
    }
    if crate::usage::mistral_dashboard::configured() {
        let instance = load(store_path)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .find(|record| record.backend == "mistral-dashboard" && record.credential_id.is_none())
            .and_then(|record| record.backend_instance);
        if let Some(handle) = maybe_refresh_backend_instance(
            store_path,
            "mistral-dashboard",
            instance.as_deref(),
            now,
            {
                let path = store_path.to_path_buf();
                move || crate::usage::mistral_dashboard::refresh_scheduled(&path)
            },
        ) {
            handles.push(handle);
        }
    }
    if crate::usage::admin_api_key().is_some() {
        if let Some(handle) = maybe_refresh_backend(store_path, "vibe", now, {
            let path = store_path.to_path_buf();
            move || refresh_vibe_admin_and_store(None, &path)
        }) {
            handles.push(handle);
        }
    }
    for info in crate::credentials::list().unwrap_or_default() {
        let id = info.id.clone();
        let path = store_path.to_path_buf();
        if let Some(handle) = maybe_refresh_source(
            store_path,
            crate::credentials::quota::backend(&info),
            None,
            Some(&info.id),
            now,
            move || crate::credentials::quota::refresh(&id, &path).map(Some),
        ) {
            handles.push(handle);
        }
    }
    handles
}

/// Issue #761: blocking variant for the dedicated `gah quota auto-refresh`
/// CLI command, which a systemd oneshot timer runs. Spawns the per-backend
/// refresh threads (same throttle/supervision as
/// `refresh_stale_quota_observations`) and JOINS them, so the oneshot
/// process does not exit and kill the threads mid-refresh. Each backend
/// refresh is independently bounded (Codex app-server timeout / curl
/// `--max-time`), so joining is bounded. Returns how many backends were
/// actually refreshed (0 = nothing was due).
pub fn refresh_quota_observations_and_wait(
    profile: &crate::config::Profile,
    now: OffsetDateTime,
    store_path: &Path,
) -> usize {
    let handles = refresh_stale_quota_observations(profile, now, store_path);
    let count = handles.len();
    for handle in handles {
        let _ = handle.join();
    }
    count
}

/// How long routing trusts an account reading (`routing::subscription::capacity`
/// and live pacing). Older readings count as unknown capacity.
pub const QUOTA_FRESHNESS: time::Duration = time::Duration::minutes(30);

/// Minimum time between live checks of one source. It must stay below
/// `QUOTA_FRESHNESS` minus the 15-minute scheduler tick, so every reading is
/// replaced before routing stops trusting it (#1331).
const QUOTA_REFRESH_INTERVAL: time::Duration = time::Duration::minutes(14);

/// Refreshes run on detached threads so a provider check never delays a loop
/// tick. Each provider call is bounded, and `IN_FLIGHT` prevents overlapping
/// checks for the same backend. Codex app-server children are also registered
/// with the runner shutdown path so a graceful stop kills them promptly.
static IN_FLIGHT: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

fn maybe_refresh_backend(
    path: &Path,
    backend: &str,
    now: OffsetDateTime,
    refresh: impl FnOnce() -> Result<Option<QuotaObservationRecord>> + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    maybe_refresh_backend_instance(path, backend, None, now, refresh)
}

fn maybe_refresh_backend_instance(
    path: &Path,
    backend: &str,
    instance: Option<&str>,
    now: OffsetDateTime,
    refresh: impl FnOnce() -> Result<Option<QuotaObservationRecord>> + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    maybe_refresh_source(path, backend, instance, None, now, refresh)
}

fn maybe_refresh_source(
    path: &Path,
    backend: &str,
    instance: Option<&str>,
    credential_id: Option<&str>,
    now: OffsetDateTime,
    refresh: impl FnOnce() -> Result<Option<QuotaObservationRecord>> + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    let records = load(path).unwrap_or_default();
    let last_checked = records
        .iter()
        .filter(|record| {
            record.backend == backend
                && record.credential_id.as_deref() == credential_id
                && (credential_id.is_some() || record.backend_instance.as_deref() == instance)
        })
        .filter_map(|record| {
            record
                .checked_at
                .as_deref()
                .or(record.observed_at.as_deref())
        })
        .filter_map(|observed_at| OffsetDateTime::parse(observed_at, &Rfc3339).ok())
        .max();
    let due = match last_checked {
        None => true,
        Some(last) => now - last > QUOTA_REFRESH_INTERVAL,
    };
    if !due {
        return None;
    }
    let in_flight_key = format!(
        "{backend}\0{}\0{}",
        instance.unwrap_or_default(),
        credential_id.unwrap_or_default()
    );
    {
        let mut in_flight = IN_FLIGHT
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let set = in_flight.get_or_insert_with(std::collections::HashSet::new);
        if !set.insert(in_flight_key.clone()) {
            // A previous tick's refresh for this exact backend hasn't
            // returned yet -- don't pile on a second attempt on top of it.
            return None;
        }
    }
    let path = path.to_path_buf();
    let backend = backend.to_string();
    let instance = instance.map(str::to_string);
    let credential_id = credential_id.map(str::to_string);
    Some(std::thread::spawn(move || {
        // `refresh_*_and_store` already appends a real record when it
        // finds data (Ok(Some(_))); on Ok(None)/Err, append a data-free
        // marker purely so the next tick's `last_checked` sees a fresh
        // attempt and stops retrying every tick against a backend that
        // isn't configured here at all.
        let refresh = refresh();
        // Named refresh owns its scoped failure marker and rejects obsolete
        // generations. A generic fallback must never revive a removed source.
        if credential_id.is_none() && !matches!(&refresh, Ok(Some(_))) {
            let marker = QuotaObservationRecord {
                backend: backend.clone(),
                backend_instance: instance,
                model: None,
                quota_pool: None,
                quota_window: None,
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: None,
                observed_at: None,
                checked_at: now.format(&Rfc3339).ok(),
                check_error: refresh
                    .err()
                    .map(|error| crate::redact::redact(&error.to_string())),
                usage_source: None,
                mistral_admin: None,
                account_usage: None,
                credential_id,
            };
            let _ = append(&path, &marker);
        }
        if let Ok(mut in_flight) = IN_FLIGHT.lock() {
            if let Some(set) = in_flight.as_mut() {
                set.remove(&in_flight_key);
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota_observations.jsonl");
        (dir, path)
    }

    // Issue #761: maybe_refresh_backend is the throttle gate for the new
    // periodic quota refresh -- these test it directly rather than through
    // refresh_stale_quota_observations, which needs a real codex/vibe
    // subprocess to actually observe anything. The refresh itself now runs
    // on a detached thread (see IN_FLIGHT's doc comment for why), so these
    // wait on the one externally-observable effect -- a marker record
    // landing in the store -- instead of asserting immediately. Each test
    // uses its own synthetic backend name so parallel test threads can't
    // collide on the process-wide IN_FLIGHT set.
    fn wait_for_backend_record(path: &Path, backend: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if load(path)
                .unwrap_or_default()
                .iter()
                .any(|record| record.backend == backend)
            {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn maybe_refresh_backend_calls_the_refresher_when_nothing_is_recorded_yet() {
        let (_dir, path) = tmp_store();
        let now = OffsetDateTime::now_utc();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_in_thread = calls.clone();

        maybe_refresh_backend(&path, "test-calls-when-empty", now, move || {
            calls_in_thread.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        });

        assert!(wait_for_backend_record(&path, "test-calls-when-empty"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn maybe_refresh_backend_skips_a_backend_checked_within_the_interval() {
        let (_dir, path) = tmp_store();
        let now = OffsetDateTime::now_utc();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let first = calls.clone();
        maybe_refresh_backend(&path, "test-skip-within-interval", now, move || {
            first.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        });
        assert!(wait_for_backend_record(&path, "test-skip-within-interval"));
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "first call must actually refresh"
        );

        // Ten minutes later, inside QUOTA_REFRESH_INTERVAL.
        let second = calls.clone();
        maybe_refresh_backend(
            &path,
            "test-skip-within-interval",
            now + time::Duration::minutes(10),
            move || {
                second.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(None)
            },
        );
        // Nothing async to wait for here -- a throttled call never spawns a
        // thread, so the skip is synchronous. A brief settle is still fair
        // in case it wrongly did spawn one.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "must not re-refresh before the interval elapses"
        );
    }

    #[test]
    fn maybe_refresh_backend_refreshes_again_once_the_interval_elapses() {
        let (_dir, path) = tmp_store();
        let now = OffsetDateTime::now_utc();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let first = calls.clone();
        maybe_refresh_backend(&path, "test-refresh-again", now, move || {
            first.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        })
        .unwrap()
        .join()
        .unwrap();
        assert!(wait_for_backend_record(&path, "test-refresh-again"));

        let second = calls.clone();
        maybe_refresh_backend(
            &path,
            "test-refresh-again",
            now + QUOTA_REFRESH_INTERVAL + time::Duration::minutes(1),
            move || {
                second.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(None)
            },
        )
        .unwrap()
        .join()
        .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn every_fifteen_minute_tick_refreshes_before_routing_distrusts_the_reading() {
        // #1331: a 30-minute throttle against a 30-minute freshness window
        // skipped every other server tick, leaving routing blind half the hour.
        let tick = time::Duration::minutes(15);
        assert!(QUOTA_REFRESH_INTERVAL < tick);
        assert!(tick < QUOTA_FRESHNESS);
        let (_dir, path) = tmp_store();
        let start = OffsetDateTime::parse("2026-10-03T18:00:00Z", &Rfc3339).unwrap();
        let identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "test-tick-cadence",
            None::<String>,
            None::<String>,
        );
        for n in 0..4 {
            let now = start + tick * n;
            if n > 0 {
                assert!(
                    crate::routing::subscription::capacity(&identity, &load(&path).unwrap(), now)
                        .known_capacity,
                    "capacity must stay known before tick {n} refreshes"
                );
            }
            let refresh_path = path.clone();
            maybe_refresh_backend(&path, "test-tick-cadence", now, move || {
                let reading: QuotaObservationRecord = serde_json::from_value(serde_json::json!({
                    "backend": "test-tick-cadence", "quota_window": "weekly",
                    "quota_remaining_percent": 50, "observed_at": now.format(&Rfc3339).unwrap(),
                    "checked_at": now.format(&Rfc3339).unwrap(),
                    "quota_reset_at": (start + time::Duration::days(7)).format(&Rfc3339).unwrap()
                }))
                .unwrap();
                append(&refresh_path, &reading)?;
                Ok(Some(reading))
            })
            .unwrap_or_else(|| panic!("tick {n} was throttled"))
            .join()
            .unwrap();
            assert!(
                crate::routing::subscription::capacity(&identity, &load(&path).unwrap(), now)
                    .known_capacity,
                "capacity must be known after tick {n} refreshes"
            );
        }
    }

    #[test]
    fn maybe_refresh_backend_throttles_on_a_failed_attempt_too() {
        // A backend that's simply not installed here (or a network hiccup)
        // must not be retried every single loop tick forever -- that's a
        // real "don't DDoS it" failure mode, not just the happy path.
        let (_dir, path) = tmp_store();
        let now = OffsetDateTime::now_utc();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let first = calls.clone();
        maybe_refresh_backend(&path, "test-throttle-on-failure", now, move || {
            first.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("vibe not installed")
        });
        assert!(wait_for_backend_record(&path, "test-throttle-on-failure"));

        let second = calls.clone();
        maybe_refresh_backend(
            &path,
            "test-throttle-on-failure",
            now + time::Duration::minutes(5),
            move || {
                second.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                anyhow::bail!("vibe not installed")
            },
        );
        std::thread::sleep(std::time::Duration::from_millis(50));

        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a failed attempt must still throttle retries"
        );
    }

    #[test]
    fn one_backend_failure_is_recorded_without_blocking_another_backend() {
        let (_dir, path) = tmp_store();
        let now = OffsetDateTime::now_utc();

        maybe_refresh_backend(&path, "test-independent-codex", now, || {
            anyhow::bail!("codex status failed with ghp_abcdefghijklmnopqrstuvwxyz")
        });
        maybe_refresh_backend(&path, "test-independent-vibe", now, || Ok(None));

        assert!(wait_for_backend_record(&path, "test-independent-codex"));
        assert!(wait_for_backend_record(&path, "test-independent-vibe"));
        let records = load(&path).unwrap();
        let codex = records
            .iter()
            .find(|record| record.backend == "test-independent-codex")
            .unwrap();
        let vibe = records
            .iter()
            .find(|record| record.backend == "test-independent-vibe")
            .unwrap();
        assert_eq!(
            codex.check_error.as_deref(),
            Some("codex status failed with [REDACTED:GITHUB_TOKEN]")
        );
        assert_eq!(vibe.check_error, None);
    }

    #[test]
    fn refresh_vibe_admin_and_store_reports_auth_required_without_api_key() {
        let _key_guard = crate::test_support::MistralAdminKeyEnvGuard::unset();
        let (_dir, path) = tmp_store();

        assert_eq!(
            refresh_vibe_admin_and_store(None, &path)
                .unwrap_err()
                .to_string(),
            "auth_required: MISTRAL_ADMIN_API_KEY is not configured"
        );
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn refresh_vibe_admin_and_store_persists_spend_limit_observation() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let _key_guard = crate::test_support::MistralAdminKeyEnvGuard::set("sk-test");
        let (_dir, path) = tmp_store();
        let bin_dir = tempfile::tempdir().unwrap();
        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mistral-admin");
        let rate_limit_path = bin_dir.path().join("rate_limit.json");
        let spend_limit_path = bin_dir.path().join("spend_limit.json");
        std::fs::write(
            &rate_limit_path,
            r#"{
  "requests_per_second": 5,
  "tokens_limits_by_model": {
    "mistral-vibe-cli-latest": {
      "tokens_per_minute": 500000,
      "tokens_per_month": 200000000
    },
    "mistral-medium-3.5": {
      "tokens_per_minute": 250000,
      "tokens_per_month": 100000000
    }
  }
}"#,
        )
        .unwrap();
        std::fs::write(
            &spend_limit_path,
            r#"{
  "limits": {
    "completion": {
      "no_monthly_limit": false,
      "monthly_limit_reached": false,
      "usage": 128.42,
      "vibe_usage": 41.1,
      "total_usage": 169.52,
      "usage_limit": 500.0,
      "usage_limit_organization": 500.0
    },
    "last_payment_failure": false,
    "last_payment_failure_protection": null,
    "currency": "USD"
  }
}"#,
        )
        .unwrap();
        let script_path = bin_dir.path().join("curl");
        std::fs::write(
            &script_path,
            format!(
                "#!/bin/sh\ncfg=$(cat)\ncase \"$cfg\" in\n  *analytics/vibe/code/usage/by_workspace*) cat '{fixtures}/vibe_workspace_usage.json' ;;\n  *v1/admin/usage*) cat '{fixtures}/usage.json' ;;\n  *v1/admin/rate-limit*) cat '{rate_limit}' ;;\n  *v1/admin/spend-limit*) cat '{spend_limit}' ;;\n  *) exit 1 ;;\nesac\n",
                rate_limit = rate_limit_path.display(),
                spend_limit = spend_limit_path.display(),
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
        let _path_guard = crate::test_support::PathGuard::set(bin_dir.path());

        let rec = refresh_vibe_admin_and_store(None, &path)
            .unwrap()
            .expect("spend limit observation persisted");
        assert_eq!(rec.backend, "vibe");
        assert_eq!(rec.quota_used_percent, Some(33.904));
        assert_eq!(
            rec.usage_source.as_deref(),
            Some("mistral_admin_spend_limit")
        );
        assert!(rec.mistral_admin.is_some());
        let admin = rec.mistral_admin.as_ref().unwrap();
        assert!(admin.workspace_usage.is_some());
        assert!(admin.billing.is_some());
        assert!(admin.rate_limits.is_some());

        let records = load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].backend, "vibe");
        assert!(records[0].mistral_admin.is_some());
    }

    #[test]
    fn refresh_vibe_admin_and_store_persists_admin_refresh_without_spend_limit() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let _key_guard = crate::test_support::MistralAdminKeyEnvGuard::set("sk-test");
        let (_dir, path) = tmp_store();
        let bin_dir = tempfile::tempdir().unwrap();
        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mistral-admin");
        let rate_limit_path = bin_dir.path().join("rate_limit.json");
        let spend_limit_path = bin_dir.path().join("spend_limit.json");
        std::fs::write(
            &rate_limit_path,
            r#"{
  "requests_per_second": 5,
  "tokens_limits_by_model": {
    "mistral-vibe-cli-latest": {
      "tokens_per_minute": 500000,
      "tokens_per_month": 200000000
    },
    "mistral-medium-3.5": {
      "tokens_per_minute": 250000,
      "tokens_per_month": 100000000
    }
  }
}"#,
        )
        .unwrap();
        std::fs::write(
            &spend_limit_path,
            r#"{
  "limits": {
    "completion": {
      "no_monthly_limit": false,
      "monthly_limit_reached": false
    },
    "currency": "USD",
    "last_payment_failure": false,
    "last_payment_failure_protection": null
  }
}"#,
        )
        .unwrap();
        let script_path = bin_dir.path().join("curl");
        std::fs::write(
            &script_path,
            format!(
                "#!/bin/sh\ncfg=$(cat)\ncase \"$cfg\" in\n  *analytics/vibe/code/usage/by_workspace*) cat '{fixtures}/vibe_workspace_usage.json' ;;\n  *v1/admin/usage*) cat '{fixtures}/usage.json' ;;\n  *v1/admin/rate-limit*) cat '{rate_limit}' ;;\n  *v1/admin/spend-limit*) cat '{spend_limit}' ;;\n  *) exit 1 ;;\nesac\n",
                rate_limit = rate_limit_path.display(),
                spend_limit = spend_limit_path.display(),
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
        let _path_guard = crate::test_support::PathGuard::set(bin_dir.path());

        let rec = refresh_vibe_admin_and_store(None, &path)
            .unwrap()
            .expect("admin refresh observation persisted");

        let records = load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(rec.backend, "vibe");
        assert!(rec.quota_used_percent.is_none());
        assert_eq!(rec.usage_source.as_deref(), Some("mistral_admin_refresh"));
        let admin = rec.mistral_admin.as_ref().expect("admin payload persisted");
        assert!(admin.workspace_usage.is_some());
        assert!(admin.billing.is_some());
        assert!(admin.rate_limits.is_some());
        assert_eq!(records[0].backend, "vibe");
        assert!(records[0].mistral_admin.is_some());
    }

    #[test]
    fn load_missing_file_is_empty() {
        let (_dir, path) = tmp_store();
        assert!(!path.exists());
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn append_then_load_round_trips() {
        let (_dir, path) = tmp_store();
        append(
            &path,
            &QuotaObservationRecord {
                backend: "codex".into(),
                backend_instance: None,
                model: Some("gpt-5".into()),
                quota_pool: None,
                quota_window: Some("300m".into()),
                quota_used_percent: Some(25.0),
                quota_remaining_percent: Some(75.0),
                quota_reset_at: Some("2026-04-29T12:00:00Z".into()),
                observed_at: Some("2026-04-28T10:00:00Z".into()),
                checked_at: None,
                check_error: None,
                usage_source: Some("codex_status_json".into()),
                mistral_admin: None,
                account_usage: None,
                credential_id: None,
            },
        )
        .unwrap();

        let records = load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].backend, "codex");
        assert_eq!(records[0].quota_used_percent, Some(25.0));
        assert_eq!(records[0].quota_remaining_percent, Some(75.0));
    }

    // Issue #206: a single malformed JSONL line must be skipped, not cause the
    // whole store (every valid record before/after it) to be discarded.
    #[test]
    fn load_skips_malformed_line_and_keeps_valid_records() {
        let (_dir, path) = tmp_store();
        let good1 = QuotaObservationRecord {
            backend: "codex".into(),
            backend_instance: None,
            model: None,
            quota_pool: None,
            quota_window: Some("weekly".into()),
            quota_used_percent: Some(10.0),
            quota_remaining_percent: Some(90.0),
            quota_reset_at: None,
            observed_at: Some("2026-04-28T10:00:00Z".into()),
            checked_at: None,
            check_error: None,
            usage_source: Some("codex_status_json".into()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        };
        let good2 = QuotaObservationRecord {
            quota_used_percent: Some(20.0),
            observed_at: Some("2026-04-29T10:00:00Z".into()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
            ..good1.clone()
        };
        let mut contents = serde_json::to_string(&good1).unwrap();
        contents.push('\n');
        contents.push_str("{ this is not valid json ]\n");
        contents.push_str(&serde_json::to_string(&good2).unwrap());
        contents.push('\n');
        std::fs::write(&path, contents).unwrap();

        let records = load(&path).unwrap();
        assert_eq!(
            records.len(),
            2,
            "the bad line should be skipped, not fatal"
        );
        assert_eq!(records[0].quota_used_percent, Some(10.0));
        assert_eq!(records[1].quota_used_percent, Some(20.0));
    }

    fn scoped_record(
        instance: Option<&str>,
        percent: f64,
        observed_at: &str,
    ) -> QuotaObservationRecord {
        QuotaObservationRecord {
            backend: "opencode".into(),
            backend_instance: instance.map(str::to_string),
            model: Some("shared-model".into()),
            quota_pool: Some("shared-pool".into()),
            quota_window: Some("daily".into()),
            quota_used_percent: Some(percent),
            quota_remaining_percent: Some(100.0 - percent),
            quota_reset_at: None,
            observed_at: Some(observed_at.into()),
            checked_at: None,
            check_error: None,
            usage_source: Some("test".into()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        }
    }

    fn identity(instance: &str) -> crate::execution_identity::ExecutionIdentity {
        let mut identity = crate::execution_identity::ExecutionIdentity::legacy_candidate(
            "opencode",
            Some("shared-model"),
            Some("shared-pool"),
        );
        identity.backend_instance = instance.into();
        identity
    }

    #[test]
    fn latest_windows_keep_short_and_weekly_limits_on_the_right_account() {
        let mut weekly = scoped_record(Some("account-a"), 20.0, "2026-07-20T10:00:00Z");
        weekly.quota_window = Some("weekly".into());
        let mut short = scoped_record(Some("account-a"), 90.0, "2026-07-20T11:00:00Z");
        short.quota_window = Some("5h".into());
        let mut old = weekly.clone();
        old.observed_at = Some("2026-07-19T10:00:00Z".into());
        let mut sibling = weekly.clone();
        sibling.backend_instance = Some("account-b".into());
        sibling.quota_used_percent = Some(99.0);
        let records = [weekly, short, old, sibling];
        let windows = latest_windows_for_identity(&records, &identity("account-a"));
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].quota_used_percent, Some(90.0));
        assert_eq!(windows[1].quota_used_percent, Some(20.0));
    }

    /// #1384 review: a newer account-wide reading must not hide an exhausted
    /// model-scoped reading of the same window from routing.
    #[test]
    fn latest_windows_keep_model_scoped_reading_beside_account_reading() {
        let mut account = scoped_record(Some("account-a"), 50.0, "2026-07-20T11:00:00Z");
        account.model = None;
        let exhausted = scoped_record(Some("account-a"), 100.0, "2026-07-20T10:00:00Z");
        let records = [account, exhausted];
        let windows = latest_windows_for_identity(&records, &identity("account-a"));
        assert_eq!(windows.len(), 2);
        assert!(windows
            .iter()
            .any(|record| record.model.is_some() && record.quota_used_percent == Some(100.0)));
    }

    #[test]
    fn newer_failed_or_empty_check_invalidates_only_its_account() {
        let success = scoped_record(Some("account-a"), 20.0, "2026-07-20T10:00:00Z");
        let sibling = scoped_record(Some("account-b"), 70.0, "2026-07-20T10:00:00Z");
        let mut failure = success.clone();
        failure.quota_window = None;
        failure.quota_used_percent = None;
        failure.quota_remaining_percent = None;
        failure.observed_at = None;
        failure.checked_at = Some("2026-07-20T11:00:00Z".into());
        for error in [None, Some("failed".into())] {
            failure.check_error = error;
            let records = [success.clone(), sibling.clone(), failure.clone()];
            assert!(latest_for_identity(&records, &identity("account-a")).is_none());
            assert_eq!(
                latest_for_identity(&records, &identity("account-b"))
                    .unwrap()
                    .quota_used_percent,
                Some(70.0)
            );
        }
    }

    #[test]
    fn account_usage_import_is_scoped_and_does_not_require_a_quota_cap() {
        let value = serde_json::json!({"backend":"mistral-dashboard", "backend_instance":"mistral-dashboard:account", "quota_pool":"mistral-dashboard:account", "checked_at":"2026-10-03T00:00:00Z", "observed_at":"2026-10-03T00:00:00Z", "usage_source":"mistral_dashboard", "account_usage":{"account_id":"customer-1", "workspace_id":null, "period_start":"2026-10-01T00:00:00Z", "period_end":"2026-10-03T00:00:00Z", "currency":"USD", "requests":4, "cost":0.03079, "cost_source":"dashboard_prices", "models":[]}});
        let record = parse_external_observation(&value.to_string()).unwrap();
        assert!(record.quota_remaining_percent.is_none());
        assert!(record.quota_reset_at.is_none());
    }

    #[test]
    fn external_observation_rejects_invalid_or_unscoped_provider_data() {
        let valid = serde_json::json!({"backend":"agy", "backend_instance":"agy-primary", "quota_pool":"agy:external", "quota_window":"weekly", "quota_remaining_percent":42.0, "checked_at":"2026-10-02T23:00:00Z", "observed_at":"2026-10-02T23:00:00Z", "usage_source":"cli_router"});
        assert_eq!(
            parse_external_observation(&valid.to_string())
                .unwrap()
                .quota_remaining_percent,
            Some(42.0)
        );
        for (field, value) in [
            ("backend_instance", serde_json::Value::Null),
            ("checked_at", serde_json::json!("bad time")),
            ("quota_remaining_percent", serde_json::json!(101)),
            ("access_token", serde_json::json!("private")),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                parse_external_observation(&invalid.to_string()).is_err(),
                "accepted invalid {field}"
            );
        }
    }

    #[test]
    fn latest_identity_observation_never_crosses_explicit_instances() {
        let records = vec![
            scoped_record(Some("account-a"), 10.0, "2026-07-20T10:00:00Z"),
            scoped_record(Some("account-b"), 70.0, "2026-07-20T11:00:00Z"),
        ];

        let first = latest_for_identity(&records, &identity("account-a")).unwrap();
        let second = latest_for_identity(&records, &identity("account-b")).unwrap();

        assert_eq!(first.quota_used_percent, Some(10.0));
        assert_eq!(second.quota_used_percent, Some(70.0));
        assert!(
            latest_for_identity(&records, &identity("account-c")).is_none(),
            "legacy aggregation must not erase explicit instance identity"
        );
    }

    /// #1339 acceptance: a backend-scoped view surfaces every source
    /// identity's latest windows — instance-scoped rows and router rows
    /// alike — instead of only legacy unscoped rows.
    #[test]
    fn latest_windows_for_backend_returns_every_identity() {
        let claude_five_hour = QuotaObservationRecord {
            backend: "claude".into(),
            backend_instance: Some("claude".into()),
            model: None,
            quota_pool: None,
            quota_window: Some("5h".into()),
            quota_used_percent: None,
            quota_remaining_percent: Some(40.0),
            quota_reset_at: None,
            observed_at: Some("2026-10-04T08:00:00Z".into()),
            checked_at: Some("2026-10-04T08:00:00Z".into()),
            check_error: None,
            usage_source: Some("claude_native".into()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        };
        let mut claude_weekly = claude_five_hour.clone();
        claude_weekly.quota_window = Some("weekly".into());
        claude_weekly.quota_remaining_percent = Some(90.0);
        let router = QuotaObservationRecord {
            backend: "agy".into(),
            backend_instance: Some("agy-primary".into()),
            model: None,
            quota_pool: Some("agy:external".into()),
            quota_window: Some("weekly".into()),
            quota_used_percent: None,
            quota_remaining_percent: Some(63.0),
            quota_reset_at: Some("in 16m44s".into()),
            observed_at: Some("2026-10-04T08:30:00Z".into()),
            checked_at: Some("2026-10-04T08:30:00Z".into()),
            check_error: None,
            usage_source: Some("cli_router".into()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        };
        let records = vec![claude_five_hour, claude_weekly, router];

        let claude = latest_windows_for_backend(&records, "claude");
        assert_eq!(claude.len(), 2, "both Claude windows, newest per window");
        assert!(claude
            .iter()
            .all(|r| r.backend_instance.as_deref() == Some("claude")));

        let agy = latest_windows_for_backend(&records, "agy");
        assert_eq!(agy.len(), 1);
        assert_eq!(agy[0].usage_source.as_deref(), Some("cli_router"));

        assert!(latest_windows_for_backend(&records, "codex").is_empty());
    }

    /// #1339: a newer failed check invalidates only its own source's
    /// windows in the backend-scoped view, never a sibling account's.
    #[test]
    fn latest_windows_for_backend_failed_check_scopes_to_its_source() {
        let good = QuotaObservationRecord {
            backend: "claude".into(),
            backend_instance: Some("claude".into()),
            model: None,
            quota_pool: None,
            quota_window: Some("5h".into()),
            quota_used_percent: None,
            quota_remaining_percent: Some(40.0),
            quota_reset_at: None,
            observed_at: Some("2026-10-04T08:00:00Z".into()),
            checked_at: Some("2026-10-04T08:00:00Z".into()),
            check_error: None,
            usage_source: Some("claude_native".into()),
            mistral_admin: None,
            account_usage: None,
            credential_id: None,
        };
        let mut failed_check = good.clone();
        failed_check.backend_instance = Some("claude-work".into());
        failed_check.quota_window = None;
        failed_check.quota_remaining_percent = None;
        failed_check.checked_at = Some("2026-10-04T09:00:00Z".into());
        failed_check.check_error = Some("auth_required: login expired".into());
        let records = vec![good, failed_check];

        let windows = latest_windows_for_backend(&records, "claude");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].backend_instance.as_deref(), Some("claude"));
        assert_eq!(windows[0].quota_remaining_percent, Some(40.0));
    }

    #[test]
    fn identity_observation_reads_legacy_unknown_without_assigning_it() {
        let records = vec![scoped_record(None, 40.0, "2026-07-20T10:00:00Z")];

        let observed = latest_for_identity(&records, &identity("account-a")).unwrap();

        assert_eq!(observed.backend_instance, None);
        assert_eq!(observed.quota_used_percent, Some(40.0));
    }

    #[test]
    fn account_level_observation_applies_to_models_on_the_same_instance() {
        let mut account = scoped_record(Some("account-a"), 55.0, "2026-07-20T10:00:00Z");
        account.model = None;
        let records = [account];

        let observed = latest_for_identity(&records, &identity("account-a")).unwrap();

        assert_eq!(observed.model, None);
        assert_eq!(observed.quota_used_percent, Some(55.0));
    }

    #[test]
    fn loading_legacy_jsonl_is_idempotent_and_keeps_instance_unknown() {
        let (_dir, path) = tmp_store();
        let legacy =
            r#"{"backend":"codex","model":null,"quota_window":"weekly","quota_used_percent":30.0}"#;
        std::fs::write(&path, format!("{legacy}\n")).unwrap();
        let original = std::fs::read(&path).unwrap();

        for _ in 0..2 {
            let records = load(&path).unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].backend_instance, None);
            assert_eq!(records[0].quota_pool, None);
        }

        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}
