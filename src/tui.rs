//! `gah tui` -- interactive terminal UI. Owns the terminal lifecycle, the
//! crossterm event loop, and all rendering. Key handling and screen/modal
//! state live in `tui_state` (pure, unit-tested there); this file is the
//! I/O shell around it.
//!
//! v1 intentionally does not let a user pick an arbitrary action for an
//! arbitrary row -- it shows exactly what `controller::decide_next_action`
//! would already do and lets a human approve or decline it. See
//! docs/MANAGER_MEMORY.md "Stretch Goal -- Optional Operator TUI": picking
//! an arbitrary action ("override next action") is explicitly Future scope.

use crate::config::GahConfig;
use crate::tui_state::{AppState, Screen, TuiCommand};
use anyhow::{anyhow, Result};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Cell, Clear, List, ListItem, Paragraph, Row, Table};
use ratatui::{DefaultTerminal, Frame};
use time::OffsetDateTime;

pub fn run(cfg: &GahConfig, profile: Option<&str>) -> Result<()> {
    if let Some(name) = profile {
        crate::config::get_profile(cfg, name)?; // fail fast, before touching the terminal
    }

    let mut profile_names: Vec<String> = cfg.profiles.keys().cloned().collect();
    profile_names.sort();

    let mut state = AppState::new(profile_names, profile.map(str::to_string));

    let mut terminal = ratatui::try_init()
        .map_err(|e| anyhow!("gah tui requires an interactive terminal: {e}"))?;

    if let Some(name) = state.profile_name.clone() {
        do_refresh(cfg, &mut state, &name);
    }

    let result = event_loop(cfg, &mut terminal, &mut state);
    ratatui::restore();
    result
}

fn event_loop(cfg: &GahConfig, terminal: &mut DefaultTerminal, state: &mut AppState) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, cfg, state))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(cmd) = crate::tui_state::handle_key(state, key) {
                    match cmd {
                        TuiCommand::Quit => break,
                        TuiCommand::Refresh => {
                            if let Some(name) = state.profile_name.clone() {
                                do_refresh(cfg, state, &name);
                            }
                        }
                        TuiCommand::SelectProfile(name) => do_refresh(cfg, state, &name),
                        TuiCommand::ExecuteConfirmedAction => {
                            run_confirmed_action(cfg, terminal, state)?;
                        }
                    }
                }
                if state.should_quit {
                    break;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn do_refresh(cfg: &GahConfig, state: &mut AppState, profile_name: &str) {
    let now = OffsetDateTime::now_utc();
    match crate::status::build_snapshot(cfg, profile_name, now) {
        Ok(snapshot) => {
            state.snapshot = Some(snapshot);
            state.status_line = "refreshed".into();
        }
        Err(e) => state.status_line = format!("refresh failed: {e:#}"),
    }
    // non-fatal on error; the events panel is supplementary
    if let Ok(mut events) = crate::events::read_events(cfg) {
        events.retain(|e| e.profile.as_deref() == Some(profile_name));
        let start = events.len().saturating_sub(20);
        state.events = events[start..].to_vec();
    }
}

fn run_confirmed_action(
    cfg: &GahConfig,
    terminal: &mut DefaultTerminal,
    state: &mut AppState,
) -> Result<()> {
    let profile_name = state
        .profile_name
        .clone()
        .expect("dashboard implies a profile is selected");
    let snapshot = state
        .snapshot
        .as_ref()
        .expect("dashboard implies a snapshot is loaded");
    let action = crate::controller::decide_next_action(snapshot);

    ratatui::restore();
    println!("\nRunning: {} -- {}\n", action.kind(), action.reason());
    let outcome = crate::controller::execute_action(cfg, &profile_name, &action, false);
    match &outcome {
        Ok(msg) => println!("\n{msg}"),
        Err(e) => println!("\nError: {e:#}"),
    }
    println!("\nPress Enter to return to gah tui...");
    let mut discard = String::new();
    let _ = std::io::stdin().read_line(&mut discard);

    *terminal = ratatui::try_init().map_err(|e| anyhow!("failed to resume terminal: {e}"))?;

    state.status_line = match outcome {
        Ok(m) => m,
        Err(e) => format!("error: {e:#}"),
    };
    do_refresh(cfg, state, &profile_name);
    Ok(())
}

fn render(frame: &mut Frame, cfg: &GahConfig, state: &AppState) {
    match state.screen {
        Screen::ProfilePicker => render_profile_picker(frame, cfg, state),
        Screen::Dashboard => render_dashboard(frame, state),
        Screen::EventsView => render_events_view(frame, state),
    }
}

fn render_profile_picker(frame: &mut Frame, cfg: &GahConfig, state: &AppState) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

    if state.profile_names.is_empty() {
        frame.render_widget(
            Paragraph::new("No profiles found in gah-config.toml")
                .block(Block::bordered().title("gah tui -- select profile")),
            body,
        );
    } else {
        let items: Vec<ListItem> = state
            .profile_names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let label = match crate::config::get_profile(cfg, name) {
                    Ok(profile) => {
                        format!("{name}  {} ({})", profile.display_name, profile.provider)
                    }
                    Err(_) => name.clone(),
                };
                let style = if i == state.profile_selected {
                    Style::default().bg(Color::Blue)
                } else {
                    Style::default()
                };
                ListItem::new(label).style(style)
            })
            .collect();
        frame.render_widget(
            List::new(items).block(Block::bordered().title("gah tui -- select profile")),
            body,
        );
    }
    frame.render_widget(
        Paragraph::new("\u{2191}/\u{2193} move  Enter select  q quit"),
        footer,
    );
}

fn render_dashboard(frame: &mut Frame, state: &AppState) {
    let [banner, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let banner_text = match &state.snapshot {
        Some(snapshot) => {
            let action = crate::controller::decide_next_action(snapshot);
            format!("{}: {}", action.kind(), action.reason())
        }
        None => "No snapshot loaded yet -- press r to refresh".to_string(),
    };
    frame.render_widget(
        Paragraph::new(banner_text).block(Block::bordered().title("Next action")),
        banner,
    );

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(body);
    let [mr_area, ticket_area, avail_area] = Layout::vertical([
        Constraint::Percentage(40),
        Constraint::Percentage(40),
        Constraint::Percentage(20),
    ])
    .areas(left);
    let [ledger_area, events_area] =
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(right);

    render_mr_table(frame, state, mr_area);
    render_ticket_table(frame, state, ticket_area);
    render_availability_table(frame, state, avail_area);
    render_ledger_panel(frame, state, ledger_area);
    render_events_tail(frame, state, events_area);

    let hotkeys = "r refresh  a confirm+run next action  e events  q quit";
    frame.render_widget(
        Paragraph::new(format!("{hotkeys}   [{}]", state.status_line)),
        footer,
    );

    if state.confirm_open {
        render_confirm_popup(frame, state);
    }
}

fn render_mr_table(frame: &mut Frame, state: &AppState, area: Rect) {
    let rows: Vec<Row> = state
        .snapshot
        .iter()
        .flat_map(|s| s.merge_requests.iter())
        .map(|mr| {
            Row::new(vec![
                Cell::from(mr.branch.clone()),
                Cell::from(mr.classification.clone()),
                Cell::from(mr.state.clone().unwrap_or_default()),
                Cell::from(mr.url.clone().unwrap_or_default()),
            ])
        })
        .collect();
    let widths = [
        Constraint::Percentage(25),
        Constraint::Percentage(20),
        Constraint::Percentage(15),
        Constraint::Percentage(40),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(Row::new(vec!["Branch", "Classification", "State", "URL"]))
            .block(Block::bordered().title("Merge Requests")),
        area,
    );
}

fn render_ticket_table(frame: &mut Frame, state: &AppState, area: Rect) {
    let rows: Vec<Row> = state
        .snapshot
        .iter()
        .flat_map(|s| s.available_tickets.iter())
        .map(|t| {
            Row::new(vec![
                Cell::from(t.ticket_path.clone()),
                Cell::from(t.work_id.clone().unwrap_or_default()),
                Cell::from(t.prior_attempt_count.to_string()),
                Cell::from(t.last_failure_class.clone().unwrap_or_default()),
                Cell::from(if t.has_active_mr { "yes" } else { "no" }),
            ])
        })
        .collect();
    let widths = [
        Constraint::Percentage(35),
        Constraint::Percentage(20),
        Constraint::Percentage(10),
        Constraint::Percentage(20),
        Constraint::Percentage(15),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(Row::new(vec![
                "Ticket",
                "Work ID",
                "Attempts",
                "Last Failure",
                "Active MR",
            ]))
            .block(Block::bordered().title("Available Tickets")),
        area,
    );
}

fn render_availability_table(frame: &mut Frame, state: &AppState, area: Rect) {
    let rows: Vec<Row> = state
        .snapshot
        .iter()
        .flat_map(|s| s.availability.iter())
        .map(|a| {
            Row::new(vec![
                Cell::from(a.backend.clone()),
                Cell::from(a.model.clone().unwrap_or_default()),
                Cell::from(if a.eligible_now { "yes" } else { "no" }),
                Cell::from(a.reason.clone().unwrap_or_default()),
                Cell::from(a.unavailable_until.clone().unwrap_or_default()),
            ])
        })
        .collect();
    let widths = [
        Constraint::Percentage(20),
        Constraint::Percentage(20),
        Constraint::Percentage(15),
        Constraint::Percentage(25),
        Constraint::Percentage(20),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(Row::new(vec![
                "Backend", "Model", "Eligible", "Reason", "Until",
            ]))
            .block(Block::bordered().title("Availability")),
        area,
    );
}

fn render_ledger_panel(frame: &mut Frame, state: &AppState, area: Rect) {
    let text = match state
        .snapshot
        .as_ref()
        .and_then(|s| s.recent_ledger.as_ref())
    {
        Some(rl) => format!(
            "mode: {}\nbackend: {}\nmodel: {}\nvalidation: {}\nbranch: {}",
            rl.most_recent_mode,
            rl.most_recent_effective_backend,
            rl.most_recent_effective_model.clone().unwrap_or_default(),
            rl.most_recent_validation_result.clone().unwrap_or_default(),
            rl.most_recent_branch.clone().unwrap_or_default(),
        ),
        None => "No ledger entries yet".to_string(),
    };
    frame.render_widget(
        Paragraph::new(text).block(Block::bordered().title("Recent Dispatch")),
        area,
    );
}

fn render_events_tail(frame: &mut Frame, state: &AppState, area: Rect) {
    let items: Vec<ListItem> = state
        .events
        .iter()
        .rev()
        .take(8)
        .map(|e| ListItem::new(format!("{}  {}", e.timestamp, e.event_type)))
        .collect();
    frame.render_widget(
        List::new(items).block(Block::bordered().title("Recent Events")),
        area,
    );
}

fn render_events_view(frame: &mut Frame, state: &AppState) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    let title = format!(
        "Events -- {}",
        state.profile_name.as_deref().unwrap_or("(none)")
    );
    let items: Vec<ListItem> = state
        .events
        .iter()
        .map(|e| {
            ListItem::new(format!(
                "{}  {}  {}  {}",
                e.timestamp,
                e.event_type,
                e.work_id.clone().unwrap_or_default(),
                e.details
            ))
        })
        .collect();
    frame.render_widget(List::new(items).block(Block::bordered().title(title)), body);
    frame.render_widget(Paragraph::new("[b]ack  [q]uit"), footer);
}

fn render_confirm_popup(frame: &mut Frame, state: &AppState) {
    let area = centered_rect(frame.area(), 50, 20);
    let text = match &state.snapshot {
        Some(snapshot) => {
            let action = crate::controller::decide_next_action(snapshot);
            format!(
                "Execute {}: {}?\n\n[y]es / [n]o",
                action.kind(),
                action.reason()
            )
        }
        None => String::new(),
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text).block(Block::bordered().title("Confirm")),
        area,
    );
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let [_, vertical, _] = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas(area);
    let [_, horizontal, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(vertical);
    horizontal
}
