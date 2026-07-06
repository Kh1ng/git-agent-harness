# TICKET-102a: Fix AGY empty-output reroute dead code

Priority: P0
Difficulty: Small
Status: COMPLETE

## Problem

PR #39 added AGY quota/auth regex patterns to quota_parser.rs, but dispatch.rs's reroute call sites (improve mode ~line 478, pm mode ~line 1202) propagate run_backend's Err immediately via `return Err(e)`/`?` before ever calling `mark_backend_unavailable_from_output`/`quota_parser::parse`. The two new regex alternatives are unreachable dead code.

The nonzero-exit AGY reroute path works correctly. Only the exit=0/empty-output silent-failure path is unaddressed.

## Fix

Wire the run_agy_with_executable empty-output Err path through the existing reroute/fallback mechanism instead of hard-aborting.

Specifically, inspect how `run_agy_with_executable` returns its Err for the empty-output case, and ensure that Err reaches `mark_backend_unavailable_from_output` before propagating, so the quota parser can classify it and trigger configured fallback.

## Affected files

- src/runner.rs (empty-output Err path)
- src/dispatch.rs (reroute call sites)
- src/quota_parser.rs (already has patterns, just unreachable)

## Acceptance Criteria

- AGY exit=0 empty-output quota failure triggers reroute (not hard abort)
- AGY exit=0 empty-output auth failure triggers reroute
- Nonzero-exit reroute still works
- Existing tests still pass
- No regression for Codex/Claude/OpenHands

## Verification

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
