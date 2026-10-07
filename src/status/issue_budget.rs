//! Per-issue budget projection (`routing.issue_budget`). Derived from the
//! ledger on every snapshot and never stored, so a `gah clear-attempts`
//! tombstone releases a held issue the same way it resets the retry cap.
//! The controller reads the projected status; it never re-counts.

use super::WorkWaypointEvidence;
use crate::config::{IssueBudget, Profile};
use crate::dispatch::{issue_budget_usage, IssueBudgetUsage};
use crate::ledger::LedgerEntry;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct IssueBudgetStatus {
    pub attempts_used: u32,
    pub max_attempts: Option<u32>,
    /// Attempts of `max_attempts` only an escalation may consume.
    pub escalation_reserve_attempts: u32,
    pub elapsed_minutes: u32,
    pub max_elapsed_minutes: Option<u32>,
    pub manager_rounds_used: u32,
    pub max_manager_rounds: Option<u32>,
    /// The remaining attempts are inside the escalation reserve: a plain
    /// same-tier retry is refused, an escalation is still allowed.
    pub escalation_only: bool,
    /// The budget is spent on at least one axis (or only the reserve is
    /// left and the last failure cannot be escalated): the issue is held
    /// and refused dispatch until an operator clears its attempts.
    pub exhausted: bool,
    pub hold_reason: Option<String>,
}

pub fn evaluate(budget: &IssueBudget, usage: &IssueBudgetUsage) -> IssueBudgetStatus {
    let reserve = budget.escalation_reserve_attempts();
    let elapsed_minutes = (usage.elapsed_seconds / 60.0).round() as u32;
    let attempts_left = budget
        .max_attempts
        .map(|max| max.saturating_sub(usage.attempts));
    let escalation_only = attempts_left.is_some_and(|left| left > 0 && left <= reserve);
    let mut hold_reason = None;
    if let Some(max) = budget.max_attempts {
        if usage.attempts >= max {
            hold_reason = Some(format!(
                "issue budget spent: {}/{max} attempts",
                usage.attempts
            ));
        }
    }
    if let (None, Some(max)) = (&hold_reason, budget.max_elapsed_minutes) {
        if usage.elapsed_seconds >= f64::from(max) * 60.0 {
            hold_reason = Some(format!(
                "issue budget spent: {elapsed_minutes}/{max} minutes elapsed"
            ));
        }
    }
    if let (None, Some(max)) = (&hold_reason, budget.max_manager_rounds) {
        if usage.manager_rounds >= max {
            hold_reason = Some(format!(
                "issue budget spent: {}/{max} manager rounds",
                usage.manager_rounds
            ));
        }
    }
    if hold_reason.is_none() && escalation_only {
        if let Some(class) = usage
            .last_failure_class
            .as_deref()
            .filter(|class| !crate::controller::is_genuine_agent_failure(class))
        {
            hold_reason = Some(format!(
                "issue budget: {} attempt(s) left, reserved for escalation; last failure ({class}) is not escalatable",
                attempts_left.unwrap_or_default()
            ));
        }
    }
    IssueBudgetStatus {
        attempts_used: usage.attempts,
        max_attempts: budget.max_attempts,
        escalation_reserve_attempts: reserve,
        elapsed_minutes,
        max_elapsed_minutes: budget.max_elapsed_minutes,
        manager_rounds_used: usage.manager_rounds,
        max_manager_rounds: budget.max_manager_rounds,
        escalation_only,
        exhausted: hold_reason.is_some(),
        hold_reason,
    }
}

/// Attach a budget status to every work id (and alias) with recorded
/// usage. `work_ids` is the ledger's work-id index key set.
pub(super) fn project<'a>(
    budget: &IssueBudget,
    entries: &[LedgerEntry],
    profile_name: &str,
    profile: &Profile,
    work_ids: impl Iterator<Item = &'a String>,
    evidence: &mut BTreeMap<String, WorkWaypointEvidence>,
) {
    let mut projected: BTreeMap<String, IssueBudgetStatus> = BTreeMap::new();
    for work_id in work_ids {
        if projected.contains_key(work_id) {
            continue;
        }
        let usage = issue_budget_usage(entries, profile_name, profile, work_id);
        if usage == IssueBudgetUsage::default() {
            continue;
        }
        let status = evaluate(budget, &usage);
        for alias in crate::ledger::work_id_aliases(work_id) {
            projected.insert(alias, status.clone());
        }
    }
    for (work_id, status) in projected {
        evidence.entry(work_id).or_default().issue_budget = Some(status);
    }
}

/// Text lines for the "Issue budgets" section; empty when nothing is
/// projected. Each issue is printed once, under its first alias.
pub(super) fn status_lines(evidence: &BTreeMap<String, WorkWaypointEvidence>) -> Vec<String> {
    let mut lines = Vec::new();
    for (work_id, item) in evidence {
        let Some(budget) = item.issue_budget.as_ref() else {
            continue;
        };
        let duplicate = crate::ledger::work_id_aliases(work_id)
            .iter()
            .any(|alias| alias < work_id && evidence.contains_key(alias));
        if duplicate {
            continue;
        }
        if lines.is_empty() {
            lines.push("Issue budgets:".to_string());
        }
        lines.push(format!("  - {work_id}: {}", describe(budget)));
    }
    lines
}

fn describe(budget: &IssueBudgetStatus) -> String {
    let axis = |used: u32, max: Option<u32>| match max {
        Some(max) => format!("{used}/{max}"),
        None => format!("{used}/unlimited"),
    };
    let mut text = format!(
        "attempts {}",
        axis(budget.attempts_used, budget.max_attempts)
    );
    if budget.max_attempts.is_some() && budget.escalation_reserve_attempts > 0 {
        text.push_str(&format!(
            " ({} reserved for escalation)",
            budget.escalation_reserve_attempts
        ));
    }
    text.push_str(&format!(
        ", elapsed {} min, manager rounds {}",
        axis(budget.elapsed_minutes, budget.max_elapsed_minutes),
        axis(budget.manager_rounds_used, budget.max_manager_rounds)
    ));
    if let Some(reason) = budget.hold_reason.as_deref() {
        text.push_str("; HELD: ");
        text.push_str(reason);
    } else if budget.escalation_only {
        text.push_str("; escalation only");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{evaluate, status_lines, IssueBudgetStatus};
    use crate::config::IssueBudget;
    use crate::dispatch::IssueBudgetUsage;
    use crate::status::WorkWaypointEvidence;
    use std::collections::BTreeMap;

    fn budget() -> IssueBudget {
        IssueBudget {
            max_attempts: Some(3),
            max_elapsed_minutes: Some(90),
            max_manager_rounds: Some(2),
            escalation_reserve_attempts: None,
        }
    }

    fn cfg_with_budget(tmp: &tempfile::TempDir, budget: &str) -> crate::config::GahConfig {
        let path = tmp.path().join("cfg.toml");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("docs/tickets")).unwrap();
        std::fs::write(
            repo.join("docs/tickets/TICKET-300-test.md"),
            "# TICKET-300: Test ticket\n\nGoal: test\n",
        )
        .unwrap();
        std::fs::write(
            &path,
            format!(
                "[profiles.test]\ndisplay_name = \"Test\"\nrepo_id = \"test/test\"\nprovider = \"\"\nrepo = \"test/test\"\nlocal_path = \"{}\"\nartifact_root = \"/tmp\"\ndefault_target_branch = \"main\"\n{budget}",
                repo.display()
            ),
        )
        .unwrap();
        let mut cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();
        cfg.defaults.artifact_root = tmp.path().to_string_lossy().into_owned();
        cfg
    }

    fn record_setup_failure(cfg: &crate::config::GahConfig, work_id: &str) {
        let mut entry = crate::ledger::LedgerEntry::new(
            "test",
            &cfg.profiles["test"],
            "codex",
            "improve",
            "docs/tickets/TICKET-300-test.md",
            None,
            None,
        );
        entry.work_id = Some(work_id.into());
        entry.failure_class = Some("environment_error".into());
        entry.failure_stage = Some("preflight".into());
        entry.duration_seconds = Some(60.0);
        crate::ledger::append(cfg, &entry).unwrap();
    }

    #[test]
    fn snapshot_holds_a_ticket_whose_budget_is_spent_and_shows_remaining_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_with_budget(
            &tmp,
            "[profiles.test.routing.issue_budget]\nmax_attempts = 2\nmax_elapsed_minutes = 90\n",
        );
        record_setup_failure(&cfg, "#300");
        let now = time::OffsetDateTime::now_utc();

        let snapshot = crate::status::build_snapshot(&cfg, "test", now).unwrap();
        let ticket = snapshot
            .available_tickets
            .iter()
            .find(|ticket| ticket.work_id.as_deref() == Some("TICKET-300"))
            .expect("ticket is scanned");
        assert!(
            ticket.human_required,
            "only the reserve is left and an infra failure cannot use it"
        );
        let budget = snapshot.work_waypoint_evidence["TICKET-300"]
            .issue_budget
            .as_ref()
            .expect("budget projected under the ticket alias");
        assert_eq!(budget.attempts_used, 1);
        assert_eq!(budget.elapsed_minutes, 1);
        assert!(budget.escalation_only);
        assert!(budget.exhausted, "an infra failure cannot use the reserve");

        record_setup_failure(&cfg, "TICKET-300");
        let snapshot = crate::status::build_snapshot(&cfg, "test", now).unwrap();
        let ticket = snapshot
            .available_tickets
            .iter()
            .find(|ticket| ticket.work_id.as_deref() == Some("TICKET-300"))
            .unwrap();
        assert!(ticket.human_required);
        assert_eq!(
            ticket.human_required_reason_code.as_deref(),
            Some("retry_budget_exhausted")
        );
        let blocker = snapshot
            .blocked_work_items
            .iter()
            .find(|blocker| blocker.source_reference.as_deref() == Some("TICKET-300"))
            .expect("held ticket is listed");
        assert_eq!(
            blocker.message.as_deref(),
            Some("issue budget spent: 2/2 attempts")
        );
        assert!(blocker.remediation_plan.is_some());
        let lines = status_lines(&snapshot.work_waypoint_evidence);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[1].starts_with("  - #300: attempts 2/2"), "{lines:?}");
        assert!(lines[1].contains("HELD: issue budget spent"), "{lines:?}");
    }

    #[test]
    fn profiles_without_a_budget_project_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_with_budget(&tmp, "");
        record_setup_failure(&cfg, "#300");
        record_setup_failure(&cfg, "#300");

        let snapshot =
            crate::status::build_snapshot(&cfg, "test", time::OffsetDateTime::now_utc()).unwrap();

        assert!(snapshot
            .work_waypoint_evidence
            .values()
            .all(|evidence| evidence.issue_budget.is_none()));
        assert!(snapshot.blocked_work_items.is_empty());
        assert!(status_lines(&snapshot.work_waypoint_evidence).is_empty());
    }

    #[test]
    fn under_budget_is_neither_held_nor_reserved() {
        let status = evaluate(
            &budget(),
            &IssueBudgetUsage {
                attempts: 1,
                elapsed_seconds: 600.0,
                manager_rounds: 1,
                last_failure_class: Some("backend_error".into()),
            },
        );
        assert!(!status.exhausted);
        assert!(!status.escalation_only);
        assert_eq!(status.elapsed_minutes, 10);
        assert_eq!(status.hold_reason, None);
    }

    #[test]
    fn every_axis_can_spend_the_budget() {
        let attempts = evaluate(
            &budget(),
            &IssueBudgetUsage {
                attempts: 3,
                ..IssueBudgetUsage::default()
            },
        );
        assert!(attempts.exhausted);
        assert_eq!(
            attempts.hold_reason.as_deref(),
            Some("issue budget spent: 3/3 attempts")
        );
        let elapsed = evaluate(
            &budget(),
            &IssueBudgetUsage {
                elapsed_seconds: 90.0 * 60.0,
                ..IssueBudgetUsage::default()
            },
        );
        assert_eq!(
            elapsed.hold_reason.as_deref(),
            Some("issue budget spent: 90/90 minutes elapsed")
        );
        let rounds = evaluate(
            &budget(),
            &IssueBudgetUsage {
                manager_rounds: 2,
                ..IssueBudgetUsage::default()
            },
        );
        assert_eq!(
            rounds.hold_reason.as_deref(),
            Some("issue budget spent: 2/2 manager rounds")
        );
    }

    #[test]
    fn reserve_allows_escalation_but_holds_an_infra_failure() {
        let escalatable = evaluate(
            &budget(),
            &IssueBudgetUsage {
                attempts: 2,
                last_failure_class: Some("agent_failure".into()),
                ..IssueBudgetUsage::default()
            },
        );
        assert!(escalatable.escalation_only);
        assert!(!escalatable.exhausted);

        let infra = evaluate(
            &budget(),
            &IssueBudgetUsage {
                attempts: 2,
                last_failure_class: Some("harness_error".into()),
                ..IssueBudgetUsage::default()
            },
        );
        assert!(infra.escalation_only);
        assert!(infra.exhausted);
        assert!(infra
            .hold_reason
            .as_deref()
            .unwrap()
            .contains("reserved for escalation"));

        let zero_reserve = evaluate(
            &IssueBudget {
                escalation_reserve_attempts: Some(0),
                ..budget()
            },
            &IssueBudgetUsage {
                attempts: 2,
                last_failure_class: Some("harness_error".into()),
                ..IssueBudgetUsage::default()
            },
        );
        assert!(!zero_reserve.escalation_only);
        assert!(!zero_reserve.exhausted);
    }

    #[test]
    fn status_lines_print_each_issue_once_with_remaining_budget() {
        let status = IssueBudgetStatus {
            attempts_used: 1,
            max_attempts: Some(3),
            escalation_reserve_attempts: 1,
            elapsed_minutes: 12,
            max_elapsed_minutes: None,
            manager_rounds_used: 0,
            max_manager_rounds: Some(2),
            escalation_only: false,
            exhausted: false,
            hold_reason: None,
        };
        let mut evidence = BTreeMap::new();
        for alias in ["#42", "TICKET-42"] {
            evidence.insert(
                alias.to_string(),
                WorkWaypointEvidence {
                    issue_budget: Some(status.clone()),
                    ..WorkWaypointEvidence::default()
                },
            );
        }
        evidence.insert("TICKET-9".into(), WorkWaypointEvidence::default());
        let lines = status_lines(&evidence);
        assert_eq!(
            lines,
            vec![
                "Issue budgets:".to_string(),
                "  - #42: attempts 1/3 (1 reserved for escalation), elapsed 12/unlimited min, manager rounds 0/2".to_string(),
            ]
        );
        assert!(status_lines(&BTreeMap::new()).is_empty());
    }
}
