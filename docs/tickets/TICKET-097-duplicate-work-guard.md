# TICKET-097: Duplicate-work guard foundation

Goal: Use durable work identity to detect active ownership — preventing duplicate dispatches for the same ticket.

Difficulty: medium
Risk: medium
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Build duplicate-work guard using durable work identity

## Problem

Dispatch currently has weak duplicate-work detection:
- `lookup_review_state` uses fragile target_summary string matching
- Branch names encode no work identity
- There is no way to ask "is this ticket already being worked on?"
- Two dispatches for the same ticket could produce concurrent branches with different IDs

TICKET-080 (duplicate-work guard) is already in the roadmap. This ticket provides the foundation using the work identity from TICKET-091/095.

## Acceptance Criteria

1. Before dispatch, check if the ticket's work_id already has an active ledger entry
2. Active = open MR in progress (not merged, not closed unmerged)
3. If active entry found, refuse dispatch with a clear message
4. Distinguish:
   - active open PR owns work → block
   - active branch may own work → warn
   - merged work → complete, allow new dispatch if appropriate
   - closed unmerged work → terminal but not active, allow new dispatch
   - stale historical work → must not block new work
5. Check covers fix, improve, and experiment modes
6. Review mode is not guarded (independent review is safe)

## Affected Files

- `src/dispatch.rs` — Pre-dispatch duplicate check
- `src/ledger.rs` — Query for active work by ID

## Constraints

- Dependencies: TICKET-091, TICKET-095 (work identity in ledger)
- Do not implement the full loop guard (TICKET-080) here
- Do not change provider code
- Do not add a database
- Do not require sync to be called before dispatch

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
