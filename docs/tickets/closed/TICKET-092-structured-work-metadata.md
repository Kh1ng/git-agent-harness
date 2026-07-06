# TICKET-092: Structured work metadata

Goal: Represent task metadata as typed structured fields rather than prompt parsing.

Difficulty: medium
Risk: medium
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Replace freeform prompt metadata with typed structured work fields

## Problem

The current `TicketMetadata` struct covers basic fields (ticket_id, title, difficulty, risk, backend/model) but the struct is not serialized independently of the ticket file. Metadata is re-parsed from markdown every dispatch. There is no structured representation for:
- summary/problem description (beyond the heading)
- acceptance criteria (parsed from prose, not typed)
- constraints
- source/authority

The PM mode's `PmPlanTicket` has richer fields (title, summary, difficulty, risk, acceptance_criteria, affected_files, verification_commands) but this struct is PM-only. The fix-mode `TicketMetadata` is a subset with different names.

## Acceptance Criteria

1. Define a single structured metadata type that serves planning, fix, improve, experiment, and review modes
2. Fields: work_id, title, summary/problem, acceptance_criteria, constraints, recommended_backend, recommended_model, risk, difficulty, source, affected_files, verification_commands
3. Ticket file format remains markdown (human-editable) but struct is authoritative when present
4. Missing fields handled explicitly (not silently defaulted)
5. Backward compatible: existing ticket files without new fields still parse
6. Avoid duplicating full prompt text when structured fields suffice

## Affected Files

- `src/dispatch.rs` — TicketMetadata → unified struct, parsing
- `src/models.rs` or new struct location

## Constraints

- Dependencies: TICKET-091 (work identity must exist first)
- Do not require a new file format
- Do not break existing ticket files
- No database

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
