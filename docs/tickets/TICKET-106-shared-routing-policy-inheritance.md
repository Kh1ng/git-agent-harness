# TICKET-106: Add Canonical Shared Routing Policy Inheritance

Priority: P1
Difficulty: Medium-Hard
Status: TODO

## Summary

Move the current trusted GAH routing/model policy out of the Git Agent Harness repo-specific config and into a shared canonical harness-level policy inherited by all repositories.

A newly imported repository should require minimal config while automatically receiving the current trusted routing/reviewer/model policy.

## Goal

Implement layered configuration with precedence equivalent to:
compiled defaults < shared canonical config < repo-specific config < CLI/runtime override

## Requirements

- Inspect actual current config loader and schemas
- Identify current GAH repo routing/model policy
- Separate reusable policy from repo-specific settings
- Introduce shared canonical config using current config-root conventions
- Make repo configs inherit automatically
- Use field-level merge semantics
- Nested repo overrides must not erase unrelated inherited settings
- Ordered routing lists should replace that specific inherited list when explicitly overridden
- Maps should merge by key where appropriate
- Preserve standalone legacy configs
- Ensure new repo onboarding does not copy canonical routing into every repo file
- Provide effective-config observability without printing secrets

## Acceptance Criteria

- GAH effective routing unchanged after migration
- GAH repo config no longer duplicates canonical shared routing
- New minimal repo inherits canonical routing
- World Cup can inherit canonical routing while overriding repo-specific behavior
- One nested override does not erase unrelated inherited values
- CLI/runtime override remains highest precedence
- Missing shared defaults preserves backward-compatible behavior

## Tests

- shared inheritance
- minimal repo config
- partial nested override
- route-list replacement
- map key merge
- no defaults file
- CLI override precedence
- malformed shared config
- malformed repo config
- no duplicate routing entries
- GAH effective policy equivalence before/after

## Constraints

- No duplicated canonical policy
- No machine-specific hardcoded paths
- No unrelated backend redesign
- No warning suppression
- No secret leakage
- Keep migration reviewable

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
