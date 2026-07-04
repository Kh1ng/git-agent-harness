# TICKET-092-REBASE: Rebase PR #33 YAML fix onto current main

Goal: Cherry-pick the YAML frontmatter parsing fix (from branch gah/gah-1783176106) onto current main, resolving conflicts.

Difficulty: easy
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4

## Affected Files
- src/dispatch.rs

## Acceptance Criteria
1. YAML values containing colons parse correctly
2. Frontmatter delimiter "---" only on its own line
3. All existing tests pass including PR title, heading validation, PR description tests

## Verification Commands
- cargo fmt --check
- cargo test
