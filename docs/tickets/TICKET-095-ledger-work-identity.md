# TICKET-095: Ledger work identity propagation

Goal: Persist stable work identity and core metadata in ledger entries.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Propagate durable work identity through ledger entries

## Problem

The ledger has no dedicated work/ticket identity field. `target_summary` stores the file path of the ticket (e.g., `docs/tickets/TICKET-074-fix...md`) but this is:
- not a stable ID
- not useful for matching an MR back to work
- fragile across filesystem moves or path changes
- not machine-friendly for cross-referencing

Without work identity in the ledger, downstream consumers (sync, status, duplicate-work guard) must use heuristic string matching.

## Acceptance Criteria

1. LedgerEntry gains a dedicated `work_id` field (Option<String>)
2. LedgerEntry gains optional `work_title` field
3. `work_id` is populated from authoritative ticket files (TICKET-NNN) during dispatch
4. Experiments and candidate-based dispatches without ticket files generate a synthetic ID or leave None
5. `mr_url` and `branch` remain in the ledger for cross-referencing
6. Backward compatible: historical entries (pre-091) deserialize with None
7. Status snapshot exposes work_id from the most recent ledger entry
8. Sync output does NOT gain a work_id field (deferred to TICKET-096)

## Affected Files

- `src/ledger.rs` — New fields, serialization
- `src/dispatch.rs` — Populate work_id during dispatch

## Constraints

- Dependencies: TICKET-091 (work identity concept)
- Do not change sync output format
- Do not change provider code
- Do not require database migration

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
