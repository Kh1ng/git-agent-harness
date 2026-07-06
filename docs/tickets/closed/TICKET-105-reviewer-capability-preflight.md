# TICKET-105: Add Reviewer Capability Preflight and Degraded-State Detection

Priority: P1
Difficulty: Medium
Status: TODO

## Summary

Extend reviewer preflight beyond binary existence.

Current checks can conclude a reviewer is available because "claude executable found" while the actual required review setup is degraded (Ponytail plugin missing, required skill unavailable, wrong model, review command not recognized). This can silently weaken review quality.

## Goal

Before a reviewer is trusted for merge policy, verify that its required capabilities are actually available.

Desired flow: executable available? → model/config available? → required review capability available? → reviewer healthy. Otherwise: degraded or unavailable → fallback reviewer → explicit ledger evidence.

## Requirements

- Build on reviewer capability model from TICKET-104
- Inspect existing preflight and availability paths
- Use one shared resolver/preflight path where possible
- Verify required capabilities without destructive side effects
- Avoid expensive real reviews during preflight
- Distinguish: backend unavailable, reviewer degraded, required capability missing
- Preserve raw diagnostic evidence
- Ensure manager does not silently trust a degraded reviewer as strong

## Acceptance Criteria

- Claude executable present but Ponytail missing → not healthy strong reviewer
- Healthy Claude + Ponytail → strong reviewer available
- Missing optional capability does not necessarily fail
- Required capability failure can trigger configured reviewer fallback
- Preflight and actual invocation use consistent configuration
- No silent downgrade

## Tests

- executable missing
- executable present, capability missing
- capability present
- optional capability missing
- fallback reviewer selection
- degraded-state ledger/status output
- config override

## Constraints

- No plugin auto-install
- No network dependency in unit tests
- No duplicated backend lookup logic
- No warning suppression

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
