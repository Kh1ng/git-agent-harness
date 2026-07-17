//! Abandoned-run reconciliation, stuck-loop detection, and stale
//! claim/slot recovery decisions. Independently testable from the daemon
//! loop -- see `controller::runtime` for what actually drives these from
//! `run_once`/`run_parallel_once`.

use super::{decide_next_action, NextAction};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// A route-only capacity refusal launched no backend and cannot become
/// actionable while the observed routing state is unchanged. Suppress that
/// work item for a bounded interval so one unroutable repair cannot monopolize
/// a profile or make the parallel refill loop create sessions indefinitely.
const CAPACITY_DEFERRAL_BACKOFF: time::Duration = time::Duration::minutes(5);

pub(super) fn recently_capacity_deferred_work_ids(
    events: &[crate::events::ControllerEvent],
    entries: &[crate::ledger::LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    now: time::OffsetDateTime,
    current_route_state: Option<&str>,
) -> HashSet<String> {
    let parse = |timestamp: &str| {
        time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339).ok()
    };

    // An explicit retry reset or paid-route grant changes eligibility and
    // invalidates an earlier capacity deferral immediately. A route-state
    // fingerprint handles configuration and backend availability changes;
    // the five-minute bound is the fallback for historical/unfingerprinted
    // events and routing inputs GAH cannot observe directly.
    let mut reset_at: HashMap<&str, time::OffsetDateTime> = HashMap::new();
    for entry in entries {
        if entry.profile != profile_name || entry.repo_id != repo_id {
            continue;
        }
        if !matches!(
            entry.mode.as_str(),
            "clear_attempts" | "paid_route_approval_grant"
        ) {
            continue;
        }
        let (Some(work_id), Some(timestamp)) = (entry.work_id.as_deref(), parse(&entry.timestamp))
        else {
            continue;
        };
        reset_at
            .entry(work_id)
            .and_modify(|current| *current = (*current).max(timestamp))
            .or_insert(timestamp);
    }

    let mut terminal_seen = HashSet::new();
    let mut deferred = HashSet::new();
    for event in events.iter().rev() {
        if event.profile.as_deref() != Some(profile_name) || event.event_type != "dispatch_finished"
        {
            continue;
        }
        let Some(work_id) = event.work_id.as_deref() else {
            continue;
        };
        if !terminal_seen.insert(work_id) {
            continue;
        }
        if !event.details.contains(": deferred_capacity:") {
            continue;
        }
        let recorded_route_state = event
            .details
            .split_whitespace()
            .rev()
            .find_map(|part| part.strip_prefix("route_state="));
        if recorded_route_state
            .zip(current_route_state)
            .is_some_and(|(recorded, current)| recorded != current)
        {
            continue;
        }
        let Some(timestamp) = parse(&event.timestamp) else {
            continue;
        };
        if reset_at
            .get(work_id)
            .is_some_and(|reset| *reset > timestamp)
        {
            continue;
        }
        let age = now - timestamp;
        if age <= CAPACITY_DEFERRAL_BACKOFF {
            deferred.insert(work_id.to_string());
        }
    }
    deferred
}

/// Finish runs left behind by a killed controller with both durable surfaces:
/// the event stream used for live activity and the normalized ledger used for
/// routing/usage reports. `run_once` calls this after acquiring the profile
/// lock, so an open start is provably abandoned rather than merely slow.
pub(super) fn reconcile_abandoned_dispatches(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    entries: &mut Vec<crate::ledger::LedgerEntry>,
) -> Result<usize> {
    let events = crate::events::read_events(cfg)?;
    let orphans = crate::events::orphaned_dispatch_runs(&events, profile_name);
    if orphans.is_empty() {
        return Ok(0);
    }
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let existing_sessions: HashSet<String> = entries
        .iter()
        .filter_map(|entry| entry.session_id.clone())
        .collect();

    for (run_id, work_id) in &orphans {
        if !existing_sessions.contains(run_id) {
            let target = work_id.as_deref().unwrap_or("unknown");
            let mut entry = crate::ledger::LedgerEntry::new(
                profile_name,
                profile,
                "unknown",
                "abandoned",
                target,
                Some(run_id.clone()),
                None,
            );
            entry.work_id = work_id.clone();
            entry.dispatch_reason = Some("abandoned_reconciliation".to_string());
            entry.validation_result = Some("not_run_abandoned".to_string());
            entry.error_summary =
                Some("dispatch abandoned before terminal telemetry was persisted".to_string());
            entry.set_failure(
                crate::ledger::FailureClass::HarnessError,
                crate::ledger::FailureStage::AgentRun,
            );
            crate::ledger::append(cfg, &entry)?;
            entries.push(entry);
        }
        crate::events::record_with_run_id(
            cfg,
            crate::events::EventType::DispatchFinished,
            Some(profile_name),
            work_id.as_deref(),
            Some(run_id),
            "abandoned (reconciled before next dispatch)",
        )?;
    }
    Ok(orphans.len())
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
pub(super) fn detect_stuck_loop(
    events: &[crate::events::ControllerEvent],
    profile_name: &str,
    action: &NextAction,
    reset_after: Option<&str>,
) -> Option<String> {
    let work_id = action.work_id()?;
    let fingerprint_prefix = format!("{}:", action.kind());
    let mut consecutive = 0;
    for event in events.iter().rev() {
        if event.profile.as_deref() != Some(profile_name) {
            continue;
        }
        if event.work_id.as_deref() != Some(work_id) {
            continue;
        }
        if reset_after.is_some_and(|reset| timestamp_at_or_before(&event.timestamp, reset)) {
            break;
        }
        // A prior selection that reached only local route contention made no
        // agent call and explicitly deferred itself. It is an expected wait
        // boundary, not evidence that the controller is spinning without a
        // state change.
        if event.event_type == "dispatch_finished" && event.details.contains(": deferred_capacity:")
        {
            break;
        }
        if event.event_type != "action_decided" {
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

fn timestamp_at_or_before(timestamp: &str, boundary: &str) -> bool {
    use time::format_description::well_known::Rfc3339;
    match (
        time::OffsetDateTime::parse(timestamp, &Rfc3339),
        time::OffsetDateTime::parse(boundary, &Rfc3339),
    ) {
        (Ok(timestamp), Ok(boundary)) => timestamp <= boundary,
        _ => timestamp <= boundary,
    }
}

/// Return the latest operator reset that applies to this exact
/// profile/repository/work item. The ledger remains append-only; consumers
/// use the tombstone timestamp as the lower bound for controller history.
pub(super) fn latest_clear_attempts_timestamp<'a>(
    entries: &'a [crate::ledger::LedgerEntry],
    profile_name: &str,
    repo_id: &str,
    work_id: &str,
) -> Option<&'a str> {
    let aliases = crate::ledger::work_id_aliases(work_id);
    entries.iter().rev().find_map(|entry| {
        (entry.profile == profile_name
            && entry.repo_id == repo_id
            && entry.mode == "clear_attempts"
            && entry
                .work_id
                .as_deref()
                .is_some_and(|id| aliases.iter().any(|alias| alias == id)))
        .then_some(entry.timestamp.as_str())
    })
}

pub(super) fn record_action_events(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    original_action: &NextAction,
    effective_action: &NextAction,
) -> Result<()> {
    let original_reason_code = original_action.human_required_reason_code();
    let effective_reason_code = effective_action.human_required_reason_code();

    crate::events::record_with_reason_code(
        cfg,
        crate::events::EventType::ActionDecided,
        Some(profile_name),
        original_action.work_id(),
        format!("{}: {}", original_action.kind(), original_action.reason()),
        original_reason_code,
    )?;
    if original_action != effective_action {
        crate::events::record_with_reason_code(
            cfg,
            crate::events::EventType::ActionOverridden,
            Some(profile_name),
            original_action.work_id(),
            format!(
                "{} -> {}: {}",
                original_action.kind(),
                effective_action.kind(),
                effective_action.reason()
            ),
            effective_reason_code,
        )?;
    }
    Ok(())
}

/// TICKET-282: before reusing an existing branch for a `FixMr`, detect a
/// branch that is already attached to a worktree GAH does not manage (an
/// externally-owned or stale worktree for the same open PR). Returns a
/// replacement `NextAction` for the loop to run instead:
///
/// * `None` -- the action is safe to execute as-is.
/// * `Some(redispatched)` -- one or more branch conflicts were skipped and the
///   returned action is the first unblocked candidate (or a terminal action).
///
/// Path location is never treated as proof of ownership. Clean and dirty
/// attachments are both deferred; lifecycle pruning decides removal.
pub(super) fn resolve_attached_branch_conflicts(
    action: &NextAction,
    mut find_attachment: impl FnMut(&str) -> Result<Option<crate::worktree::BranchWorktreeAttachment>>,
    mut record_deferral: impl FnMut(
        &str,
        Option<&str>,
        &crate::worktree::BranchWorktreeAttachment,
    ) -> Result<()>,
    mut choose_next: impl FnMut(&HashSet<String>, &HashSet<String>) -> Result<NextAction>,
) -> Result<Option<NextAction>> {
    let mut candidate = action.clone();
    let mut deferred_work_ids = HashSet::new();
    let mut deferred_branches = HashSet::new();

    loop {
        let (branch, work_id) = match &candidate {
            NextAction::FixMr {
                branch, work_id, ..
            } => (branch, work_id),
            _ => return Ok((!deferred_branches.is_empty()).then_some(candidate)),
        };
        let Some(attachment) = find_attachment(branch)? else {
            return Ok((!deferred_branches.is_empty()).then_some(candidate));
        };

        deferred_branches.insert(branch.clone());
        if let Some(work_id) = work_id {
            deferred_work_ids.insert(work_id.clone());
        }
        record_deferral(branch, work_id.as_deref(), &attachment)?;
        candidate = choose_next(&deferred_work_ids, &deferred_branches)?;
    }
}

pub(super) fn defer_if_branch_attached(
    cfg: &crate::config::GahConfig,
    profile_name: &str,
    action: &NextAction,
) -> Result<Option<NextAction>> {
    let profile = crate::config::get_profile(cfg, profile_name)?;
    let repo = Path::new(&profile.local_path);
    resolve_attached_branch_conflicts(
        action,
        |branch| crate::worktree::branch_attachment(repo, branch),
        |branch, work_id, attachment| {
            crate::events::record(
                cfg,
                crate::events::EventType::WorkDeferred,
                Some(profile_name),
                work_id,
                format!(
                    "branch '{}' already attached to worktree {} (clean={}); deferring to next eligible item",
                    branch,
                    attachment.path.display(),
                    attachment.clean
                ),
            )
        },
        |deferred_work_ids, deferred_branches| {
            let mut fresh =
                crate::status::build_snapshot(cfg, profile_name, time::OffsetDateTime::now_utc())?;
            fresh.merge_requests.retain(|mr| {
                !mr.work_id
                    .as_ref()
                    .is_some_and(|id| deferred_work_ids.contains(id))
                    && !deferred_branches.contains(&mr.branch)
            });
            fresh.available_tickets.retain(|ticket| {
                !ticket
                    .work_id
                    .as_ref()
                    .is_some_and(|id| deferred_work_ids.contains(id))
            });
            Ok(decide_next_action(&fresh))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{
        detect_stuck_loop, latest_clear_attempts_timestamp, recently_capacity_deferred_work_ids,
        record_action_events, resolve_attached_branch_conflicts, NextAction, STUCK_LOOP_THRESHOLD,
    };
    use crate::config::{Defaults, GahConfig, RoutingPolicy};
    use crate::events::ControllerEvent;
    use crate::worktree::BranchWorktreeAttachment;
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn fix(work_id: &str, branch: &str) -> NextAction {
        NextAction::FixMr {
            work_id: Some(work_id.to_string()),
            branch: branch.to_string(),
            mr_url: None,
            reason: "test repair".to_string(),
        }
    }

    #[test]
    fn skips_every_attached_repair_before_selecting_runnable_work() {
        let first = fix("#1", "gah/one");
        let mut deferred = Vec::new();
        let mut decisions = 0;

        let replacement = resolve_attached_branch_conflicts(
            &first,
            |branch| {
                Ok(match branch {
                    "gah/one" | "gah/two" => Some(BranchWorktreeAttachment {
                        path: PathBuf::from(format!("/tmp/{branch}")),
                        clean: branch == "gah/one",
                    }),
                    _ => None,
                })
            },
            |branch, work_id, attachment| {
                deferred.push((
                    branch.to_string(),
                    work_id.map(str::to_string),
                    attachment.clean,
                ));
                Ok(())
            },
            |work_ids: &HashSet<String>, branches: &HashSet<String>| {
                decisions += 1;
                match decisions {
                    1 => {
                        assert_eq!(work_ids, &HashSet::from(["#1".to_string()]));
                        assert_eq!(branches, &HashSet::from(["gah/one".to_string()]));
                        Ok(fix("#2", "gah/two"))
                    }
                    2 => {
                        assert_eq!(
                            work_ids,
                            &HashSet::from(["#1".to_string(), "#2".to_string()])
                        );
                        assert_eq!(
                            branches,
                            &HashSet::from(["gah/one".to_string(), "gah/two".to_string()])
                        );
                        Ok(NextAction::DispatchTicket {
                            work_id: Some("#3".to_string()),
                            ticket_path: "#3".to_string(),
                            recommended_backend: None,
                            recommended_model: None,
                            reason: "next runnable item".to_string(),
                        })
                    }
                    _ => panic!("unexpected extra controller decision"),
                }
            },
        )
        .unwrap()
        .expect("conflicts must produce a replacement action");

        assert_eq!(
            deferred,
            vec![
                ("gah/one".to_string(), Some("#1".to_string()), true),
                ("gah/two".to_string(), Some("#2".to_string()), false),
            ]
        );
        assert!(matches!(
            replacement,
            NextAction::DispatchTicket { ticket_path, .. } if ticket_path == "#3"
        ));
    }

    fn decided_event(profile: &str, work_id: &str, kind: &str) -> ControllerEvent {
        ControllerEvent {
            timestamp: "2026-07-05T00:00:00Z".into(),
            event_type: "action_decided".into(),
            profile: Some(profile.into()),
            work_id: Some(work_id.into()),
            run_id: None,
            reason_code: None,
            details: format!("{kind}: test"),
        }
    }

    fn fix_mr_action() -> NextAction {
        NextAction::FixMr {
            work_id: Some("TICKET-500".into()),
            branch: "gah/real-1".into(),
            mr_url: None,
            reason: "test".into(),
        }
    }

    #[test]
    fn stuck_loop_not_detected_below_threshold() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD - 1)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        assert_eq!(
            detect_stuck_loop(&events, "real", &fix_mr_action(), None),
            None
        );
    }

    #[test]
    fn stuck_loop_detected_at_threshold() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        assert!(detect_stuck_loop(&events, "real", &fix_mr_action(), None).is_some());
    }

    #[test]
    fn different_action_kind_breaks_the_streak() {
        let mut events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        events.push(decided_event("real", "TICKET-500", "review_mr"));
        assert_eq!(
            detect_stuck_loop(&events, "real", &fix_mr_action(), None),
            None
        );
    }

    #[test]
    fn events_for_other_work_ids_do_not_count_or_break_the_streak() {
        let mut events: Vec<_> = (0..STUCK_LOOP_THRESHOLD - 1)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        events.push(decided_event("real", "TICKET-999", "fix_mr"));
        assert_eq!(
            detect_stuck_loop(&events, "real", &fix_mr_action(), None),
            None
        );
    }

    #[test]
    fn events_from_a_different_profile_are_ignored() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("other", "TICKET-500", "fix_mr"))
            .collect();
        assert_eq!(
            detect_stuck_loop(&events, "real", &fix_mr_action(), None),
            None
        );
    }

    #[test]
    fn actions_without_a_work_id_are_never_flagged_stuck() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "no_op"))
            .collect();
        let action = NextAction::NoOp {
            reason: "nothing actionable".into(),
        };
        assert_eq!(detect_stuck_loop(&events, "real", &action, None), None);
    }

    #[test]
    fn clear_attempts_boundary_resets_old_stuck_loop_decisions() {
        let events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        assert_eq!(
            detect_stuck_loop(
                &events,
                "real",
                &fix_mr_action(),
                Some("2026-07-05T00:00:01Z"),
            ),
            None
        );
    }

    #[test]
    fn post_clear_decisions_still_trigger_stuck_loop_detection() {
        let mut events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        for event in &mut events {
            event.timestamp = "2026-07-05T00:00:02Z".into();
        }
        assert!(detect_stuck_loop(
            &events,
            "real",
            &fix_mr_action(),
            Some("2026-07-05T00:00:01Z"),
        )
        .is_some());
    }

    #[test]
    fn capacity_deferral_breaks_the_stuck_loop_streak() {
        let mut events: Vec<_> = (0..STUCK_LOOP_THRESHOLD)
            .map(|_| decided_event("real", "TICKET-500", "fix_mr"))
            .collect();
        events.push(ControllerEvent {
            timestamp: "2026-07-05T00:00:01Z".into(),
            event_type: "dispatch_finished".into(),
            profile: Some("real".into()),
            work_id: Some("TICKET-500".into()),
            run_id: Some("deferred-run".into()),
            reason_code: None,
            details: "fix_existing: deferred_capacity: claude/sonnet busy".into(),
        });

        assert_eq!(
            detect_stuck_loop(&events, "real", &fix_mr_action(), None),
            None
        );
    }

    fn capacity_finished(profile: &str, work_id: &str, timestamp: &str) -> ControllerEvent {
        ControllerEvent {
            timestamp: timestamp.into(),
            event_type: "dispatch_finished".into(),
            profile: Some(profile.into()),
            work_id: Some(work_id.into()),
            run_id: Some(format!("run-{profile}-{work_id}")),
            reason_code: None,
            details: "fix_existing: deferred_capacity: no eligible backend".into(),
        }
    }

    #[test]
    fn recent_capacity_deferral_is_provider_neutral_and_bounded() {
        let now = time::OffsetDateTime::parse(
            "2026-07-17T08:10:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        for (provider, profile, repo_id) in [
            ("github", "gah", "gah-repo"),
            ("gitlab", "sportsball", "sportsball-repo"),
        ] {
            let recent = capacity_finished(profile, "#155", "2026-07-17T08:09:00Z");
            let deferred =
                recently_capacity_deferred_work_ids(&[recent], &[], profile, repo_id, now, None);
            assert!(
                deferred.contains("#155"),
                "{provider} deferral was not suppressed"
            );

            let old = capacity_finished(profile, "#155", "2026-07-17T08:04:59Z");
            assert!(
                recently_capacity_deferred_work_ids(&[old], &[], profile, repo_id, now, None)
                    .is_empty()
            );
        }
    }

    #[test]
    fn later_terminal_result_or_explicit_grant_releases_capacity_backoff() {
        let now = time::OffsetDateTime::parse(
            "2026-07-17T08:10:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let deferred = capacity_finished("sportsball", "#155", "2026-07-17T08:09:00Z");
        let mut success = deferred.clone();
        success.timestamp = "2026-07-17T08:09:10Z".into();
        success.details = "fix_existing: success".into();
        assert!(recently_capacity_deferred_work_ids(
            &[deferred.clone(), success],
            &[],
            "sportsball",
            "sportsball",
            now,
            None,
        )
        .is_empty());

        let mut profile = crate::ledger::test_util::profile();
        profile.repo_id = "sportsball".into();
        profile.provider = "gitlab".into();
        let mut grant = crate::ledger::LedgerEntry::new(
            "sportsball",
            &profile,
            "auto",
            "paid_route_approval_grant",
            "#155",
            None,
            None,
        );
        grant.work_id = Some("#155".into());
        grant.timestamp = "2026-07-17T08:09:30Z".into();
        assert!(recently_capacity_deferred_work_ids(
            &[deferred],
            &[grant],
            "sportsball",
            "sportsball",
            now,
            None,
        )
        .is_empty());
    }

    #[test]
    fn changed_route_state_releases_capacity_backoff_immediately() {
        let now = time::OffsetDateTime::parse(
            "2026-07-17T08:10:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let mut deferred = capacity_finished("gah", "#683", "2026-07-17T08:09:59Z");
        deferred.details.push_str(" route_state=before");

        assert!(recently_capacity_deferred_work_ids(
            std::slice::from_ref(&deferred),
            &[],
            "gah",
            "gah",
            now,
            Some("before"),
        )
        .contains("#683"));
        assert!(recently_capacity_deferred_work_ids(
            &[deferred],
            &[],
            "gah",
            "gah",
            now,
            Some("after"),
        )
        .is_empty());
    }

    #[test]
    fn latest_clear_attempts_is_scoped_to_profile_repo_and_work_alias() {
        let profile: crate::config::Profile = toml::from_str(
            r#"
display_name = "Real"
repo_id = "real-repo"
provider = "github"
repo = "owner/real"
local_path = "/tmp/real"
artifact_root = "/tmp/real-artifacts"
default_target_branch = "main"
"#,
        )
        .unwrap();
        let mut matching =
            crate::ledger::LedgerEntry::new_clear_attempts("real", &profile, "TICKET-500");
        matching.timestamp = "2026-07-05T00:00:01Z".into();
        let mut other_profile =
            crate::ledger::LedgerEntry::new_clear_attempts("other", &profile, "#500");
        other_profile.timestamp = "2026-07-05T00:00:02Z".into();
        let mut other_work =
            crate::ledger::LedgerEntry::new_clear_attempts("real", &profile, "#501");
        other_work.timestamp = "2026-07-05T00:00:03Z".into();

        assert_eq!(
            latest_clear_attempts_timestamp(
                &[matching, other_profile, other_work],
                "real",
                "real-repo",
                "#500",
            ),
            Some("2026-07-05T00:00:01Z")
        );
    }

    fn event_test_config() -> (tempfile::TempDir, GahConfig) {
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
    fn once_reconciliation_writes_terminal_event_and_unknown_ledger_record() {
        let (_tmp, mut cfg) = event_test_config();
        let profile: crate::config::Profile = toml::from_str(
            r#"
display_name = "Real"
repo_id = "real"
provider = "github"
repo = "owner/real"
local_path = "/tmp/real"
artifact_root = "/tmp/real-artifacts"
default_target_branch = "main"
"#,
        )
        .unwrap();
        cfg.profiles.insert("real".into(), profile);
        crate::events::record_with_run_id(
            &cfg,
            crate::events::EventType::DispatchStarted,
            Some("real"),
            Some("TICKET-500"),
            Some("orphaned-run"),
            "dispatch_ticket: 500",
        )
        .unwrap();
        let mut ledger_entries = crate::ledger::read_entries(&cfg).unwrap();

        assert_eq!(
            super::reconcile_abandoned_dispatches(&cfg, "real", &mut ledger_entries).unwrap(),
            1
        );

        let events = crate::events::read_events(&cfg).unwrap();
        assert!(events.iter().any(|event| {
            event.run_id.as_deref() == Some("orphaned-run")
                && event.event_type == "dispatch_finished"
                && event.details.contains("abandoned")
        }));
        let ledger = crate::ledger::read_entries(&cfg).unwrap();
        let entry = ledger
            .iter()
            .find(|entry| entry.session_id.as_deref() == Some("orphaned-run"))
            .expect("reconciliation must persist an unknown terminal ledger record");
        assert_eq!(entry.work_id.as_deref(), Some("TICKET-500"));
        assert_eq!(entry.failure_class.as_deref(), Some("harness_error"));
        assert_eq!(
            entry.validation_result.as_deref(),
            Some("not_run_abandoned")
        );
    }

    #[test]
    fn stuck_loop_override_records_original_decision_and_override() {
        let (_tmp, cfg) = event_test_config();
        let original = NextAction::ReviewMr {
            work_id: Some("TICKET-500".into()),
            branch: "gah/real-1".into(),
            mr_url: Some("https://example/review".into()),
            reason: "MR on branch 'gah/real-1' classified NEEDS_REVIEW".into(),
        };
        let effective = NextAction::HumanRequired {
            reason: "stuck-loop detected: 'review_mr' selected 3 times in a row for TICKET-500 with no intervening state change".into(),
            reference: Some("TICKET-500".into()),
            reason_code: Some("policy_approval".into()),
        };

        record_action_events(&cfg, "real", &original, &effective).unwrap();

        let events = crate::events::read_events(&cfg).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "action_decided");
        assert!(events[0].details.starts_with("review_mr:"));
        assert_eq!(events[1].event_type, "action_overridden");
        assert!(events[1].details.contains("review_mr -> human_required"));
    }
}
