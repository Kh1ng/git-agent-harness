//! Telemetry export health (Issue #230).
//!
//! Tracks the operational health of the automatic post-attempt telemetry
//! export pipeline so operators can distinguish a healthy exporter from a
//! stale or failing one without inspecting raw export files. State is
//! persisted next to the exported telemetry, guarded by a cross-process
//! exclusive file lock so concurrent terminal attempts serialize their
//! exports instead of racing the same output files.

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::records::SCHEMA_VERSION;
use crate::config::GahConfig;

/// Destination for the telemetry repo that automatic post-attempt export
/// writes into, same override convention as `GAH_LEDGER_PATH`/`GAH_EVENTS_PATH`.
pub fn export_repo_path(cfg: &GahConfig) -> PathBuf {
    if let Ok(path) = std::env::var("GAH_TELEMETRY_EXPORT_PATH") {
        return PathBuf::from(path);
    }
    let artifact_root = cfg.defaults.artifact_root.trim();
    if !artifact_root.is_empty() {
        return PathBuf::from(artifact_root).join("telemetry");
    }
    crate::config::default_config_dir().join("telemetry")
}

/// In-process attempts made per scheduled export before giving up and
/// leaving `retry_pending` set for the next scheduled export (the next
/// terminal attempt, or an explicit `gah telemetry export`) to pick up.
pub(crate) const MAX_IMMEDIATE_RETRIES: u32 = 3;

/// How long a successful export may go without a newer success before
/// `gah status` reports it as stale rather than healthy.
const STALE_THRESHOLD_SECONDS: i64 = 15 * 60;

/// Consecutive failures tolerated before reporting "failed" (retries
/// exhausted, needs operator attention) rather than "retrying" (transient,
/// still within its own retry budget).
const FAILED_AFTER_CONSECUTIVE_FAILURES: u32 = 5;

/// Operator-visible export health state, distinguishing never-run from a
/// healthy exporter, one that has gone stale, one actively retrying a
/// recent failure, and one whose retries have been exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportHealthStatus {
    NeverRun,
    Healthy,
    Stale,
    Retrying,
    Failed,
}

impl ExportHealthStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportHealthStatus::NeverRun => "never_run",
            ExportHealthStatus::Healthy => "healthy",
            ExportHealthStatus::Stale => "stale",
            ExportHealthStatus::Retrying => "retrying",
            ExportHealthStatus::Failed => "failed",
        }
    }
}

/// Durable export health record, persisted as JSON alongside the exported
/// telemetry repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportHealthState {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub last_error_class: Option<String>,
    /// Latest ledger entry timestamp reflected in a successful export --
    /// how far the authoritative ledger has been durably exported.
    pub exported_watermark: Option<String>,
    /// Cumulative count of telemetry records ever exported.
    #[serde(default)]
    pub record_count: usize,
    /// True when the most recent export attempt failed and a retry is
    /// still owed.
    #[serde(default)]
    pub retry_pending: bool,
    #[serde(default)]
    pub consecutive_failures: u32,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for ExportHealthState {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            last_attempt_at: None,
            last_success_at: None,
            last_error: None,
            last_error_class: None,
            exported_watermark: None,
            record_count: 0,
            retry_pending: false,
            consecutive_failures: 0,
        }
    }
}

impl ExportHealthState {
    /// Derive the operator-visible health status as of `now`.
    pub fn status(&self, now: OffsetDateTime) -> ExportHealthStatus {
        if self.last_attempt_at.is_none() {
            return ExportHealthStatus::NeverRun;
        }
        if self.retry_pending {
            return if self.consecutive_failures >= FAILED_AFTER_CONSECUTIVE_FAILURES {
                ExportHealthStatus::Failed
            } else {
                ExportHealthStatus::Retrying
            };
        }
        match self.last_success_at.as_deref().and_then(parse_rfc3339) {
            Some(success_at) if (now - success_at).whole_seconds() > STALE_THRESHOLD_SECONDS => {
                ExportHealthStatus::Stale
            }
            _ => ExportHealthStatus::Healthy,
        }
    }

    fn begin_attempt(&mut self, now_str: &str) {
        self.last_attempt_at = Some(now_str.to_string());
    }

    fn record_success(&mut self, now_str: &str, new_records: usize, watermark: Option<String>) {
        self.last_success_at = Some(now_str.to_string());
        self.last_error = None;
        self.last_error_class = None;
        self.retry_pending = false;
        self.consecutive_failures = 0;
        self.record_count += new_records;
        if let Some(candidate) = watermark {
            let advances = self
                .exported_watermark
                .as_deref()
                .map(|current| candidate.as_str() > current)
                .unwrap_or(true);
            if advances {
                self.exported_watermark = Some(candidate);
            }
        }
    }

    fn record_failure(&mut self, err: &anyhow::Error) {
        self.last_error = Some(format!("{err:#}"));
        self.last_error_class = Some(classify_error(err));
        self.retry_pending = true;
        self.consecutive_failures += 1;
    }
}

/// Coarse, non-sensitive error classification. Never includes secrets --
/// only the io error kind or a fixed category label.
fn classify_error(err: &anyhow::Error) -> String {
    for cause in err.chain() {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            return format!("io:{:?}", io_err.kind());
        }
        if cause.downcast_ref::<serde_json::Error>().is_some() {
            return "serialization".to_string();
        }
    }
    "export".to_string()
}

fn parse_rfc3339(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}

fn state_path(repo_path: &Path) -> PathBuf {
    repo_path.join("export_health.json")
}

fn lock_path(repo_path: &Path) -> PathBuf {
    repo_path.join("export_health.json.lock")
}

fn load(repo_path: &Path) -> Result<ExportHealthState> {
    let path = state_path(repo_path);
    if !path.exists() {
        return Ok(ExportHealthState::default());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading export health state: {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("parsing export health state: {}", path.display()))
}

fn save(repo_path: &Path, state: &ExportHealthState) -> Result<()> {
    fs::create_dir_all(repo_path)
        .with_context(|| format!("creating telemetry repo dir: {}", repo_path.display()))?;
    let path = state_path(repo_path);
    let tmp_path = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp_path)
        .with_context(|| format!("opening export health temp file: {}", tmp_path.display()))?;
    file.write_all(serde_json::to_string_pretty(state)?.as_bytes())
        .with_context(|| format!("writing export health temp file: {}", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing export health temp file: {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("replacing export health state: {}", path.display()))?;
    Ok(())
}

/// Run `update` with an exclusive cross-process lock held on the export
/// health state, serializing concurrent exports from parallel dispatch
/// workers so they cannot corrupt or redundantly race the same output.
/// Loads current state, runs `update`, then durably persists the result
/// (temp file + rename) before releasing the lock.
fn with_locked_health<T>(
    repo_path: &Path,
    update: impl FnOnce(&mut ExportHealthState) -> T,
) -> Result<T> {
    fs::create_dir_all(repo_path)
        .with_context(|| format!("creating telemetry repo dir: {}", repo_path.display()))?;
    let lock_file_path = lock_path(repo_path);
    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_file_path)
        .with_context(|| format!("opening export health lock: {}", lock_file_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("locking export health: {}", lock_file_path.display()))?;
    let mut state = load(repo_path)?;
    let result = update(&mut state);
    let saved = save(repo_path, &state);
    FileExt::unlock(&lock_file).ok();
    saved?;
    Ok(result)
}

/// Run one scheduled, idempotent export attempt (bounded immediate retries)
/// under the export health lock, updating persisted health state
/// regardless of outcome. `run_export` is expected to read from the
/// authoritative ledger and dedupe against already-exported record IDs, so
/// repeated/concurrent calls never duplicate exported records.
pub(crate) fn run_locked_export(
    repo_path: &Path,
    mut run_export: impl FnMut() -> Result<(usize, Option<String>)>,
) -> Result<()> {
    with_locked_health(repo_path, |state| {
        let now_str = now_rfc3339();
        state.begin_attempt(&now_str);
        let mut last_error = None;
        for attempt in 0..MAX_IMMEDIATE_RETRIES {
            match run_export() {
                Ok((new_records, watermark)) => {
                    state.record_success(&now_str, new_records, watermark);
                    last_error = None;
                    break;
                }
                Err(err) => {
                    last_error = Some(err);
                    if attempt + 1 < MAX_IMMEDIATE_RETRIES {
                        std::thread::sleep(std::time::Duration::from_millis(u64::from(
                            20 * (attempt + 1),
                        )));
                    }
                }
            }
        }
        if let Some(err) = last_error {
            state.record_failure(&err);
        }
    })
}

/// Operator-visible projection of export health, suitable for `gah status
/// --json`, the control-plane read API, and the dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct ExportHealthView {
    pub status: &'static str,
    pub schema_version: u32,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub last_error_class: Option<String>,
    pub exported_watermark: Option<String>,
    pub record_count: usize,
    pub retry_pending: bool,
}

impl Default for ExportHealthView {
    fn default() -> Self {
        Self::from(&ExportHealthState::default())
    }
}

impl From<&ExportHealthState> for ExportHealthView {
    fn from(state: &ExportHealthState) -> Self {
        Self {
            status: state.status(OffsetDateTime::now_utc()).as_str(),
            schema_version: state.schema_version,
            last_attempt_at: state.last_attempt_at.clone(),
            last_success_at: state.last_success_at.clone(),
            last_error: state.last_error.clone(),
            last_error_class: state.last_error_class.clone(),
            exported_watermark: state.exported_watermark.clone(),
            record_count: state.record_count,
            retry_pending: state.retry_pending,
        }
    }
}

/// Read the current export health for display (e.g. `gah status --json`).
/// Never fails: a missing or unreadable state file reads as never-run
/// rather than surfacing an internal error on an unrelated status command.
pub fn read_view(repo_path: &Path) -> ExportHealthView {
    match load(repo_path) {
        Ok(state) => ExportHealthView::from(&state),
        Err(err) => {
            log::debug!("export health state unavailable: {err:#}");
            ExportHealthView::from(&ExportHealthState::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_run_before_first_attempt() {
        let state = ExportHealthState::default();
        assert_eq!(
            state.status(OffsetDateTime::now_utc()),
            ExportHealthStatus::NeverRun
        );
    }

    #[test]
    fn healthy_immediately_after_success() {
        let mut state = ExportHealthState::default();
        let now = now_rfc3339();
        state.begin_attempt(&now);
        state.record_success(&now, 3, Some("2026-01-01T00:00:00Z".to_string()));
        assert_eq!(
            state.status(OffsetDateTime::now_utc()),
            ExportHealthStatus::Healthy
        );
        assert_eq!(state.record_count, 3);
        assert_eq!(
            state.exported_watermark.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert!(!state.retry_pending);
    }

    #[test]
    fn stale_when_last_success_old() {
        let mut state = ExportHealthState::default();
        let old = (OffsetDateTime::now_utc() - time::Duration::minutes(30))
            .format(&Rfc3339)
            .unwrap();
        state.begin_attempt(&old);
        state.record_success(&old, 1, None);
        assert_eq!(
            state.status(OffsetDateTime::now_utc()),
            ExportHealthStatus::Stale
        );
    }

    #[test]
    fn retrying_then_failed_after_repeated_failures() {
        let mut state = ExportHealthState::default();
        let now = now_rfc3339();
        state.begin_attempt(&now);
        state.record_failure(&anyhow::anyhow!("boom"));
        assert_eq!(
            state.status(OffsetDateTime::now_utc()),
            ExportHealthStatus::Retrying
        );
        for _ in 0..FAILED_AFTER_CONSECUTIVE_FAILURES {
            state.record_failure(&anyhow::anyhow!("boom again"));
        }
        assert_eq!(
            state.status(OffsetDateTime::now_utc()),
            ExportHealthStatus::Failed
        );
    }

    #[test]
    fn watermark_never_regresses() {
        let mut state = ExportHealthState::default();
        let now = now_rfc3339();
        state.record_success(&now, 1, Some("2026-02-01T00:00:00Z".to_string()));
        state.record_success(&now, 1, Some("2026-01-01T00:00:00Z".to_string()));
        assert_eq!(
            state.exported_watermark.as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
    }

    #[test]
    fn run_locked_export_persists_success_across_reloads() {
        let dir = tempfile::tempdir().unwrap();
        run_locked_export(dir.path(), || {
            Ok((2, Some("2026-01-01T00:00:00Z".to_string())))
        })
        .unwrap();
        let view = read_view(dir.path());
        assert_eq!(view.status, "healthy");
        assert_eq!(view.record_count, 2);
        assert!(!view.retry_pending);
    }

    #[test]
    fn run_locked_export_marks_failure_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        run_locked_export(dir.path(), || Err(anyhow::anyhow!("forced"))).unwrap();
        let view = read_view(dir.path());
        assert_eq!(view.status, "retrying");
        assert!(view.retry_pending);
        assert_eq!(view.last_error_class.as_deref(), Some("export"));
    }
}
