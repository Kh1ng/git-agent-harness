# TICKET-093: Authoritative PR title generation

Goal: Generate PR title from structured work metadata rather than stale ticket numbers or arbitrary prompt text.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Derive PR titles from authoritative structured work metadata

## Problem

Current `build_mr_title()` constructs the title from:
- `ticket.ticket_id` (from filename `TICKET-NNN`)
- `ticket.title` (from first markdown heading or Goal: line)

This embeds TICKET-NNN in the PR title. When TICKET-074 was used for the closed-unmerged fix but was already reserved for baseline disposition, the PR title propagated the collision.

The title `[GAH] Fix: TICKET-074 Fix closed unmerged MR classification` contains a stale/invalid ticket number relative to manager memory.

## Acceptance Criteria

1. PR title is derived from structured work metadata, not raw filename parsing
2. Work ID included only when the ID is authoritative (no stale/duplicate IDs in title)
3. Preserve mode prefix: `[GAH] Fix:`, `[GAH] Improve:`, etc.
4. Preserve `[GAH][DRAFT-FAIL]` prefix when validation failed (existing semantics)
5. Deterministic fallback when title metadata is absent (use mode + repo_id, current behavior)
6. Length handling: titles beyond provider limits are truncated gracefully
7. Tests for:
   - collision detection preventing stale ID from appearing in title
   - missing metadata fallback
   - draft-fail prefix preservation
   - normal title generation from valid metadata

## Affected Files

- `src/dispatch.rs` — build_mr_title, render_ticket_label

## Constraints

- Dependencies: TICKET-091, TICKET-092 (work identity and structured metadata)
- Do not change provider code
- Do not change sync or status
- Do not rename existing CLI flags

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
