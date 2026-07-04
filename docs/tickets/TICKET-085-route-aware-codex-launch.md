# TICKET-085: Route-aware Codex model launch

Goal: The resolved RouteDecision model must determine the actual Codex model invocation.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Make Codex model launch follow resolved RouteDecision

## Problem

Codex execution is currently pinned through profile `codex_args`, for example:

`-m gpt-5.4-mini`

while the routing layer may record a different `effective_model`.

This can cause:
- ledger model attribution mismatch
- incorrect availability scope
- corrupted usage/economics data
- misleading routing statistics
- incorrect future empirical optimization

## Acceptance Criteria

1. Resolved `gpt-5.4` route launches Codex with `gpt-5.4`
2. Resolved `gpt-5.4-mini` route launches Codex with `gpt-5.4-mini`
3. Invariant extra codex_args (e.g., `--dangerously-bypass-approvals-and-sandbox`) are preserved
4. Conflicting stale `-m` or `--model` flags in codex_args must not silently override the route
5. Ledger effective_model must match actual launched model
6. Non-Codex runners remain unchanged
7. Profile config can still supply invariant non-model args

## Affected Files

- `src/runner.rs` — Codex argument building

## Constraints

- Do not refactor the entire backend-instance system
- Do not add AGY support
- Do not change non-Codex runners
- Do not change availability or ledger schema

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
