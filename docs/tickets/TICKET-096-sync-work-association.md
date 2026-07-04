# TICKET-096: Sync/reconciliation association by work identity

Goal: Allow sync/outcome reconciliation to associate provider PR/MR state with one logical work item using structured identity, not branch string heuristics.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Associate sync output with work items using structured identity

## Problem

Sync output currently has no work/task identity. Reconciliation relies on:
- branch name heuristics (`gah/gah-1783134254`)
- PR title string parsing
- stale manager memory text

This is fragile. After TICKET-091 and TICKET-095, the ledger has work identity. Sync output should carry enough information to match an MR to its originating ticket.

## Acceptance Criteria

1. Sync output gains an optional `work_id` field where available
2. Work ID is populated from the PR title where the title contains an authoritative work ID
3. Where no work ID exists in sync output, reconciliation falls back to branch matching against the ledger
4. Terminal states (MERGED, CLOSED_UNMERGED) can be traced to one work item via the ledger
5. A new reconciliation field or sub-structure is NOT required — ledger queries are sufficient
6. Backward compatible: sync output without work_id still functions
7. No changes to provider API calls

## Affected Files

- `src/sync.rs` — Optional work_id field in SyncMr
- `src/ledger.rs` — Query helper for matching sync entries to work

## Constraints

- Dependencies: TICKET-091, TICKET-095 (work identity in ledger)
- Do not change provider code
- Do not restructure sync output (add field, don't reshape)
- Do not require a database

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
