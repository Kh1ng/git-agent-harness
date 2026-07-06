# TICKET-087: Shared quota/cost pool identity seam

Goal: Represent shared scarce resources and route economics without confusing model, runner, account, and quota pool.

Difficulty: medium
Risk: medium
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Add quota pool identity for shared scarce resources

## Problem

Current architecture conflates model name, runner kind, backend, account, and quota pool. A quota/cost pool is not the same as a model name or a runner kind.

Examples:
- Codex mini and Codex gpt-5.4 share the `codex-main` quota pool
- Claude Haiku and Sonnet share the `claude-main` quota pool
- OpenHands routes may use independent metered pools

Without a pool seam, quota-pacing and cost-aware routing cannot distinguish:
- same pool, different model
- same model, different pool (Codex subscription vs Codex API)
- which pool's quota was consumed

## Acceptance Criteria

1. Define a quota pool identity concept (separate from model and runner)
2. Profile config can associate a backend/model candidate with a quota pool
3. Availability state can optionally reference a quota pool
4. Failed backends mark availability at the pool level where applicable
5. Missing pool identity is handled gracefully (no crash)
6. No cost or quota pacing policy yet — pure identity seam
7. Backward compatible: historical availability entries without pool metadata deserialize correctly

## Affected Files

- `src/routing.rs` — Pool identity in candidate definitions
- `src/availability.rs` — Optional pool scope
- Profile config — Pool association

## Constraints

- Minimal: just the identity seam, not the full policy
- Do not add AGY support
- Do not broadly redesign backend instances
- Do not change runner.rs

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
