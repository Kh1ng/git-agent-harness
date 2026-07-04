# TICKET-102: Add AGY-Specific Failure Classification for Deterministic Rerouting

Priority: P0
Difficulty: Medium
Status: TODO

## Summary

Extend GAH's shared backend failure/quota classification so real AGY failures can trigger deterministic retry and reroute behavior.

PR #38 fixed AGY silent-success handling: when AGY exits 0 with empty output, the runner now scopes cli.log evidence to the current invocation, classifies known quota/auth failures, and routes the result through the normal backend-error path instead of hard-aborting the harness.

However, the shared reroute classifier does not currently recognize AGY-specific failure text. As a result, AGY quota or auth failures are correctly recorded as backend failures but may not trigger cross-instance or cross-backend rerouting.

## Current State

Confirmed real AGY failure behavior includes:
- `RESOURCE_EXHAUSTED`
- `code 429`
- `Individual quota reached`
- and auth-related text such as `not logged into Antigravity`, `not logged in`

PR #38 intentionally did not extend the shared classifier because that module requires provenance-backed patterns.

## Goal

Teach the shared failure classifier to recognize real AGY quota/auth failures narrowly and safely so routing can make deterministic decisions.

Desired behavior:
- agy-main quota exhausted → classify quota_exhausted → try agy-second if independently available → then continue configured fallback chain
- agy-second auth failed → classify auth_failed → do not blindly retry same broken instance → continue policy-defined fallback

## Requirements

- Inspect the current shared classifier and provenance conventions
- Use real captured AGY failure evidence from this host or existing session artifacts
- Add narrowly scoped AGY patterns
- Do not match naked 429
- Keep quota and auth failures distinct
- Verify classifications enter the existing reroute/fallback path
- Do not duplicate runner-local classification logic unnecessarily
- Preserve current behavior for Codex, Claude, and other backends

## Acceptance Criteria

- Real AGY RESOURCE_EXHAUSTED evidence classifies as quota exhaustion
- Real contextual HTTP/code 429 evidence classifies as quota exhaustion
- Real AGY auth failure evidence classifies as auth failure
- Unrelated text containing 429 does not false-positive
- Classified AGY failure can trigger configured reroute behavior
- No infinite retry loop on a known exhausted instance
- Existing classifier tests remain green

## Tests

- AGY RESOURCE_EXHAUSTED
- AGY contextual code 429
- Individual quota reached
- auth failure
- naked unrelated 429
- unknown AGY empty failure
- correct fallback/reroute behavior
- no regression for existing backend patterns

## Constraints

- No speculative patterns without evidence
- No broad regex matching
- No backend redesign
- No token/auth-state manipulation
- No warning suppression
- No real network calls in tests

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`

## Report

Report: provenance used, patterns added, classification outputs, reroute behavior before/after, tests added, validation commands and results.
