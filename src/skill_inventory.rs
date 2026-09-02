//! Durable per-backend-instance skill inventory (issue #966, #863 gap 2).
//!
//! `#863` states plainly: "GAH currently has zero notion of 'which skills
//! does this backend instance have'." This module closes that gap with one
//! uniform call (`refresh_and_store`, via `runner::backend_runner::for_kind`
//! -- never a per-backend match at the call site) that asks a backend
//! instance to self-report its skills, bounded by a timeout so a hung
//! backend can never block a caller.
//!
//! Storage mirrors `quota_store.rs`'s design philosophy: append-only JSONL,
//! not an in-place keyed map, so concurrent GAH processes can never erase
//! each other's writes. "Current state" for a (backend, instance) scope is
//! the latest record in that scope; a missing file is a clean empty state.
//!
//! A backend that cannot self-report (the `BackendRunner::observe_skills`
//! default) is stored as `observed_skill_ids: None` -- distinct from
//! `Some(vec![])`, which means the backend was actually asked and confirmed
//! zero skills. Reading `SkillInventoryView::drift` off a `None` observation
//! deliberately returns `None` too: reconciliation is only meaningful once
//! GAH has actually heard back from the backend.

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::backend_kind::BackendKind;
use crate::runner::backend_runner::{self, ObservedSkills, SkillObservationContext};

/// How long a self-report query is allowed to run before the caller treats
/// it as unknown. Never block a dispatch decision on a hung backend
/// (#966 AC6).
pub const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Past this age a stored observation is stale: real, but no longer
/// trustworthy as "current" -- the same discipline #741 requires for cached
/// quota data, so a dashboard reader must not present it as live truth.
pub const STALE_AFTER_SECONDS: i64 = 15 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInventoryRecord {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    /// `None` means the backend could not self-report (unknown), never an
    /// empty list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_skill_ids: Option<Vec<String>>,
    pub observed_at: String,
}

/// Global, not per-profile (like `availability.rs`/`quota_store.rs`): a
/// backend instance's self-reported skills don't depend on which repo GAH
/// happens to be working on. `GAH_SKILL_INVENTORY_STORE_PATH` is an explicit
/// override, matching the existing `GAH_AVAILABILITY_PATH`/
/// `GAH_QUOTA_STORE_PATH` convention.
pub fn store_path() -> PathBuf {
    if let Ok(path) = std::env::var("GAH_SKILL_INVENTORY_STORE_PATH") {
        return PathBuf::from(path);
    }
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        Path::new(&dir).join("gah").join("skill_inventory.jsonl")
    } else {
        Path::new(&std::env::var("HOME").unwrap_or_default())
            .join(".local")
            .join("state")
            .join("gah")
            .join("skill_inventory.jsonl")
    }
}

/// Load all records. A missing file is an empty list; a corrupt line is
/// skipped rather than discarding every valid record around it (mirrors
/// `quota_store.rs`/`availability.rs`'s resilience to a partial write).
pub fn load(state_path: &Path) -> Result<Vec<SkillInventoryRecord>> {
    if !state_path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(state_path).context("read skill inventory store")?;
    let mut records = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(rec) = serde_json::from_str::<SkillInventoryRecord>(line) {
            records.push(rec);
        }
    }
    Ok(records)
}

/// Append one record under an exclusive lock. Missing parent dirs are
/// created.
pub fn append(state_path: &Path, rec: &SkillInventoryRecord) -> Result<()> {
    if let Some(parent) = state_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_path)
        .context("open skill inventory store")?;
    file.lock_exclusive()
        .with_context(|| format!("locking {}", state_path.display()))?;
    let line = serde_json::to_string(rec).context("serialize skill inventory observation")?;
    writeln!(file, "{line}").context("write skill inventory observation")?;
    let _ = file.unlock();
    Ok(())
}

/// Most-recent observation for a (backend, instance) scope.
pub fn latest_for<'a>(
    records: &'a [SkillInventoryRecord],
    backend: &str,
    instance: Option<&str>,
) -> Option<&'a SkillInventoryRecord> {
    records
        .iter()
        .filter(|r| r.backend == backend && r.backend_instance.as_deref() == instance)
        .max_by(|a, b| a.observed_at.cmp(&b.observed_at))
}

/// Query a backend instance for its skills -- one uniform call across every
/// `BackendKind` via `backend_runner::for_kind` (#966 AC1) -- and persist the
/// result. Bounded by `OBSERVATION_TIMEOUT`; a backend with no self-report
/// support, or one that fails/times out, is stored as `Unknown`
/// (`observed_skill_ids: None`), never an empty list (#966 AC2).
pub fn refresh_and_store(
    kind: BackendKind,
    instance: Option<&str>,
    executable: &Path,
    state_path: &Path,
    now: OffsetDateTime,
) -> Result<SkillInventoryRecord> {
    let runner = backend_runner::for_kind(kind);
    let observed = runner.observe_skills(&SkillObservationContext {
        executable,
        timeout: OBSERVATION_TIMEOUT,
    });
    let record = SkillInventoryRecord {
        backend: kind.as_str().to_string(),
        backend_instance: instance.map(str::to_string),
        observed_skill_ids: match observed {
            ObservedSkills::Unknown => None,
            ObservedSkills::Skills(ids) => Some(ids),
        },
        observed_at: now
            .format(&Rfc3339)
            .context("formatting skill observation timestamp")?,
    };
    append(state_path, &record)?;
    Ok(record)
}

/// Reconciliation between what GAH intends bound and what a backend
/// self-reported.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillDrift {
    /// Bound in GAH but missing from the backend's self-reported set
    /// (#966 AC4).
    pub bound_not_observed: Vec<String>,
    /// Present on the backend but not bound in GAH -- the #863 hand-edit
    /// scenario (#966 AC3).
    pub observed_not_bound: Vec<String>,
}

impl SkillDrift {
    pub fn is_empty(&self) -> bool {
        self.bound_not_observed.is_empty() && self.observed_not_bound.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillInventoryView {
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_instance: Option<String>,
    pub bound_skill_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_skill_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation_age_seconds: Option<i64>,
    /// `None` when there is no observation yet to judge freshness of.
    /// `Some(true)` means the dashboard must not present this observation as
    /// live truth (#966 AC5 / #741 discipline).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation_stale: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift: Option<SkillDrift>,
}

/// Build the reportable view for one backend instance from its bound
/// resolution and its latest stored observation (if any).
pub fn view(
    bound_skill_ids: Vec<String>,
    record: Option<&SkillInventoryRecord>,
    backend: &str,
    instance: Option<&str>,
    now: OffsetDateTime,
) -> SkillInventoryView {
    let observed_skill_ids = record.and_then(|r| r.observed_skill_ids.clone());
    let observed_at = record.map(|r| r.observed_at.clone());
    let observation_age_seconds = observed_at.as_deref().and_then(|ts| {
        OffsetDateTime::parse(ts, &Rfc3339)
            .ok()
            .map(|parsed| (now - parsed).whole_seconds())
    });
    let observation_stale = observation_age_seconds.map(|age| age >= STALE_AFTER_SECONDS);
    let drift = observed_skill_ids.as_ref().map(|observed| SkillDrift {
        bound_not_observed: bound_skill_ids
            .iter()
            .filter(|id| !observed.contains(id))
            .cloned()
            .collect(),
        observed_not_bound: observed
            .iter()
            .filter(|id| !bound_skill_ids.contains(id))
            .cloned()
            .collect(),
    });
    SkillInventoryView {
        backend: backend.to_string(),
        backend_instance: instance.map(str::to_string),
        bound_skill_ids,
        observed_skill_ids,
        observed_at,
        observation_age_seconds,
        observation_stale,
        drift,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        backend: &str,
        instance: Option<&str>,
        observed_skill_ids: Option<Vec<&str>>,
        observed_at: &str,
    ) -> SkillInventoryRecord {
        SkillInventoryRecord {
            backend: backend.to_string(),
            backend_instance: instance.map(str::to_string),
            observed_skill_ids: observed_skill_ids
                .map(|ids| ids.into_iter().map(str::to_string).collect()),
            observed_at: observed_at.to_string(),
        }
    }

    #[test]
    fn append_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skill_inventory.jsonl");
        let rec = record(
            "hermes",
            Some("gah-manager"),
            Some(vec!["review"]),
            "2026-08-01T00:00:00Z",
        );

        append(&path, &rec).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].backend, "hermes");
        assert_eq!(loaded[0].observed_skill_ids, Some(vec!["review".into()]));
    }

    #[test]
    fn missing_store_file_is_a_clean_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.jsonl");

        assert_eq!(load(&path).unwrap().len(), 0);
    }

    #[test]
    fn a_corrupt_line_is_skipped_not_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skill_inventory.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\nnot valid json\n{}\n",
                serde_json::to_string(&record("hermes", None, Some(vec!["a"]), "t1")).unwrap(),
                serde_json::to_string(&record("hermes", None, Some(vec!["b"]), "t2")).unwrap(),
            ),
        )
        .unwrap();

        let loaded = load(&path).unwrap();

        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn latest_for_picks_the_most_recent_record_in_scope() {
        let records = vec![
            record(
                "hermes",
                Some("a"),
                Some(vec!["old"]),
                "2026-01-01T00:00:00Z",
            ),
            record(
                "hermes",
                Some("a"),
                Some(vec!["new"]),
                "2026-06-01T00:00:00Z",
            ),
            record(
                "hermes",
                Some("b"),
                Some(vec!["other"]),
                "2026-06-02T00:00:00Z",
            ),
        ];

        let latest = latest_for(&records, "hermes", Some("a")).unwrap();

        assert_eq!(latest.observed_skill_ids, Some(vec!["new".into()]));
    }

    #[test]
    fn latest_for_never_crosses_instance_scope() {
        let records = vec![record(
            "hermes",
            Some("gah-manager"),
            Some(vec!["review"]),
            "2026-06-01T00:00:00Z",
        )];

        assert!(latest_for(&records, "hermes", Some("other-instance")).is_none());
        assert!(latest_for(&records, "hermes", None).is_none());
    }

    #[test]
    fn refresh_and_store_records_unknown_for_a_backend_with_no_self_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skill_inventory.jsonl");
        let now = OffsetDateTime::parse("2026-08-01T00:00:00Z", &Rfc3339).unwrap();

        // Codex's BackendRunner uses the trait default -- Unknown -- since
        // no self-report implementation is wired up for it.
        let rec =
            refresh_and_store(BackendKind::Codex, None, Path::new("codex"), &path, now).unwrap();

        assert_eq!(rec.observed_skill_ids, None, "unknown must not become []");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].observed_skill_ids, None);
    }

    #[test]
    fn view_without_any_observation_has_no_drift_no_age_no_staleness() {
        let now = OffsetDateTime::parse("2026-08-01T00:00:00Z", &Rfc3339).unwrap();

        let view = view(
            vec!["review".into()],
            None,
            "hermes",
            Some("gah-manager"),
            now,
        );

        assert_eq!(view.observed_skill_ids, None);
        assert_eq!(view.observed_at, None);
        assert_eq!(view.observation_age_seconds, None);
        assert_eq!(view.observation_stale, None);
        assert!(view.drift.is_none());
    }

    #[test]
    fn view_detects_bound_but_not_observed_drift() {
        let now = OffsetDateTime::parse("2026-08-01T00:00:05Z", &Rfc3339).unwrap();
        let rec = record(
            "hermes",
            Some("gah-manager"),
            Some(vec![]),
            "2026-08-01T00:00:00Z",
        );

        let view = view(
            vec!["review".into()],
            Some(&rec),
            "hermes",
            Some("gah-manager"),
            now,
        );

        let drift = view.drift.unwrap();
        assert_eq!(drift.bound_not_observed, vec!["review".to_string()]);
        assert!(drift.observed_not_bound.is_empty());
    }

    #[test]
    fn view_detects_observed_but_not_bound_drift_the_863_hand_edit_scenario() {
        let now = OffsetDateTime::parse("2026-08-01T00:00:05Z", &Rfc3339).unwrap();
        // The gah-manager Hermes profile's skills were hand-edited outside
        // GAH -- the backend now reports a skill GAH never bound.
        let rec = record(
            "hermes",
            Some("gah-manager"),
            Some(vec!["hand-added-skill"]),
            "2026-08-01T00:00:00Z",
        );

        let view = view(vec![], Some(&rec), "hermes", Some("gah-manager"), now);

        let drift = view.drift.unwrap();
        assert!(drift.bound_not_observed.is_empty());
        assert_eq!(
            drift.observed_not_bound,
            vec!["hand-added-skill".to_string()]
        );
        assert!(!drift.is_empty());
    }

    #[test]
    fn view_matching_bound_and_observed_has_empty_drift() {
        let now = OffsetDateTime::parse("2026-08-01T00:00:05Z", &Rfc3339).unwrap();
        let rec = record("hermes", None, Some(vec!["review"]), "2026-08-01T00:00:00Z");

        let view = view(vec!["review".into()], Some(&rec), "hermes", None, now);

        assert!(view.drift.unwrap().is_empty());
    }

    #[test]
    fn observation_age_and_staleness_are_computed_from_now() {
        let rec = record("hermes", None, Some(vec![]), "2026-08-01T00:00:00Z");

        let fresh_now = OffsetDateTime::parse("2026-08-01T00:05:00Z", &Rfc3339).unwrap();
        let fresh = view(vec![], Some(&rec), "hermes", None, fresh_now);
        assert_eq!(fresh.observation_age_seconds, Some(300));
        assert_eq!(fresh.observation_stale, Some(false));

        let stale_now = OffsetDateTime::parse("2026-08-01T00:20:00Z", &Rfc3339).unwrap();
        let stale = view(vec![], Some(&rec), "hermes", None, stale_now);
        assert_eq!(stale.observation_age_seconds, Some(1200));
        assert_eq!(stale.observation_stale, Some(true));
    }
}
