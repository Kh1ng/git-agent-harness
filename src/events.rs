//! TICKET-083: append-only controller event stream. Same shape/conventions
//! as `ledger::reconcile` (TICKET-072) -- a separate JSONL file, never
//! rewritten, only ever appended to.

use crate::config::GahConfig;
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Plain lowercase string on the wire (not a serde-tagged enum), same
/// reasoning as `FailureClass`/`FailureStage`: the wire format must survive
/// internal renames of these variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // unwired variants are the schema for future tickets, not unused code
pub enum EventType {
    ObservationCompleted,
    ContextBuilt,
    ActionDecided,
    ActionOverridden,
    DispatchStarted,
    DispatchFinished,
    /// Not yet emitted: would require `dispatch.rs`'s own
    /// `mark_backend_unavailable_from_output` to call into `events::record`,
    /// a separate change from `gah loop --once`'s own decision/execution
    /// loop (availability persistence already has its own mechanism via
    /// `availability::record_unavailable`).
    BackendMarkedUnavailable,
    WaitSelected,
    HumanRequired,
    /// A review was deliberately not launched because its ticket exhausted a
    /// configured review-cycle or paid-review budget. This is terminal for
    /// the controller run (not an in-flight dispatch) and is intentionally
    /// distinct from a backend failure.
    ReviewBudgetExhausted,
    DuplicateGuardTriggered,
    LoopStopped,
    /// TICKET-282: a work item was deliberately NOT dispatched because it would
    /// reuse a branch already attached to another worktree. This is a
    /// non-terminal, per-item deferral: the loop records it and continues with
    /// another eligible item rather than stalling on a hard `git worktree add`
    /// failure. Distinct from `LoopStopped` so dashboards/operators can tell a
    /// transient branch conflict apart from a genuine profile stall.
    WorkDeferred,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ObservationCompleted => "observation_completed",
            Self::ContextBuilt => "context_built",
            Self::ActionDecided => "action_decided",
            Self::ActionOverridden => "action_overridden",
            Self::DispatchStarted => "dispatch_started",
            Self::DispatchFinished => "dispatch_finished",
            Self::BackendMarkedUnavailable => "backend_marked_unavailable",
            Self::WaitSelected => "wait_selected",
            Self::HumanRequired => "human_required",
            Self::ReviewBudgetExhausted => "review_budget_exhausted",
            Self::DuplicateGuardTriggered => "duplicate_guard_triggered",
            Self::LoopStopped => "loop_stopped",
            Self::WorkDeferred => "work_deferred",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ControllerEvent {
    pub timestamp: String,
    pub event_type: String,
    /// Same disambiguation role as `LedgerEntry.profile` -- the event
    /// stream is one shared file across all profiles in a config.
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub work_id: Option<String>,
    /// Stable identity for one controller-launched dispatch. Older events
    /// omit this field and remain readable.
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub details: String,
    #[serde(default)]
    pub reason_code: Option<String>,
}

pub fn append(cfg: &GahConfig, event: &ControllerEvent) -> Result<()> {
    let path = cfg.defaults.events_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating events directory {}", parent.display()))?;
    }
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening events lock {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("locking events {}", path.display()))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening events log {}", path.display()))?;
    let mut value = serde_json::to_value(event).context("serializing controller event")?;
    crate::redact::redact_json_value(&mut value);
    let mut line = serde_json::to_vec(&value).context("serializing controller event")?;
    line.push(b'\n');
    file.write_all(&line).context("writing controller event")?;
    Ok(())
}

/// Convenience wrapper for the common case -- construct-and-append in one
/// call, timestamped now.
pub fn record(
    cfg: &GahConfig,
    event_type: EventType,
    profile: Option<&str>,
    work_id: Option<&str>,
    details: impl Into<String>,
) -> Result<()> {
    record_with_run_id_and_reason_code(cfg, event_type, profile, work_id, None, details, None)
}

pub fn record_with_run_id(
    cfg: &GahConfig,
    event_type: EventType,
    profile: Option<&str>,
    work_id: Option<&str>,
    run_id: Option<&str>,
    details: impl Into<String>,
) -> Result<()> {
    record_with_run_id_and_reason_code(cfg, event_type, profile, work_id, run_id, details, None)
}

pub fn record_with_reason_code(
    cfg: &GahConfig,
    event_type: EventType,
    profile: Option<&str>,
    work_id: Option<&str>,
    details: impl Into<String>,
    reason_code: Option<&str>,
) -> Result<()> {
    record_with_run_id_and_reason_code(
        cfg,
        event_type,
        profile,
        work_id,
        None,
        details,
        reason_code,
    )
}

pub fn record_with_run_id_and_reason_code(
    cfg: &GahConfig,
    event_type: EventType,
    profile: Option<&str>,
    work_id: Option<&str>,
    run_id: Option<&str>,
    details: impl Into<String>,
    reason_code: Option<&str>,
) -> Result<()> {
    append(
        cfg,
        &ControllerEvent {
            timestamp: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_default(),
            event_type: event_type.as_str().to_string(),
            profile: profile.map(str::to_string),
            work_id: work_id.map(str::to_string),
            run_id: run_id.map(str::to_string),
            details: details.into(),
            reason_code: reason_code.map(str::to_string),
        },
    )
}

pub fn read_events(cfg: &GahConfig) -> Result<Vec<ControllerEvent>> {
    let path = cfg.defaults.events_path();
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut events = vec![];
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str::<ControllerEvent>(line)
            .with_context(|| format!("parsing event {} from {}", idx + 1, path.display()))?;
        events.push(event);
    }
    Ok(events)
}

/// `dispatch_started` run_ids for `profile` with no matching terminal event
/// (`dispatch_finished` or `duplicate_guard_triggered`), paired with the
/// work_id the start event recorded. Terminal events are correlated purely
/// by `run_id`, matching the dashboard's `deriveControllerActivity`
/// (apps/server/src/controllerActivity.ts) so Rust and the dashboard agree
/// on what "still running" means.
pub(crate) fn orphaned_dispatch_runs(
    events: &[ControllerEvent],
    profile: &str,
) -> Vec<(String, Option<String>)> {
    let mut started = vec![];
    let mut finished = std::collections::HashSet::new();
    for event in events {
        let Some(run_id) = event.run_id.as_deref() else {
            continue;
        };
        if event.profile.as_deref() != Some(profile) {
            continue;
        }
        match event.event_type.as_str() {
            "dispatch_started" => started.push((run_id.to_string(), event.work_id.clone())),
            "dispatch_finished" | "duplicate_guard_triggered" | "review_budget_exhausted" => {
                finished.insert(run_id.to_string());
            }
            _ => {}
        }
    }
    started
        .into_iter()
        .filter(|(run_id, _)| !finished.contains(run_id))
        .collect()
}

/// Close out every `dispatch_started` for `profile` that never got a
/// terminal event -- i.e. whatever process started it (a `gah loop`
/// daemon) was killed/restarted before recording completion. Call once,
/// right after acquiring the per-profile execution lock: holding that lock
/// proves no other process can be concurrently dispatching for this
/// profile, so anything still open at that moment is provably abandoned,
/// not just slow. Without this, the dashboard's "Controller activity"
/// panel counts every such orphan as running forever.
///
/// Returns the number of orphans reconciled.
#[allow(dead_code)] // controller reconciliation also persists a ledger record
pub fn reconcile_abandoned_dispatches(cfg: &GahConfig, profile_name: &str) -> Result<usize> {
    let events = read_events(cfg)?;
    let orphans = orphaned_dispatch_runs(&events, profile_name);
    for (run_id, work_id) in &orphans {
        record_with_run_id(
            cfg,
            EventType::DispatchFinished,
            Some(profile_name),
            work_id.as_deref(),
            Some(run_id),
            "abandoned (process restarted)",
        )?;
    }
    Ok(orphans.len())
}

/// TICKET-084: `gah events [--json] [--since DURATION] [--profile NAME]`.
/// Same shape as `gah ledger summary` -- read, filter, print. No
/// `--watch`/follow mode per the ticket's own "do not overbuild before
/// event schema exists."
pub fn run(cfg: &GahConfig, since: &str, profile: Option<&str>, json: bool) -> Result<()> {
    let cutoff = crate::ledger::summary::parse_since(since)?;
    let mut events = read_events(cfg)?;
    events.retain(|e| e.timestamp >= cutoff);
    if let Some(profile) = profile {
        events.retain(|e| e.profile.as_deref() == Some(profile));
    }

    if json {
        println!("{}", serde_json::to_string(&events)?);
        return Ok(());
    }

    println!("Events: {}", cfg.defaults.events_path().display());
    println!("Count: {}", events.len());
    for event in &events {
        println!(
            "{}  {}{}  {}",
            event.timestamp,
            event.event_type,
            event
                .work_id
                .as_deref()
                .map(|w| format!("  {w}"))
                .unwrap_or_default(),
            event.details
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        append, orphaned_dispatch_runs, read_events, reconcile_abandoned_dispatches,
        record_with_run_id, ControllerEvent, EventType,
    };
    use crate::config::{Defaults, GahConfig, RoutingPolicy};
    use std::collections::HashMap;

    fn test_config() -> (tempfile::TempDir, GahConfig) {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GahConfig {
            context: Default::default(),
            defaults: Defaults {
                current_manager: None,
                artifact_root: tmp.path().to_string_lossy().into_owned(),
                worktree_base: String::new(),
                llm_base_url: String::new(),
                llm_model_local: String::new(),
                llm_model_cloud: String::new(),
                routing: RoutingPolicy::default(),
            },
            profiles: HashMap::new(),
        };
        (tmp, cfg)
    }

    #[test]
    fn event_type_as_str_matches_ticket_083_examples() {
        assert_eq!(
            EventType::ObservationCompleted.as_str(),
            "observation_completed"
        );
        assert_eq!(EventType::ActionDecided.as_str(), "action_decided");
        assert_eq!(EventType::ActionOverridden.as_str(), "action_overridden");
        assert_eq!(EventType::DispatchStarted.as_str(), "dispatch_started");
        assert_eq!(EventType::DispatchFinished.as_str(), "dispatch_finished");
        assert_eq!(
            EventType::BackendMarkedUnavailable.as_str(),
            "backend_marked_unavailable"
        );
        assert_eq!(EventType::WaitSelected.as_str(), "wait_selected");
        assert_eq!(EventType::HumanRequired.as_str(), "human_required");
        assert_eq!(
            EventType::ReviewBudgetExhausted.as_str(),
            "review_budget_exhausted"
        );
        assert_eq!(
            EventType::DuplicateGuardTriggered.as_str(),
            "duplicate_guard_triggered"
        );
        assert_eq!(EventType::LoopStopped.as_str(), "loop_stopped");
    }

    #[test]
    fn read_events_is_empty_when_file_does_not_exist() {
        let (_tmp, cfg) = test_config();
        assert!(read_events(&cfg).unwrap().is_empty());
    }

    #[test]
    fn append_then_read_round_trips() {
        let (_tmp, cfg) = test_config();
        super::record(
            &cfg,
            EventType::ActionDecided,
            Some("real"),
            Some("TICKET-001"),
            "decided DispatchTicket",
        )
        .unwrap();
        super::record(
            &cfg,
            EventType::LoopStopped,
            None,
            None,
            "one iteration done",
        )
        .unwrap();

        let events = read_events(&cfg).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "action_decided");
        assert_eq!(events[0].work_id.as_deref(), Some("TICKET-001"));
        assert_eq!(events[1].event_type, "loop_stopped");
        assert_eq!(events[1].work_id, None);
    }

    #[test]
    fn malformed_line_fails_loudly() {
        let (tmp, cfg) = test_config();
        std::fs::write(tmp.path().join("events.jsonl"), "not valid json\n").unwrap();
        assert!(read_events(&cfg).is_err());
    }

    #[test]
    fn append_never_rewrites_prior_lines() {
        let (_tmp, cfg) = test_config();
        let event = ControllerEvent {
            timestamp: "2026-07-05T00:00:00Z".into(),
            event_type: "action_decided".into(),
            profile: None,
            work_id: None,
            run_id: None,
            reason_code: None,
            details: String::new(),
        };
        append(&cfg, &event).unwrap();
        append(&cfg, &event).unwrap();
        let text = std::fs::read_to_string(cfg.defaults.events_path()).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert_eq!(text.lines().next(), text.lines().nth(1));
    }

    #[test]
    fn append_redacts_secret_like_event_details_before_persisting() {
        let (_tmp, cfg) = test_config();
        super::record(
            &cfg,
            EventType::DispatchFinished,
            Some("real"),
            None,
            "backend said Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        let text = std::fs::read_to_string(cfg.defaults.events_path()).unwrap();
        assert!(!text.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(text.contains("[REDACTED:TOKEN]"));
    }

    #[test]
    fn run_id_round_trips_for_correlated_dispatch_events() {
        let (_tmp, cfg) = test_config();
        record_with_run_id(
            &cfg,
            EventType::DispatchStarted,
            Some("gah"),
            Some("TICKET-140"),
            Some("run-123"),
            "review",
        )
        .unwrap();

        let events = read_events(&cfg).unwrap();
        assert_eq!(events[0].run_id.as_deref(), Some("run-123"));
        assert_eq!(events[0].work_id.as_deref(), Some("TICKET-140"));
    }

    #[test]
    fn review_budget_exhausted_is_terminal_for_controller_activity() {
        let (_tmp, cfg) = test_config();
        record_with_run_id(
            &cfg,
            EventType::DispatchStarted,
            Some("real"),
            Some("#113"),
            Some("run-budget"),
            "review",
        )
        .unwrap();
        record_with_run_id(
            &cfg,
            EventType::ReviewBudgetExhausted,
            Some("real"),
            Some("#113"),
            Some("run-budget"),
            "review budget exhausted",
        )
        .unwrap();

        let events = read_events(&cfg).unwrap();
        assert!(orphaned_dispatch_runs(&events, "real").is_empty());
    }

    /// Incident: a `gah loop` process gets killed/restarted mid-dispatch,
    /// leaving a `dispatch_started` with no matching `dispatch_finished`
    /// forever -- the dashboard's "Controller activity" panel counts it as
    /// running indefinitely. Reconciliation (called once, right after a
    /// fresh process acquires the per-profile lock) must close it out.
    #[test]
    fn reconcile_abandoned_dispatches_closes_out_orphaned_run_and_stops_counting_it_as_running() {
        let (_tmp, cfg) = test_config();
        record_with_run_id(
            &cfg,
            EventType::DispatchStarted,
            Some("real"),
            Some("TICKET-500"),
            Some("run-abandoned"),
            "dispatch_ticket: TICKET-500",
        )
        .unwrap();
        // An unrelated, still-legitimately-finished run for the same
        // profile must be left alone.
        record_with_run_id(
            &cfg,
            EventType::DispatchStarted,
            Some("real"),
            Some("TICKET-501"),
            Some("run-finished"),
            "dispatch_ticket: TICKET-501",
        )
        .unwrap();
        record_with_run_id(
            &cfg,
            EventType::DispatchFinished,
            Some("real"),
            Some("TICKET-501"),
            Some("run-finished"),
            "dispatch_ticket: success",
        )
        .unwrap();

        // Before reconciliation, the abandoned run_id would be counted as
        // "currently running" (the same logic the dashboard uses).
        let before = read_events(&cfg).unwrap();
        assert_eq!(
            orphaned_dispatch_runs(&before, "real"),
            vec![("run-abandoned".to_string(), Some("TICKET-500".to_string()))]
        );

        let reconciled = reconcile_abandoned_dispatches(&cfg, "real").unwrap();
        assert_eq!(reconciled, 1);

        let after = read_events(&cfg).unwrap();
        // No longer counted as running.
        assert!(orphaned_dispatch_runs(&after, "real").is_empty());
        // A terminal event now exists for it.
        let synthetic = after
            .iter()
            .find(|e| {
                e.run_id.as_deref() == Some("run-abandoned") && e.event_type == "dispatch_finished"
            })
            .expect("synthetic dispatch_finished must exist for the abandoned run_id");
        assert_eq!(synthetic.work_id.as_deref(), Some("TICKET-500"));
        assert_eq!(synthetic.details, "abandoned (process restarted)");

        // Running reconciliation again is a no-op (idempotent) -- no
        // duplicate synthetic event.
        let reconciled_again = reconcile_abandoned_dispatches(&cfg, "real").unwrap();
        assert_eq!(reconciled_again, 0);
    }

    #[test]
    fn reconcile_abandoned_dispatches_ignores_other_profiles() {
        let (_tmp, cfg) = test_config();
        record_with_run_id(
            &cfg,
            EventType::DispatchStarted,
            Some("other-profile"),
            Some("TICKET-1"),
            Some("run-other"),
            "dispatch_ticket: TICKET-1",
        )
        .unwrap();

        let reconciled = reconcile_abandoned_dispatches(&cfg, "real").unwrap();
        assert_eq!(reconciled, 0);
        let events = read_events(&cfg).unwrap();
        assert_eq!(events.len(), 1, "must not touch other profiles' events");
    }
}
