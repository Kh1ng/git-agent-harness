# TICKET-074: Fix closed unmerged MR classification

Goal: Treat closed, unmerged MRs as terminal sync state instead of classifying them as NEEDS_REVIEW / RUN_REVIEW.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Treat closed unmerged MRs as terminal sync state

## Problem

`gah sync --json` and `gah status --json` currently classify closed, unmerged PRs as:
- classification = `NEEDS_REVIEW`
- recommended_action = `RUN_REVIEW`

This is controller-dangerous because closed terminal work can be mistaken for active review work.

Examples in this repository: PRs #15, #13, #11, #8, #1 are all CLOSED + merged=false but appear as NEEDS_REVIEW.

## Required Behavior

| State | merged | Classification | recommended_action |
|---|---|---|---|
| CLOSED | false | CLOSED_UNMERGED | NONE |
| MERGED | true | MERGED | NONE |
| OPEN | *any* | existing behavior unchanged | existing behavior unchanged |

Terminal state checks (MERGED, CLOSED_UNMERGED) must take precedence over:
- draft state
- merge status (CLEAN / DIRTY / UNKNOWN)
- review state
- CI state

## Affected Files
- The sync classification function that maps provider state to classification/recommended_action enums

## Required Tests

1. CLOSED + merged=false => CLOSED_UNMERGED
2. CLOSED + draft=true => CLOSED_UNMERGED
3. CLOSED + merge_status=CLEAN => CLOSED_UNMERGED
4. CLOSED + merge_status=DIRTY => CLOSED_UNMERGED
5. MERGED remains MERGED
6. OPEN draft behavior unchanged
7. sync --json and status --json remain consistent
8. Closed unmerged never recommends RUN_REVIEW
9. Fake GitHub provider integration verifies closed PR behavior
10. Fake GitLab provider integration verifies closed MR behavior where current fixtures support it

## Constraints

- No loop implementation
- No unrelated sync refactor
- No routing changes
- No provider API redesign
- No broad cleanup
- No dead-code removal
- Do not change non-Codex runner behavior
- Terminal-state check must be a simple precedence rule, not a system redesign

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
