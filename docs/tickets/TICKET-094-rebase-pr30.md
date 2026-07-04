# TICKET-094-REBASE: Rebase PR #30 changes onto current main

Goal: Cherry-pick the TICKET-094 authoritative PR description generation feature (from branch gah/gah-1783174575) onto current main, resolving any merge conflicts with #28 (PR title) and #34 (heading validation) which were merged after #30 was created.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4

## Affected Files
- src/dispatch.rs

## Acceptance Criteria
1. The build_mr_description function from PR #30 is present
2. No regressions from PR #28 (PR title) or #34 (heading validation)
3. cargo fmt --check passes
4. cargo test passes
5. Existing tests for PR titles and heading validation still pass

## Verification Commands
- cargo fmt --check
- cargo test
