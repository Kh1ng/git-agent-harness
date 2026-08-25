//! #166 (within #151): durable store for *account-level* quota observations.
//!
//! Per-attempt usage (tokens, per-attempt cost) is already recorded in the
//! ledger via `usage.rs`'s structured parsers. This module holds the separate
//! *account-level* quota picture that only surfaces through a backend's own
//! status/quota endpoint — e.g. `codex status --json` — which is not part
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaObservationRecord {
    pub backend: String,
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
    pub usage_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mistral_admin: Option<MistralAdminObservationRecord>,
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

/// Load all records. A missing file is an empty list; any read/parse error
/// is swallowed to an empty list so callers can always safely enrich a report
/// without a quota store present (e.g. in hermetic tests).
pub fn load(state_path: &Path) -> Result<Vec<QuotaObservationRecord>> {
    if !state_path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(state_path).context("read quota store")?;
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
            let normalized = crate::execution_identity::validate_secret_safe_label(field, value)?;
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

/// Most-recent observation for a (backend, model) scope that actually carries
/// quota data (used/remaining percent, window, or reset). `model` of `None`
/// matches account-level records that have no model qualifier.
pub fn latest_for<'a>(
    records: &'a [QuotaObservationRecord],
    backend: &str,
    model: Option<&'a str>,
) -> Option<&'a QuotaObservationRecord> {
    records
        .iter()
        .filter(|r| {
            r.backend == backend
                && r.model.as_deref() == model
                && r.backend_instance.is_none()
                && r.quota_pool.is_none()
                && has_quota_data(r)
        })
        .max_by(|a, b| a.observed_at.cmp(&b.observed_at))
}

/// Most-recent observation for an exact execution identity, with a
/// deterministic fallback to legacy instance-unknown rows. A row for a
/// different explicit instance never matches, even when backend/model agree.
pub fn latest_for_identity<'a>(
    records: &'a [QuotaObservationRecord],
    identity: &crate::execution_identity::ExecutionIdentity,
) -> Option<&'a QuotaObservationRecord> {
    records
        .iter()
        .filter(|record| {
            record.backend == identity.logical_backend
                && (record.model.is_none() || record.model == identity.effective_model)
                && record
                    .backend_instance
                    .as_deref()
                    .is_none_or(|instance| instance == identity.backend_instance)
                && record
                    .quota_pool
                    .as_deref()
                    .is_none_or(|pool| Some(pool) == identity.quota_pool.as_deref())
                && has_quota_data(record)
        })
        .max_by(|left, right| left.observed_at.cmp(&right.observed_at))
}

fn has_quota_data(record: &QuotaObservationRecord) -> bool {
    record.quota_used_percent.is_some()
        || record.quota_remaining_percent.is_some()
        || record.quota_window.is_some()
        || record.quota_reset_at.is_some()
}

/// #166: run `codex status --json`, parse its account-level quota, and append
/// the result to the durable store. Returns the stored record when Codex
/// reported quota data, `Ok(None)` when it reported nothing (or `codex` is
/// unavailable), and an error only when the subprocess itself failed to spawn.
pub fn refresh_codex_and_store(
    codex_cmd: &str,
    model: Option<&str>,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let obs = crate::usage::refresh_codex_quota(codex_cmd, model)
        .map_err(|e| anyhow::anyhow!("`codex status --json` failed: {e}"))?;
    match obs {
        Some(obs) => {
            let rec = QuotaObservationRecord {
                backend: obs.backend.clone(),
                backend_instance: None,
                model: obs.model.clone(),
                quota_pool: None,
                quota_window: obs.quota_window.clone(),
                quota_used_percent: obs.quota_used_percent,
                quota_remaining_percent: obs.quota_remaining_percent,
                quota_reset_at: obs.quota_reset_at.clone(),
                observed_at: obs.observed_at.clone(),
                usage_source: obs.usage_source.clone(),
                mistral_admin: None,
            };
            append(state_path, &rec)?;
            Ok(Some(rec))
        }
        None => Ok(None),
    }
}

/// #154: refresh account-level Mistral Admin API data (aggregate usage,
/// billing, rate-limit ceilings, spend-limit percent) and persist the
/// spend-limit reading -- the only piece of that refresh that collapses
/// into this store's (backend, model) -> quota-percent shape; aggregate
/// token/billing figures have no durable sink of their own yet. Returns
/// `Ok(None)` only when `MISTRAL_ADMIN_API_KEY` is unset or the Admin API
/// yielded no persisted observation at all; partial aggregate data still
/// gets stored and returned as `Some(...)` so callers can distinguish
/// "nothing recorded" from "no spend-limit reading, but other admin data was
/// captured."
pub fn refresh_vibe_admin_and_store(
    model: Option<&str>,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let Some(api_key) = crate::usage::admin_api_key() else {
        return Ok(None);
    };
    let end_time = time::OffsetDateTime::now_utc().unix_timestamp();
    let thirty_days_secs = 30 * 24 * 60 * 60;
    let refresh = crate::usage::refresh_admin_data(
        &api_key,
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
                usage_source: Some("mistral_admin_refresh".to_string()),
                mistral_admin: Some(admin_refresh),
            };
            append(state_path, &rec)?;
            return Ok(Some(rec));
        }
        return Ok(None);
    };
    let rec = QuotaObservationRecord {
        backend: obs.backend,
        backend_instance: None,
        model: obs.model,
        quota_pool: None,
        quota_window: obs.quota_window,
        quota_used_percent: obs.quota_used_percent,
        quota_remaining_percent: obs.quota_remaining_percent,
        quota_reset_at: obs.quota_reset_at,
        observed_at: obs.observed_at,
        usage_source: obs.usage_source,
        mistral_admin: admin_refresh,
    };
    append(state_path, &rec)?;
    Ok(Some(rec))
}

/// Refresh and persist account quota for one explicit execution identity.
pub fn refresh_codex_and_store_for_identity(
    codex_cmd: &str,
    identity: &crate::execution_identity::ExecutionIdentity,
    state_path: &Path,
) -> Result<Option<QuotaObservationRecord>> {
    let observation =
        crate::usage::refresh_codex_quota(codex_cmd, identity.effective_model.as_deref())
            .map_err(|error| anyhow::anyhow!("`codex status --json` failed: {error}"))?;
    let Some(observation) = observation else {
        return Ok(None);
    };
    let record = QuotaObservationRecord {
        backend: identity.logical_backend.clone(),
        backend_instance: Some(identity.backend_instance.clone()),
        model: identity.effective_model.clone(),
        quota_pool: identity.quota_pool.clone(),
        quota_window: observation.quota_window,
        quota_used_percent: observation.quota_used_percent,
        quota_remaining_percent: observation.quota_remaining_percent,
        quota_reset_at: observation.quota_reset_at,
        observed_at: observation.observed_at,
        usage_source: observation.usage_source,
        mistral_admin: None,
    };
    append(state_path, &record)?;
    Ok(Some(record))
}

/// Issue #761: nothing refreshed this store periodically -- only a human
/// running `gah quota refresh` by hand did, so account-level quota data
/// went stale for days even while dispatch itself was active. Called once
/// per `gah loop` tick (see `controller::runtime::run_once`), throttled to
/// `QUOTA_REFRESH_INTERVAL_SECONDS` per backend so this can't hammer either
/// endpoint: codex's own `codex status --json` is a local CLI call, but
/// vibe's is a real network call against the Mistral Admin API with its own
/// rate limits. Best-effort: a refresh failure (backend not installed, no
/// `MISTRAL_ADMIN_API_KEY`) must not fail the loop tick, so errors are
/// swallowed here, not propagated.
///
/// Returns the spawned refresh threads (one per backend that was actually
/// due and not already in flight) so a caller that must outlive them -- the
/// dedicated `gah quota auto-refresh` oneshot CLI -- can join them; the
/// fire-and-forget loop-tick caller simply drops the handles.
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
    if let Some(handle) = maybe_refresh_backend(store_path, "codex", now, {
        let codex_cmd = codex_cmd.clone();
        let path = store_path.to_path_buf();
        move || refresh_codex_and_store(&codex_cmd, None, &path)
    }) {
        handles.push(handle);
    }
    if let Some(handle) = maybe_refresh_backend(store_path, "vibe", now, {
        let path = store_path.to_path_buf();
        move || refresh_vibe_admin_and_store(None, &path)
    }) {
        handles.push(handle);
    }
    handles
}

/// Issue #761: blocking variant for the dedicated `gah quota auto-refresh`
/// CLI command, which a systemd oneshot timer runs. Spawns the per-backend
/// refresh threads (same throttle/supervision as
/// `refresh_stale_quota_observations`) and JOINS them, so the oneshot
/// process does not exit and kill the threads mid-refresh. Each backend
/// refresh is independently bounded (codex status timeout / curl
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

/// How long a stale reading is trusted before another live check is worth
/// the API/process cost. Conservative on purpose -- see the module-level
/// doc comment on `refresh_stale_quota_observations`.
const QUOTA_REFRESH_INTERVAL_SECONDS: i64 = 30 * 60;

/// `refresh_codex_and_store`/`refresh_vibe_admin_and_store` shell out to a
/// real CLI/HTTP endpoint with zero supervision of their own (unlike every
/// dispatch-path backend invocation, which goes through
/// `runner::process::spawn_with_idle_watch` for exactly this reason -- see
/// #87/#170). That was an acceptable risk for the previous, sole caller (a
/// human running `gah quota refresh` who can just Ctrl-C a hang); it is
/// not acceptable on this function's *new* caller, the automatic,
/// unattended `gah loop` tick -- confirmed live, an isolated-HOME test
/// subprocess made `codex status --json` hang (this codebase's
/// idle-watch precedent exists for exactly this failure class). A bounded
/// `recv_timeout` on the calling thread was tried first and rejected: the
/// controller tick still has to wait out the full timeout before giving
/// up, which is just as damaging to tick latency as the hang itself.
/// Instead this is genuinely fire-and-forget -- the refresh runs on a
/// detached thread the tick never waits on, and `IN_FLIGHT` below is the
/// only thing standing between a persistently-hanging backend and an
/// unbounded pile-up of abandoned threads across ticks (at most one
/// outstanding attempt per backend per process, however long it takes to
/// finally return). Fixed here rather than by adding real idle-watch
/// supervision to `usage.rs`'s refresh functions, since those are shared
/// with the manual CLI command and changing their blocking behavior there
/// is a separate, larger question this fix doesn't need to answer.
static IN_FLIGHT: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

fn maybe_refresh_backend(
    path: &Path,
    backend: &str,
    now: OffsetDateTime,
    refresh: impl FnOnce() -> Result<Option<QuotaObservationRecord>> + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    let records = load(path).unwrap_or_default();
    let last_checked = records
        .iter()
        .filter(|record| record.backend == backend)
        .filter_map(|record| record.observed_at.as_deref())
        .filter_map(|observed_at| OffsetDateTime::parse(observed_at, &Rfc3339).ok())
        .max();
    let due = match last_checked {
        None => true,
        Some(last) => now - last > time::Duration::seconds(QUOTA_REFRESH_INTERVAL_SECONDS),
    };
    if !due {
        return None;
    }
    {
        let mut in_flight = IN_FLIGHT
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let set = in_flight.get_or_insert_with(std::collections::HashSet::new);
        if !set.insert(backend.to_string()) {
            // A previous tick's refresh for this exact backend hasn't
            // returned yet -- don't pile on a second attempt on top of it.
            return None;
        }
    }
    let path = path.to_path_buf();
    let backend = backend.to_string();
    Some(std::thread::spawn(move || {
        // `refresh_*_and_store` already appends a real record when it
        // finds data (Ok(Some(_))); on Ok(None)/Err, append a data-free
        // marker purely so the next tick's `last_checked` sees a fresh
        // attempt and stops retrying every tick against a backend that
        // isn't configured here at all.
        if !matches!(refresh(), Ok(Some(_))) {
            let marker = QuotaObservationRecord {
                backend: backend.clone(),
                backend_instance: None,
                model: None,
                quota_pool: None,
                quota_window: None,
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: None,
                observed_at: now.format(&Rfc3339).ok(),
                usage_source: None,
                mistral_admin: None,
            };
            let _ = append(&path, &marker);
        }
        if let Ok(mut in_flight) = IN_FLIGHT.lock() {
            if let Some(set) = in_flight.as_mut() {
                set.remove(&backend);
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

        // Ten minutes later, well inside QUOTA_REFRESH_INTERVAL_SECONDS.
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
        });
        assert!(wait_for_backend_record(&path, "test-refresh-again"));

        let second = calls.clone();
        maybe_refresh_backend(
            &path,
            "test-refresh-again",
            now + time::Duration::seconds(QUOTA_REFRESH_INTERVAL_SECONDS + 60),
            move || {
                second.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(None)
            },
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while calls.load(std::sync::atomic::Ordering::SeqCst) < 2
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
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
    fn maybe_refresh_backend_does_not_throttle_a_different_backend() {
        let (_dir, path) = tmp_store();
        let now = OffsetDateTime::now_utc();

        maybe_refresh_backend(&path, "test-independent-codex", now, || Ok(None));
        maybe_refresh_backend(&path, "test-independent-vibe", now, || Ok(None));

        assert!(wait_for_backend_record(&path, "test-independent-codex"));
        assert!(wait_for_backend_record(&path, "test-independent-vibe"));
    }

    #[test]
    fn refresh_vibe_admin_and_store_returns_none_without_api_key() {
        let _key_guard = crate::test_support::MistralAdminKeyEnvGuard::unset();
        let (_dir, path) = tmp_store();

        assert!(refresh_vibe_admin_and_store(None, &path).unwrap().is_none());
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
                "#!/bin/sh\ncfg=$(cat)\ncase \"$cfg\" in\n  *analytics/vibe/usage/by_workspace*) cat '{fixtures}/vibe_workspace_usage.json' ;;\n  *api/admin/usage*) cat '{fixtures}/usage.json' ;;\n  *api/admin/rate-limit*) cat '{rate_limit}' ;;\n  *api/admin/spend-limit*) cat '{spend_limit}' ;;\n  *) exit 1 ;;\nesac\n",
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
                "#!/bin/sh\ncfg=$(cat)\ncase \"$cfg\" in\n  *analytics/vibe/usage/by_workspace*) cat '{fixtures}/vibe_workspace_usage.json' ;;\n  *api/admin/usage*) cat '{fixtures}/usage.json' ;;\n  *api/admin/rate-limit*) cat '{rate_limit}' ;;\n  *api/admin/spend-limit*) cat '{spend_limit}' ;;\n  *) exit 1 ;;\nesac\n",
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
                usage_source: Some("codex_status_json".into()),
                mistral_admin: None,
            },
        )
        .unwrap();

        let records = load(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].backend, "codex");
        assert_eq!(records[0].quota_used_percent, Some(25.0));
        assert_eq!(records[0].quota_remaining_percent, Some(75.0));
    }

    #[test]
    fn latest_for_picks_newest_and_scopes_by_model() {
        let (_dir, path) = tmp_store();
        for (ts, pct) in [
            ("2026-04-28T10:00:00Z", 10.0),
            ("2026-04-29T10:00:00Z", 40.0),
        ] {
            append(
                &path,
                &QuotaObservationRecord {
                    backend: "codex".into(),
                    backend_instance: None,
                    model: None,
                    quota_pool: None,
                    quota_window: Some("300m".into()),
                    quota_used_percent: Some(pct),
                    quota_remaining_percent: Some(100.0 - pct),
                    quota_reset_at: None,
                    observed_at: Some(ts.into()),
                    usage_source: Some("codex_status_json".into()),
                    mistral_admin: None,
                },
            )
            .unwrap();
        }
        // A different backend must not be selected.
        append(
            &path,
            &QuotaObservationRecord {
                backend: "agy".into(),
                backend_instance: None,
                model: None,
                quota_pool: None,
                quota_window: Some("AGY individual quota".into()),
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: Some("in 16m44s".into()),
                observed_at: Some("2026-04-29T11:00:00Z".into()),
                usage_source: Some("agy_cli_log_delta".into()),
                mistral_admin: None,
            },
        )
        .unwrap();

        let records = load(&path).unwrap();
        let latest = latest_for(&records, "codex", None).unwrap();
        assert_eq!(latest.observed_at.as_deref(), Some("2026-04-29T10:00:00Z"));
        assert_eq!(latest.quota_used_percent, Some(40.0));
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
            usage_source: Some("codex_status_json".into()),
            mistral_admin: None,
        };
        let good2 = QuotaObservationRecord {
            quota_used_percent: Some(20.0),
            observed_at: Some("2026-04-29T10:00:00Z".into()),
            mistral_admin: None,
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

    #[test]
    fn latest_for_ignores_data_less_records() {
        let (_dir, path) = tmp_store();
        append(
            &path,
            &QuotaObservationRecord {
                backend: "codex".into(),
                backend_instance: None,
                model: None,
                quota_pool: None,
                quota_window: None,
                quota_used_percent: None,
                quota_remaining_percent: None,
                quota_reset_at: None,
                observed_at: Some("2026-04-29T10:00:00Z".into()),
                usage_source: Some("codex_status_json".into()),
                mistral_admin: None,
            },
        )
        .unwrap();
        assert!(latest_for(&load(&path).unwrap(), "codex", None).is_none());
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
            usage_source: Some("test".into()),
            mistral_admin: None,
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
            latest_for(&records, "opencode", Some("shared-model")).is_none(),
            "legacy aggregation must not erase explicit instance identity"
        );
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
