# TICKET-111: Explicit red-baseline policy

Status: COMPLETE

Renumbered from TICKET-075 on 2026-07-05 (depends on TICKET-110, itself renumbered from
TICKET-074 — see that ticket for the collision this avoids).

Difficulty: medium
Risk: medium
Recommended backend: codex
Recommended model: gpt-5.4

## Problem

Baseline validation currently always proceeds regardless of why it failed. A harness or
environment problem should stop the dispatch before wasting an attempt on it; an
explicitly-expected red baseline should proceed under the existing warning; an unknown
red baseline is the ambiguous case that must not be silently improvised.

## Goal

Build policy on top of TICKET-110's `BaselineDisposition`:

- `clean` => proceed normally
- `expected_red` => proceed, existing warning-in-prompt behavior preserved
- `harness_error` => stop dispatch with a clear, actionable error
- `environment_error` => stop dispatch with a clear, actionable error
- `unknown_red` => stop dispatch unless an explicit override flag is passed

## Acceptance Criteria

1. `harness_error`/`environment_error` abort `improve()`/`fix()` before any attempt runs,
   with an error message naming the disposition and the raw baseline failure text
2. `unknown_red` aborts by default; an explicit CLI flag allows proceeding (matches the
   existing `--allow-draft-fail` convention for "operator explicitly accepted the risk")
3. `clean`/`expected_red` behavior is unchanged from current behavior
4. Tests for all five dispositions' dispatch-level effect (stop vs proceed)
5. Stopping preserves the worktree/session for inspection instead of silently cleaning up
   is not required unless already implemented that way — do not change unrelated cleanup
   behavior

## Constraints

- Do not let an LLM improvise baseline ownership
- Do not weaken existing draft-fail semantics
- Keep the override flag explicit and named for what it does, not a generic bypass

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
