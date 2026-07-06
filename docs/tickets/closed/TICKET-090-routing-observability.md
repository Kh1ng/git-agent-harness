# TICKET-090: Routing observability

**Status:** [MERGED] — via MR !45

Goal: Expose enough structured detail to debug the routing policy and understand why a specific backend/model was selected.

Difficulty: easy
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Expose structured routing observability in status and ledger

## Problem

Current routing is opaque. The user sees `effective_backend` and `effective_model` in the ledger, but not:
- which candidates were considered
- which candidates were skipped and why
- whether policy reordered the default candidate order
- what quota pool and pace band influenced the decision

Without this, routing bugs are hard to diagnose and the policy cannot be validated.

## Acceptance Criteria

1. Ledger records the full candidate list considered
2. Each candidate carries skip reason where applicable
3. Routing records whether policy reordered the defaults
4. Quota pool and pace band are exposed where relevant
5. Cost class is exposed where relevant
6. Status snapshot includes routing diagnostic data
7. Human-readable output when available (no JSON required)
8. Backward compatible: historical entries without routing detail deserialize correctly

## Affected Files

- `src/ledger.rs` — Routing diagnostic fields
- `src/routing.rs` — Diagnostic collection
- `src/status.rs` — Diagnostic exposure

## Constraints

- Dependencies: TICKET-086, TICKET-087, TICKET-088, TICKET-089 must define the data to expose
- No CLI overhaul
- No dashboard/UI work
- No new commands unless data cannot be surfaced from existing commands

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
