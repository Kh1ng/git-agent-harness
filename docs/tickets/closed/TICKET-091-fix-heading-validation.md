# TICKET-091-FIX: Fix heading validation and ? propagation

Goal: Fix two bugs from TICKET-091 review:
1. parse_ticket_metadata changed .ok().flatten() to ? in improve/experiment — revert to .ok().flatten() so ticket files with # Goal headings don't hard-abort dispatch
2. The heading-vs-filename validation should warn on mismatch, not error

Difficulty: easy
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4

## Affected Files
- src/dispatch.rs

## Acceptance Criteria
1. Ticket files with # Goal as first heading can be dispatched without error
2. Heading/filename mismatch is logged as a warning, not a hard error
3. .ok().flatten() restored for improve/experiment parse_ticket_metadata calls
4. All existing tests pass
5. Review/review routing paths also consistent

## Verification Commands
- cargo fmt --check
- cargo test
