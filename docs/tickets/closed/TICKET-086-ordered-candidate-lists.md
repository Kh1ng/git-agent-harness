# TICKET-086: Ordered route candidate lists

Goal: Configurable ordered candidate lists per mode with availability-aware skipping.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Add ordered route candidate lists with availability-aware skipping

## Problem

Routing currently selects from an unordered set of backend/model candidates. There is no stable priority order, no structured pairwise fallback, and no configurable escalation path. Availability is checked but the candidate iteration order is ad-hoc.

## Acceptance Criteria

1. Profile routing config accepts an ordered list of backend/model candidates per mode
2. Unavailable candidates (based on `gah availability` state) are silently skipped
3. Expired availability blocks re-enter the candidate pool
4. Manual disable is respected
5. All candidates exhausted produces a structured `NoEligibleBackend` error, not a silent fallback to an arbitrary default
6. Skip reasons (availability block, disable, exhaustion) are preserved for observability
7. Existing per-mode fallback config is honored
8. No quota or cost policy yet — pure availability-based skip
9. Backward compatible: profiles without ordered lists use current behavior

## Affected Files

- `src/routing.rs` — Candidate iteration and ordered list support
- Profile config — Ordered candidate list format

## Constraints

- No quota or cost policy in this ticket
- No AGY support
- No backend-instance abstraction redesign
- No broad runner refactor

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
