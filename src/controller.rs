//! TICKET-077: durable, typed controller actions. The schema only -- no
//! execution here (see `dispatch::run` for execution, wired from
//! `gah loop --once`, TICKET-079).
//!
//! Every variant carries a mandatory `reason` (why this action was
//! selected) plus enough identity to execute it without re-observing
//! state. Serializable so it can be persisted verbatim into a controller
//! event (TICKET-083).

use crate::status::StatusSnapshot;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// TICKET-078: how many times the controller will automatically
/// Retry/Escalate the same work_id before giving up and requiring a human.
/// Deliberately small and inline (not configurable) -- this is a safety
/// floor, not a policy knob; see TICKET-081 for the broader stuck-loop
/// detector this complements.
const AUTO_RETRY_CAP: usize = 2;

fn is_genuine_agent_failure(failure_class: &str) -> bool {
    matches!(failure_class, "agent_no_progress" | "agent_failure")
}

fn is_infra_failure(failure_class: &str) -> bool {
    matches!(
        failure_class,
        "harness_error" | "environment_error" | "backend_error" | "unknown"
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum NextAction {
    ReviewMr {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        reason: String,
    },
    FixMr {
        work_id: Option<String>,
        branch: String,
        mr_url: Option<String>,
        reason: String,
    },
    DispatchTicket {
        ticket_path: String,
        work_id: Option<String>,
        recommended_backend: Option<String>,
        recommended_model: Option<String>,
        reason: String,
    },
    /// TICKET-078: redispatch a ticket whose last attempt failed for an
    /// infra reason (harness/environment/backend/unknown) that has since
    /// cleared -- same backend/model as before, not escalated.
    Retry {
        work_id: String,
        ticket_path: String,
        reason: String,
    },
    /// TICKET-078: redispatch a ticket whose last attempt was a genuine
    /// agent-capability failure (agent_no_progress/agent_failure),
    /// requesting a stronger backend/model this time.
    Escalate {
        work_id: String,
        ticket_path: String,
        reason: String,
    },
    WaitUntil {
        until: String,
        reason: String,
    },
    HumanRequired {
        reason: String,
        #[serde(default)]
        reference: Option<String>,
    },
    NoOp {
        reason: String,
    },
}

impl NextAction {
    /// Coarse type name for logging/fingerprinting (TICKET-081) -- stable
    /// even if variant fields change shape.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ReviewMr { .. } => "review_mr",
            Self::FixMr { .. } => "fix_mr",
            Self::DispatchTicket { .. } => "dispatch_ticket",
            Self::Retry { .. } => "retry",
            Self::Escalate { .. } => "escalate",
            Self::WaitUntil { .. } => "wait_until",
            Self::HumanRequired { .. } => "human_required",
            Self::NoOp { .. } => "no_op",
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::ReviewMr { reason, .. }
            | Self::FixMr { reason, .. }
            | Self::DispatchTicket { reason, .. }
            | Self::Retry { reason, .. }
            | Self::Escalate { reason, .. }
            | Self::WaitUntil { reason, .. }
            | Self::HumanRequired { reason, .. }
            | Self::NoOp { reason } => reason,
        }
    }

    /// The work_id this action is about, where one exists. Used for
    /// fingerprinting (TICKET-081) and event logging (TICKET-083).
    pub fn work_id(&self) -> Option<&str> {
        match self {
            Self::ReviewMr { work_id, .. } | Self::FixMr { work_id, .. } => work_id.as_deref(),
            Self::DispatchTicket { work_id, .. } => work_id.as_deref(),
            Self::Retry { work_id, .. } | Self::Escalate { work_id, .. } => Some(work_id),
            Self::WaitUntil { .. } | Self::HumanRequired { .. } | Self::NoOp { .. } => None,
        }
    }
}

/// TICKET-078: pure, deterministic, no LLM, no I/O -- consumes an
/// already-built `StatusSnapshot` and returns exactly one action. First
/// matching rule wins:
///
/// 1. incomplete critical observation -> stop safely (NoOp)
/// 2. a recorded blocker (today: ledger human_required) -> HumanRequired
/// 3. an MR classified NEEDS_REVIEW -> ReviewMr
/// 4. an MR classified CI_FAILED/NEEDS_FIX -> FixMr
/// 5. an MR classified READY_FOR_HUMAN -> HumanRequired
/// 6. a ticket with failed history, no active MR, capability failure,
///    under the retry cap -> Escalate
/// 7. a ticket with failed history, no active MR, infra failure, some
///    backend eligible again, under the retry cap -> Retry
/// 8. a ticket with failed history, no active MR, retry cap exceeded ->
///    HumanRequired
/// 9. an eligible never-dispatched ticket -> DispatchTicket
/// 10. all remaining backends unavailable but with a known reset -> WaitUntil
/// 11. otherwise -> NoOp
///
/// Ties within a tier (multiple matching MRs) are broken by branch name,
/// lexicographically -- `SyncMrJson` doesn't carry `updated_at`, so this is
/// the only deterministic ordering available without widening that type.
pub fn decide_next_action(snapshot: &StatusSnapshot) -> NextAction {
    if let Some(err) = snapshot.errors.iter().find(|e| e.incomplete_snapshot) {
        return NextAction::NoOp {
            reason: format!(
                "observation incomplete ({}): {}",
                err.subsystem, err.message
            ),
        };
    }

    if let Some(blocker) = snapshot.blockers.first() {
        return NextAction::HumanRequired {
            reason: blocker
                .message
                .clone()
                .unwrap_or_else(|| blocker.kind.clone()),
            reference: blocker.source_reference.clone(),
        };
    }

    let mut mrs: Vec<&crate::sync::SyncMrJson> = snapshot.merge_requests.iter().collect();
    mrs.sort_by(|a, b| a.branch.cmp(&b.branch));

    if let Some(mr) = mrs.iter().find(|mr| mr.classification == "NEEDS_REVIEW") {
        return NextAction::ReviewMr {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            reason: format!("MR on branch '{}' classified NEEDS_REVIEW", mr.branch),
        };
    }
    if let Some(mr) = mrs
        .iter()
        .find(|mr| matches!(mr.classification.as_str(), "CI_FAILED" | "NEEDS_FIX"))
    {
        return NextAction::FixMr {
            work_id: mr.work_id.clone(),
            branch: mr.branch.clone(),
            mr_url: mr.url.clone(),
            reason: format!(
                "MR on branch '{}' classified {}",
                mr.branch, mr.classification
            ),
        };
    }
    if let Some(mr) = mrs.iter().find(|mr| mr.classification == "READY_FOR_HUMAN") {
        return NextAction::HumanRequired {
            reason: format!(
                "MR on branch '{}' ready for human merge decision",
                mr.branch
            ),
            reference: mr.url.clone(),
        };
    }

    let some_backend_eligible = snapshot.availability.iter().any(|a| a.eligible_now);
    let mut failed_tickets: Vec<_> = snapshot
        .available_tickets
        .iter()
        .filter(|t| !t.has_active_mr && t.prior_attempt_count > 0)
        .collect();
    failed_tickets.sort_by(|a, b| a.ticket_path.cmp(&b.ticket_path));

    for ticket in &failed_tickets {
        if ticket.prior_attempt_count >= AUTO_RETRY_CAP {
            return NextAction::HumanRequired {
                reason: format!(
                    "{} failed {} time(s) with no active MR; stopping automatic retries",
                    ticket.work_id.as_deref().unwrap_or(&ticket.ticket_path),
                    ticket.prior_attempt_count
                ),
                reference: ticket
                    .work_id
                    .clone()
                    .or_else(|| Some(ticket.ticket_path.clone())),
            };
        }
    }
    for ticket in &failed_tickets {
        if let Some(fc) = ticket.last_failure_class.as_deref() {
            if is_genuine_agent_failure(fc) {
                return NextAction::Escalate {
                    work_id: ticket
                        .work_id
                        .clone()
                        .unwrap_or_else(|| ticket.ticket_path.clone()),
                    ticket_path: ticket.ticket_path.clone(),
                    reason: format!(
                        "prior attempt failed ({fc}); escalating to a stronger backend/model"
                    ),
                };
            }
        }
    }
    for ticket in &failed_tickets {
        if let Some(fc) = ticket.last_failure_class.as_deref() {
            if is_infra_failure(fc) && some_backend_eligible {
                return NextAction::Retry {
                    work_id: ticket
                        .work_id
                        .clone()
                        .unwrap_or_else(|| ticket.ticket_path.clone()),
                    ticket_path: ticket.ticket_path.clone(),
                    reason: format!(
                        "prior attempt failed ({fc}); retrying now that a backend appears available"
                    ),
                };
            }
        }
    }

    let mut undispatched: Vec<_> = snapshot
        .available_tickets
        .iter()
        .filter(|t| !t.has_active_mr && t.prior_attempt_count == 0)
        .collect();
    undispatched.sort_by(|a, b| a.ticket_path.cmp(&b.ticket_path));
    if let Some(ticket) = undispatched.first() {
        return NextAction::DispatchTicket {
            ticket_path: ticket.ticket_path.clone(),
            work_id: ticket.work_id.clone(),
            recommended_backend: ticket.recommended_backend.clone(),
            recommended_model: ticket.recommended_model.clone(),
            reason: "eligible undispatched ticket".into(),
        };
    }

    if let Some(scope) = snapshot
        .availability
        .iter()
        .find(|a| !a.eligible_now && a.unavailable_until.is_some())
    {
        return NextAction::WaitUntil {
            until: scope.unavailable_until.clone().unwrap(),
            reason: format!(
                "{} unavailable ({})",
                scope.backend,
                scope.reason.clone().unwrap_or_default()
            ),
        };
    }

    NextAction::NoOp {
        reason: "nothing actionable".into(),
    }
}

/// TICKET-081: how many consecutive `action_decided` events for the same
/// (kind, work_id) fingerprint, with nothing else for that work_id in
/// between, count as "stuck." Broader than TICKET-078's inline retry cap,
/// which only gates Retry/Escalate via ledger counts -- this catches any
/// action kind repeating (e.g. ReviewMr/FixMr selected over and over for a
/// branch whose classification never changes).
const STUCK_LOOP_THRESHOLD: usize = 3;

/// Returns `Some(reason)` if the last `STUCK_LOOP_THRESHOLD` decisions for
/// this action's work_id all match its current fingerprint. Reads the
/// existing event stream (TICKET-083) rather than new storage.
fn detect_stuck_loop(
    events: &[crate::events::ControllerEvent],
    profile_name: &str,
    action: &NextAction,
) -> Option<String> {
    let work_id = action.work_id()?;
    let fingerprint_prefix = format!("{}:", action.kind());
    let mut consecutive = 0;
    for event in events.iter().rev() {
        if event.profile.as_deref() != Some(profile_name) || event.event_type != "action_decided" {
            continue;
        }
        if event.work_id.as_deref() != Some(work_id) {
            continue;
        }
        if event.details.starts_with(&fingerprint_prefix) {
            consecutive += 1;
            if consecutive >= STUCK_LOOP_THRESHOLD {
                return Some(format!(
                    "stuck-loop detected: '{}' selected {} times in a row for {} with no \
                     intervening state change",
                    action.kind(),
                    consecutive,
                    work_id
                ));
            }
        } else {
            break;
        }
    }
    None
}

/// TICKET-079: `gah loop --once` -- exactly one bounded controller
/// iteration. Build a snapshot, decide one action, execute at most that
/// one action, persist one controller event trail, exit. No daemon, no
/// repeated recursion.
#[derive(Debug, Serialize)]
pub struct LoopOnceResult {
    pub action: NextAction,
    pub outcome: String,
}

pub fn run_once(cfg: &crate::config::GahConfig, profile_name: &str, json: bool) -> Result<()> {
    let now = time::OffsetDateTime::now_utc();
    let snapshot = crate::status::build_snapshot(cfg, profile_name, now)?;
    crate::events::record(
        cfg,
        crate::events::EventType::ObservationCompleted,
        Some(profile_name),
        None,
        format!("profile={profile_name}"),
    )?;

    let mut action = decide_next_action(&snapshot);
    let history = crate::events::read_events(cfg)?;
    if let Some(reason) = detect_stuck_loop(&history, profile_name, &action) {
        action = NextAction::HumanRequired {
            reason,
            reference: action.work_id().map(str::to_string),
        };
    }
    crate::events::record(
        cfg,
        crate::events::EventType::ActionDecided,
        Some(profile_name),
        action.work_id(),
        format!("{}: {}", action.kind(), action.reason()),
    )?;

    let outcome = execute_action(cfg, profile_name, &action)?;

    let stop_event_type = match &action {
        NextAction::WaitUntil { .. } => crate::events::EventType::WaitSelected,
        NextAction::HumanRequired { .. } => crate::events::EventType::HumanRequired,
        _ => crate::events::EventType::LoopStopped,
    };
    crate::events::record(
        cfg,
        stop_event_type,
        Some(profile_name),
        action.work_id(),
        outcome.clone(),
    )?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&LoopOnceResult { action, outcome })?
        );
    } else {
        println!("Decided: {} -- {}", action.kind(), action.reason());
        println!("{outcome}");
    }
    Ok(())
}

/// Executes at most one action. `FixMr` is deliberately not executed here:
/// `fix`/`improve` mode always branches fresh off `default_target_branch`
/// (see `worktree::create`) -- there is no existing capability in this
/// codebase to check out and continue an already-open MR's branch. Rather
/// than silently building that (a separate, larger change) or silently
/// dropping the decision, this reports it honestly and takes no action.
pub(crate) fn execute_action(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    action: &NextAction,
) -> Result<String> {
    let base_args = || crate::dispatch::DispatchArgs {
        profile: profile_name.to_string(),
        mode: "fix".to_string(),
        backend: "auto".to_string(),
        target: String::new(),
        branch: None,
        mr: None,
        current_branch: false,
        budget: 0,
        dry_run: false,
        config_path: None,
        oh_profile: None,
        model: None,
        retries: 2,
        allow_draft_fail: false,
        prod: false,
        allow_unknown_red_baseline: false,
        escalate: false,
    };

    match action {
        NextAction::ReviewMr { branch, .. } => {
            let args = crate::dispatch::DispatchArgs {
                mode: "review".to_string(),
                branch: Some(branch.clone()),
                ..base_args()
            };
            run_dispatch_and_record(cfg, action, &args, "review")?;
            Ok(format!("Dispatched review for branch '{branch}'"))
        }
        NextAction::FixMr { branch, mr_url, .. } => Ok(format!(
            "FixMr decided for branch '{branch}' ({}), but fix mode does not yet support \
             continuing an existing branch -- no action taken.",
            mr_url.as_deref().unwrap_or("no MR URL")
        )),
        NextAction::DispatchTicket { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                ..base_args()
            };
            run_dispatch_and_record(cfg, action, &args, "dispatch_ticket")?;
            Ok(format!("Dispatched ticket '{ticket_path}'"))
        }
        NextAction::Retry { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                ..base_args()
            };
            run_dispatch_and_record(cfg, action, &args, "retry")?;
            Ok(format!("Retried ticket '{ticket_path}'"))
        }
        NextAction::Escalate { ticket_path, .. } => {
            let args = crate::dispatch::DispatchArgs {
                target: ticket_path.clone(),
                escalate: true,
                ..base_args()
            };
            run_dispatch_and_record(cfg, action, &args, "escalate")?;
            Ok(format!("Escalated ticket '{ticket_path}'"))
        }
        NextAction::WaitUntil { until, reason } => Ok(format!("Waiting until {until} ({reason})")),
        NextAction::HumanRequired { reason, reference } => Ok(format!(
            "Human required: {reason}{}",
            reference
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        )),
        NextAction::NoOp { reason } => Ok(format!("No action: {reason}")),
    }
}

/// Records `DispatchStarted`, runs `dispatch::run`, then records either
/// `DispatchFinished` (success) or `DuplicateGuardTriggered` (the specific,
/// already-known "active open PR" refusal from TICKET-097's
/// `check_duplicate_work`) / a generic failure note -- so the event log
/// distinguishes "the duplicate guard correctly refused this" from an
/// ordinary dispatch failure, without needing a new typed error variant
/// threaded through `dispatch::run`'s `Result<()>`.
fn run_dispatch_and_record(
    cfg: &crate::config::GahConfig,
    action: &NextAction,
    args: &crate::dispatch::DispatchArgs,
    label: &str,
) -> Result<()> {
    crate::events::record(
        cfg,
        crate::events::EventType::DispatchStarted,
        Some(args.profile.as_str()),
        action.work_id(),
        label,
    )?;
    match crate::dispatch::run(cfg, args) {
        Ok(()) => {
            crate::events::record(
                cfg,
                crate::events::EventType::DispatchFinished,
                Some(args.profile.as_str()),
                action.work_id(),
                format!("{label}: success"),
            )?;
            Ok(())
        }
        Err(e) => {
            let event_type = if e.to_string().contains("active open PR already exists") {
                crate::events::EventType::DuplicateGuardTriggered
            } else {
                crate::events::EventType::DispatchFinished
            };
            crate::events::record(
                cfg,
                event_type,
                Some(args.profile.as_str()),
                action.work_id(),
                format!("{label}: {e:#}"),
            )?;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NextAction;

    #[test]
    fn kind_is_stable_short_name_per_variant() {
        let action = NextAction::NoOp {
            reason: "nothing actionable".into(),
        };
        assert_eq!(action.kind(), "no_op");
        assert_eq!(action.reason(), "nothing actionable");
        assert_eq!(action.work_id(), None);
    }

    #[test]
    fn retry_and_escalate_expose_work_id() {
        let retry = NextAction::Retry {
            work_id: "TICKET-042".into(),
            ticket_path: "docs/tickets/TICKET-042-x.md".into(),
            reason: "infra failure cleared".into(),
        };
        assert_eq!(retry.kind(), "retry");
        assert_eq!(retry.work_id(), Some("TICKET-042"));

        let escalate = NextAction::Escalate {
            work_id: "TICKET-043".into(),
            ticket_path: "docs/tickets/TICKET-043-y.md".into(),
            reason: "no progress last attempt".into(),
        };
        assert_eq!(escalate.kind(), "escalate");
        assert_eq!(escalate.work_id(), Some("TICKET-043"));
    }

    #[test]
    fn round_trips_through_json() {
        let action = NextAction::ReviewMr {
            work_id: Some("TICKET-001".into()),
            branch: "gah/real-1".into(),
            mr_url: Some("https://example/pull/1".into()),
            reason: "classified NEEDS_REVIEW".into(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: NextAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, action);
    }

    #[test]
    fn wait_until_and_human_required_have_no_work_id() {
        let wait = NextAction::WaitUntil {
            until: "2026-07-06T00:00:00Z".into(),
            reason: "backend unavailable".into(),
        };
        assert_eq!(wait.work_id(), None);

        let human = NextAction::HumanRequired {
            reason: "MR ready for human decision".into(),
            reference: Some("https://example/pull/2".into()),
        };
        assert_eq!(human.work_id(), None);
    }

    use super::decide_next_action;
    use crate::models::AvailableTicket;
    use crate::status::{
        Blocker, ObservationStatus, Observations, ProfileIdentity, ScopeStatusJson, StatusError,
        StatusSnapshot,
    };
    use crate::sync::{RecommendedAction, SyncMrJson};

    fn empty_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            schema_version: 1,
            generated_at: "2026-07-05T00:00:00Z".into(),
            profile: ProfileIdentity {
                profile: "real".into(),
                display_name: "Real".into(),
                repo_id: "real".into(),
                provider: "github".into(),
                local_path: "/tmp/repo".into(),
                default_target_branch: "main".into(),
            },
            observations: Observations {
                sync: ObservationStatus { status: "ok" },
                availability: ObservationStatus { status: "ok" },
                ledger: ObservationStatus { status: "ok" },
            },
            merge_requests: vec![],
            availability: vec![],
            recent_ledger: None,
            constraints: vec![],
            blockers: vec![],
            errors: vec![],
            available_tickets: vec![],
        }
    }

    fn mr(branch: &str, classification: &str) -> SyncMrJson {
        SyncMrJson {
            profile: None,
            branch: branch.into(),
            work_id: Some(format!("TICKET-{branch}")),
            id: Some("1".into()),
            url: Some(format!("https://example/{branch}")),
            state: Some("OPEN".into()),
            draft: false,
            merge_status: None,
            merged: classification == "MERGED",
            classification: classification.into(),
            recommended_action: RecommendedAction::from_class(classification),
        }
    }

    fn ticket(
        path: &str,
        work_id: Option<&str>,
        prior_attempt_count: usize,
        last_failure_class: Option<&str>,
        has_active_mr: bool,
    ) -> AvailableTicket {
        AvailableTicket {
            ticket_path: path.into(),
            work_id: work_id.map(str::to_string),
            title: None,
            recommended_backend: None,
            recommended_model: None,
            prior_attempt_count,
            last_failure_class: last_failure_class.map(str::to_string),
            has_active_mr,
        }
    }

    #[test]
    fn incomplete_observation_stops_safely() {
        let mut snapshot = empty_snapshot();
        snapshot.errors.push(StatusError {
            subsystem: "sync".into(),
            message: "gh not found".into(),
            incomplete_snapshot: true,
        });
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
        assert!(action.reason().contains("sync"));
    }

    #[test]
    fn blocker_forces_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot.blockers.push(Blocker {
            kind: "human_required".into(),
            reason: Some("ledger_human_required".into()),
            message: Some("Ledger indicates human intervention required".into()),
            backend: None,
            model: None,
            until: None,
            source_reference: Some("gah/real-1".into()),
        });
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
    }

    #[test]
    fn needs_review_mr_takes_priority() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(mr("gah/real-1", "NEEDS_FIX"));
        snapshot
            .merge_requests
            .push(mr("gah/real-2", "NEEDS_REVIEW"));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::ReviewMr { branch, .. } => assert_eq!(branch, "gah/real-2"),
            other => panic!("expected ReviewMr, got {other:?}"),
        }
    }

    #[test]
    fn ci_failed_mr_maps_to_fix_mr() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(mr("gah/real-1", "CI_FAILED"));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "fix_mr");
    }

    #[test]
    fn ready_for_human_mr_maps_to_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot
            .merge_requests
            .push(mr("gah/real-1", "READY_FOR_HUMAN"));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
    }

    #[test]
    fn merged_and_closed_mrs_are_not_actionable() {
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests.push(mr("gah/real-1", "MERGED"));
        snapshot
            .merge_requests
            .push(mr("gah/real-2", "CLOSED_UNMERGED"));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
    }

    #[test]
    fn genuine_agent_failure_escalates() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-001-x.md",
            Some("TICKET-001"),
            1,
            Some("agent_no_progress"),
            false,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::Escalate { work_id, .. } => assert_eq!(work_id, "TICKET-001"),
            other => panic!("expected Escalate, got {other:?}"),
        }
    }

    #[test]
    fn infra_failure_retries_only_when_a_backend_is_eligible() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-002-x.md",
            Some("TICKET-002"),
            1,
            Some("harness_error"),
            false,
        ));

        // No eligible backend at all -> must not retry blindly.
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");

        // Now a backend is eligible -> retry.
        snapshot.availability.push(ScopeStatusJson {
            backend: "codex".into(),
            model: None,
            quota_pool: None,
            eligible_now: true,
            reason: None,
            unavailable_until: None,
            source: None,
            last_error_summary: None,
            observed_at: None,
            scope: None,
        });
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::Retry { work_id, .. } => assert_eq!(work_id, "TICKET-002"),
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn retry_cap_exceeded_forces_human_required() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-003-x.md",
            Some("TICKET-003"),
            2, // == AUTO_RETRY_CAP
            Some("agent_no_progress"),
            false,
        ));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "human_required");
    }

    #[test]
    fn never_dispatched_ticket_is_eligible() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-004-x.md",
            Some("TICKET-004"),
            0,
            None,
            false,
        ));
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::DispatchTicket { work_id, .. } => {
                assert_eq!(work_id.as_deref(), Some("TICKET-004"))
            }
            other => panic!("expected DispatchTicket, got {other:?}"),
        }
    }

    #[test]
    fn ticket_with_active_mr_is_never_a_dispatch_candidate() {
        let mut snapshot = empty_snapshot();
        snapshot.available_tickets.push(ticket(
            "docs/tickets/TICKET-005-x.md",
            Some("TICKET-005"),
            1,
            Some("agent_no_progress"),
            true, // has_active_mr
        ));
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
    }

    #[test]
    fn unavailable_backend_with_known_reset_waits() {
        let mut snapshot = empty_snapshot();
        snapshot.availability.push(ScopeStatusJson {
            backend: "claude".into(),
            model: None,
            quota_pool: None,
            eligible_now: false,
            reason: Some("rate_limited".into()),
            unavailable_until: Some("2026-07-06T00:00:00Z".into()),
            source: None,
            last_error_summary: None,
            observed_at: None,
            scope: None,
        });
        let action = decide_next_action(&snapshot);
        match action {
            NextAction::WaitUntil { until, .. } => assert_eq!(until, "2026-07-06T00:00:00Z"),
            other => panic!("expected WaitUntil, got {other:?}"),
        }
    }

    #[test]
    fn nothing_actionable_is_noop() {
        let snapshot = empty_snapshot();
        let action = decide_next_action(&snapshot);
        assert_eq!(action.kind(), "no_op");
    }

    use super::{detect_stuck_loop, STUCK_LOOP_THRESHOLD};
    use crate::events::ControllerEvent;

    fn decided_event(profile: &str, work_id: &str, kind: &str) -> ControllerEvent {
        ControllerEvent {
            timestamp: "2026-07-05T00:00:00Z".into(),
            event_type: "action_decided".into(),
            profile: Some(profile.into()),
            work_id: Some(work_id.into()),
            details: format!("{kind}: some reason"),
        }
    }

    fn fix_mr_action() -> NextAction {
        NextAction::FixMr {
            work_id: Some("TICKET-500".into()),
            branch: "gah/real-1".into(),
            mr_url: None,
            reason: "MR needs fix".into(),
        }
    }

    #[test]
    fn stuck_loop_not_detected_below_threshold() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD - 1)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action()).is_none());
    }

    #[test]
    fn stuck_loop_detected_at_threshold() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        let reason = detect_stuck_loop(&events, "real", &fix_mr_action());
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("fix_mr"));
    }

    #[test]
    fn different_action_kind_breaks_the_streak() {
        let mut events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        // A review_mr decision landed in between -- state changed, no longer stuck.
        events.push(decided_event("real", "TICKET-500", "review_mr"));
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action()).is_none());
    }

    #[test]
    fn events_for_other_work_ids_do_not_count_or_break_the_streak() {
        let mut events = vec![decided_event("real", "TICKET-500", "fix_mr")];
        events.push(decided_event("real", "TICKET-999", "dispatch_ticket"));
        events.extend(
            (0..STUCK_LOOP_THRESHOLD - 1).map(|_| decided_event("real", "TICKET-500", "fix_mr")),
        );
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action()).is_some());
    }

    #[test]
    fn events_from_a_different_profile_are_ignored() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("other-profile", "TICKET-500", "fix_mr"))
            .collect();
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action()).is_none());
    }

    #[test]
    fn actions_without_a_work_id_are_never_flagged_stuck() {
        let events: Vec<_> = (0..10)
            .map(|_| decided_event("real", "", "no_op"))
            .collect();
        let action = NextAction::NoOp {
            reason: "nothing actionable".into(),
        };
        assert!(detect_stuck_loop(&events, "real", &action).is_none());
    }
}
