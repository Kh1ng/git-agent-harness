//! Pure key-handling state machine for `gah tui`. No I/O, no `GahConfig`/
//! `Profile` types -- `tui.rs` owns the terminal, the event loop, and all
//! calls into `status`/`events`/`dispatch`. Kept separate specifically so
//! this logic is unit-testable without a real terminal (crossterm raw mode
//! needs a TTY; `assert_cmd`'s subprocess capture can't drive one).

use crate::controller::NextAction;
use crate::events::ControllerEvent;
use crate::status::StatusSnapshot;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    ProfilePicker,
    Dashboard,
    EventsView,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TuiCommand {
    Refresh,
    SelectProfile(String),
    ExecuteConfirmedAction,
    Quit,
}

pub struct AppState {
    pub screen: Screen,
    pub profile_names: Vec<String>,
    pub profile_selected: usize,
    pub profile_name: Option<String>,
    pub snapshot: Option<StatusSnapshot>,
    pub events: Vec<ControllerEvent>,
    pub confirm_open: bool,
    pub status_line: String,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(profile_names: Vec<String>, initial_profile: Option<String>) -> Self {
        let screen = if initial_profile.is_some() {
            Screen::Dashboard
        } else {
            Screen::ProfilePicker
        };
        Self {
            screen,
            profile_names,
            profile_selected: 0,
            profile_name: initial_profile,
            snapshot: None,
            events: Vec::new(),
            confirm_open: false,
            status_line: String::new(),
            should_quit: false,
        }
    }
}

/// Only these variants actually perform an automated action; `FixMr` runs a
/// dispatch, `MarkReadyForReview` flips provider state, and `WaitUntil`/
/// `HumanRequired`/`NoOp` are informational, so none of those should ever
/// reach a confirm prompt.
fn is_dispatchable(action: &NextAction) -> bool {
    matches!(
        action,
        NextAction::ReviewMr { .. }
            | NextAction::MarkReadyForReview { .. }
            | NextAction::FixMr { .. }
            | NextAction::DispatchTicket { .. }
            | NextAction::Retry { .. }
            | NextAction::Escalate { .. }
    )
}

/// Mutates only navigation/selection/modal state; any actual I/O
/// (rebuilding the snapshot, running the action, reading events) is
/// signaled via the returned `TuiCommand` for `tui.rs` to perform.
pub fn handle_key(state: &mut AppState, key: KeyEvent) -> Option<TuiCommand> {
    // Raw mode disables the terminal's own SIGINT delivery, so Ctrl+C
    // arrives as an ordinary KeyEvent -- without this it wouldn't kill
    // the app the way a user expects.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        state.should_quit = true;
        return Some(TuiCommand::Quit);
    }

    match state.screen {
        Screen::ProfilePicker => match key.code {
            KeyCode::Char('q') => {
                state.should_quit = true;
                Some(TuiCommand::Quit)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !state.profile_names.is_empty() {
                    state.profile_selected = state
                        .profile_selected
                        .checked_sub(1)
                        .unwrap_or(state.profile_names.len() - 1);
                }
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !state.profile_names.is_empty() {
                    state.profile_selected =
                        (state.profile_selected + 1) % state.profile_names.len();
                }
                None
            }
            KeyCode::Enter => {
                let name = state.profile_names.get(state.profile_selected)?.clone();
                state.profile_name = Some(name.clone());
                state.screen = Screen::Dashboard;
                Some(TuiCommand::SelectProfile(name))
            }
            _ => None,
        },
        Screen::Dashboard if state.confirm_open => match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                state.confirm_open = false;
                Some(TuiCommand::ExecuteConfirmedAction)
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.confirm_open = false;
                None
            }
            _ => None,
        },
        Screen::Dashboard => match key.code {
            KeyCode::Char('q') => {
                state.should_quit = true;
                Some(TuiCommand::Quit)
            }
            KeyCode::Char('r') => Some(TuiCommand::Refresh),
            KeyCode::Char('e') => {
                state.screen = Screen::EventsView;
                None
            }
            KeyCode::Char('a') => {
                if let Some(snapshot) = &state.snapshot {
                    if is_dispatchable(&crate::controller::decide_next_action(snapshot)) {
                        state.confirm_open = true;
                    } else {
                        state.status_line = "nothing to confirm for this action".into();
                    }
                }
                None
            }
            _ => None,
        },
        Screen::EventsView => match key.code {
            KeyCode::Char('q') => {
                state.should_quit = true;
                Some(TuiCommand::Quit)
            }
            KeyCode::Esc | KeyCode::Char('b') => {
                state.screen = Screen::Dashboard;
                None
            }
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AvailableTicket;
    use crate::status::{ObservationStatus, Observations, ProfileIdentity, StatusSnapshot};
    use crate::sync::{RecommendedAction, SyncMrJson};
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn empty_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            schema_version: 1,
            review_contract_version: crate::ledger::CURRENT_REVIEW_CONTRACT_VERSION,
            generated_at: "2026-07-05T00:00:00Z".into(),
            profile: ProfileIdentity {
                profile: "real".into(),
                display_name: "Real".into(),
                repo_id: "real".into(),
                provider: "github".into(),
                local_path: "/tmp/repo".into(),
                default_target_branch: "main".into(),
                merge_policy: crate::config::MergePolicy::default(),
                max_fix_attempts_per_mr: 2,
                max_implementation_failures_per_ticket: 8,
                max_open_managed_mrs: 1,
                issue_intake_policy: crate::models::IssueIntakePolicy {
                    mode: "canonical_autonomous_only".into(),
                    canonical_autonomous_label: "exec:autonomous".into(),
                    trusted_human_authors: vec![],
                    trusted_bot_authors: vec![],
                    github_issue_author_allowlist: vec![],
                },
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
            blocked_work_items: vec![],
            issue_intake_rejections: vec![],
            dependency_blockers: vec![],
            errors: vec![],
            available_tickets: vec![],
            active_claims: vec![],
            fix_attempt_counts: std::collections::HashMap::new(),
            merge_attempt_counts: std::collections::HashMap::new(),
            review_held_work_ids: std::collections::HashSet::new(),
            publishing_allow_pr: true,
            generated_artifact_deny_patterns: vec![],
            max_parallel_workers: 1,
            open_managed_mr_count: 0,
            inflight_implementation_count: 0,
            implementation_intake_paused: false,
            backend_configured: std::collections::HashMap::new(),
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
            merged_at: None,
            ci_passed: false,
            ci_pending: false,
            title: None,
            effective_backend: None,
            effective_model: None,
            review_verdict: None,
            review_gate_reason: None,
            classification: classification.into(),
            recommended_action: RecommendedAction::from_class(classification),
        }
    }

    fn ticket(path: &str, work_id: Option<&str>, prior_attempt_count: usize) -> AvailableTicket {
        AvailableTicket {
            ticket_path: path.into(),
            work_id: work_id.map(str::to_string),
            title: None,
            recommended_backend: None,
            recommended_model: None,
            prior_attempt_count,
            genuine_agent_failure_count: 0,
            last_failure_class: None,
            has_active_mr: false,
            human_required: false,
            human_required_reason_code: None,
            has_active_claim: false,
        }
    }

    fn state_with_profiles(names: &[&str]) -> AppState {
        AppState::new(names.iter().map(|s| s.to_string()).collect(), None)
    }

    #[test]
    fn profile_picker_up_wraps_from_zero_to_last() {
        let mut state = state_with_profiles(&["a", "b", "c"]);
        handle_key(&mut state, key(KeyCode::Up));
        assert_eq!(state.profile_selected, 2);
    }

    #[test]
    fn profile_picker_down_wraps_from_last_to_zero() {
        let mut state = state_with_profiles(&["a", "b", "c"]);
        state.profile_selected = 2;
        handle_key(&mut state, key(KeyCode::Down));
        assert_eq!(state.profile_selected, 0);
    }

    #[test]
    fn profile_picker_enter_emits_select_profile_and_switches_to_dashboard() {
        let mut state = state_with_profiles(&["a", "b"]);
        state.profile_selected = 1;
        let cmd = handle_key(&mut state, key(KeyCode::Enter));
        assert_eq!(cmd, Some(TuiCommand::SelectProfile("b".into())));
        assert_eq!(state.screen, Screen::Dashboard);
        assert_eq!(state.profile_name.as_deref(), Some("b"));
    }

    #[test]
    fn profile_picker_enter_is_noop_when_no_profiles() {
        let mut state = state_with_profiles(&[]);
        let cmd = handle_key(&mut state, key(KeyCode::Enter));
        assert_eq!(cmd, None);
        assert_eq!(state.screen, Screen::ProfilePicker);
    }

    #[test]
    fn profile_picker_q_emits_quit() {
        let mut state = state_with_profiles(&["a"]);
        let cmd = handle_key(&mut state, key(KeyCode::Char('q')));
        assert_eq!(cmd, Some(TuiCommand::Quit));
        assert!(state.should_quit);
    }

    #[test]
    fn ctrl_c_emits_quit_from_every_screen() {
        for screen in [Screen::ProfilePicker, Screen::Dashboard, Screen::EventsView] {
            let mut state = state_with_profiles(&["a"]);
            state.screen = screen;
            let cmd = handle_key(&mut state, ctrl_key('c'));
            assert_eq!(cmd, Some(TuiCommand::Quit), "screen {screen:?}");
        }
    }

    #[test]
    fn dashboard_r_emits_refresh() {
        let mut state = state_with_profiles(&[]);
        state.screen = Screen::Dashboard;
        let cmd = handle_key(&mut state, key(KeyCode::Char('r')));
        assert_eq!(cmd, Some(TuiCommand::Refresh));
    }

    fn dashboard_state_with_action(action_snapshot_mrs: Vec<SyncMrJson>) -> AppState {
        let mut state = state_with_profiles(&[]);
        state.screen = Screen::Dashboard;
        let mut snapshot = empty_snapshot();
        snapshot.merge_requests = action_snapshot_mrs;
        state.snapshot = Some(snapshot);
        state
    }

    #[test]
    fn dashboard_a_opens_confirm_for_dispatch_ticket_action() {
        let mut state = state_with_profiles(&[]);
        state.screen = Screen::Dashboard;
        let mut snapshot = empty_snapshot();
        snapshot
            .available_tickets
            .push(ticket("docs/tickets/T-1.md", Some("TICKET-1"), 0));
        state.snapshot = Some(snapshot);
        let cmd = handle_key(&mut state, key(KeyCode::Char('a')));
        assert_eq!(cmd, None);
        assert!(state.confirm_open);
    }

    #[test]
    fn dashboard_a_opens_confirm_for_review_mr_action() {
        let mut state = dashboard_state_with_action(vec![mr("gah/x", "NEEDS_REVIEW")]);
        handle_key(&mut state, key(KeyCode::Char('a')));
        assert!(state.confirm_open);
    }

    #[test]
    fn dashboard_a_opens_confirm_for_fix_mr_action() {
        let mut state = dashboard_state_with_action(vec![mr("gah/x", "CI_FAILED")]);
        handle_key(&mut state, key(KeyCode::Char('a')));
        assert!(state.confirm_open);
    }

    #[test]
    fn dashboard_a_does_not_open_confirm_for_wait_human_or_noop() {
        // No MRs, no tickets, no availability -> decide_next_action is NoOp.
        let mut state = dashboard_state_with_action(vec![]);
        handle_key(&mut state, key(KeyCode::Char('a')));
        assert!(!state.confirm_open);
        assert_eq!(state.status_line, "nothing to confirm for this action");
    }

    #[test]
    fn dashboard_a_is_noop_before_first_refresh_no_snapshot() {
        let mut state = state_with_profiles(&[]);
        state.screen = Screen::Dashboard;
        let cmd = handle_key(&mut state, key(KeyCode::Char('a')));
        assert_eq!(cmd, None);
        assert!(!state.confirm_open);
    }

    #[test]
    fn confirm_modal_y_emits_execute_confirmed_action_and_closes_modal() {
        let mut state = dashboard_state_with_action(vec![mr("gah/x", "NEEDS_REVIEW")]);
        state.confirm_open = true;
        let cmd = handle_key(&mut state, key(KeyCode::Char('y')));
        assert_eq!(cmd, Some(TuiCommand::ExecuteConfirmedAction));
        assert!(!state.confirm_open);
    }

    #[test]
    fn confirm_modal_n_cancels_without_command() {
        let mut state = dashboard_state_with_action(vec![mr("gah/x", "NEEDS_REVIEW")]);
        state.confirm_open = true;
        let cmd = handle_key(&mut state, key(KeyCode::Char('n')));
        assert_eq!(cmd, None);
        assert!(!state.confirm_open);
    }

    #[test]
    fn confirm_modal_esc_cancels_without_command() {
        let mut state = dashboard_state_with_action(vec![mr("gah/x", "NEEDS_REVIEW")]);
        state.confirm_open = true;
        let cmd = handle_key(&mut state, key(KeyCode::Esc));
        assert_eq!(cmd, None);
        assert!(!state.confirm_open);
    }

    #[test]
    fn dashboard_e_switches_to_events_view() {
        let mut state = state_with_profiles(&[]);
        state.screen = Screen::Dashboard;
        handle_key(&mut state, key(KeyCode::Char('e')));
        assert_eq!(state.screen, Screen::EventsView);
    }

    #[test]
    fn events_view_esc_returns_to_dashboard() {
        let mut state = state_with_profiles(&[]);
        state.screen = Screen::EventsView;
        handle_key(&mut state, key(KeyCode::Esc));
        assert_eq!(state.screen, Screen::Dashboard);
    }

    #[test]
    fn events_view_b_returns_to_dashboard() {
        let mut state = state_with_profiles(&[]);
        state.screen = Screen::EventsView;
        handle_key(&mut state, key(KeyCode::Char('b')));
        assert_eq!(state.screen, Screen::Dashboard);
    }

    #[test]
    fn events_view_q_emits_quit() {
        let mut state = state_with_profiles(&[]);
        state.screen = Screen::EventsView;
        let cmd = handle_key(&mut state, key(KeyCode::Char('q')));
        assert_eq!(cmd, Some(TuiCommand::Quit));
    }
}
