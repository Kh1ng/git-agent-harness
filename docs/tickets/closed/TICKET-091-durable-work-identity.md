# TICKET-091: Durable unique work item identity

Goal: Introduce or formalize a stable unique work ID that flows through dispatch, ledger, branch, and sync — not extracted from arbitrary prose.

Difficulty: hard
Risk: high
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Formalize durable unique work item identity for dispatch and ledger

## Problem

Ticket IDs are currently derived from:
- filename parsing (`TICKET-{number}` from `TICKET-NNN-slug.md`)
- manager memory prose
- PR title strings
- prompt text

None of these are authoritative or collision-free. TICKET-074 was used for two entirely different features (closed-unmerged fix in PR #17 vs baseline disposition classifier in manager memory).

The ledger has no dedicated `work_id` field — only `target_summary` (a file path string).

Branch names encode no work identity (`gah/{repo_id}-{timestamp}`).

## Acceptance Criteria

1. Define a stable work identity concept. Consider:
   - manager-assigned sequential IDs (existing `TICKET-NNN` convention with collision defense)
   - UUID/ULID for internal entries without an external ticket file
   - external issue ID when synced from provider
2. Work ID persists through dispatch and ledger as a typed field (not a string-freeform)
3. Backward compatible for historical ledger entries (deserialize to None/unknown)
4. Explicit behavior when no external ticket ID exists (generate internal ID)
5. Ticket file always carries the authoritative ID — never diverges from filename
6. Collision detection: `next_ticket_id` must check manager memory as well as file system
7. No collision with stale manager memory entries

## Affected Files

- `src/dispatch.rs` — TicketMetadata, next_ticket_id
- `src/ledger.rs` — New work_id field
- `docs/MANAGER_MEMORY.md` — Collision rule

## Constraints

- Do not redesign the entire ticket system
- Do not require a database
- Backward compatible at the JSONL level
- The identifier must fit within ledger, sync, and provider metadata fields

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
