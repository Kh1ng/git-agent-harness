//! TICKET-083: append-only controller event stream. Same shape/conventions
//! as `ledger::reconcile` (TICKET-072) -- a separate JSONL file, never
//! rewritten, only ever appended to.

use crate::config::GahConfig;
use anyhow::{Context, Result};
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
    DuplicateGuardTriggered,
    LoopStopped,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ObservationCompleted => "observation_completed",
            Self::ActionDecided => "action_decided",
            Self::ActionOverridden => "action_overridden",
            Self::DispatchStarted => "dispatch_started",
            Self::DispatchFinished => "dispatch_finished",
            Self::BackendMarkedUnavailable => "backend_marked_unavailable",
            Self::WaitSelected => "wait_selected",
            Self::HumanRequired => "human_required",
            Self::DuplicateGuardTriggered => "duplicate_guard_triggered",
            Self::LoopStopped => "loop_stopped",
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
}

pub fn append(cfg: &GahConfig, event: &ControllerEvent) -> Result<()> {
    let path = cfg.defaults.events_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating events directory {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening events log {}", path.display()))?;
    serde_json::to_writer(&mut file, event).context("serializing controller event")?;
    file.write_all(b"\n").context("writing events newline")?;
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
    record_with_run_id(cfg, event_type, profile, work_id, None, details)
}

pub fn record_with_run_id(
    cfg: &GahConfig,
    event_type: EventType,
    profile: Option<&str>,
    work_id: Option<&str>,
    run_id: Option<&str>,
    details: impl Into<String>,
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
    use super::{append, read_events, record_with_run_id, ControllerEvent, EventType};
    use crate::config::{Defaults, GahConfig, RoutingPolicy};
    use std::collections::HashMap;

    fn test_config() -> (tempfile::TempDir, GahConfig) {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GahConfig {
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
            details: String::new(),
        };
        append(&cfg, &event).unwrap();
        append(&cfg, &event).unwrap();
        let text = std::fs::read_to_string(cfg.defaults.events_path()).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert_eq!(text.lines().next(), text.lines().nth(1));
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
}
