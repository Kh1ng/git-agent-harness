# Manager Memory — GAH Agent-Harness

**Source of truth for manager agents dispatching against GAH itself.**
Update this file when ticket status changes or decisions are made.

## Active Mission

Improve GAH itself — MR title quality, review automation.

## Tickets

| Ticket | Title | Status | Backend | Notes |
|--------|-------|--------|---------|-------|
| TICKET-058 | Descriptive MR Titles | BACKLOG | codex | Parse ticket title from h1 heading, use in MR title |
| TICKET-059 | Automated Review Trigger | BACKLOG | codex | Post review verdict to MR thread + auto-label |

## Merge Policy

- `cargo build --quiet` must pass before merge
- Review all MRs before merging into main
- Manual `cargo build --release && sudo -S -p '' cp target/release/gah /usr/local/bin/gah` after merge
