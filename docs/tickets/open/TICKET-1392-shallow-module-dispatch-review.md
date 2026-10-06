# TICKET-1392: Resolve Shallow Module in Dispatch Review

## Problem
The `src/dispatch/review/mod.rs` module violates the APOSD "Deep Modules" principle. It serves purely as a pass-through module for its sub-components (`context` and `policy`) and introduces an extra layer of interface complexity without adding functionality or information hiding.

## Acceptance Criteria
- `src/dispatch/review/mod.rs` is refactored to either subsume the contents of `context.rs` and `policy.rs` directly, or the components are hoisted to a more appropriate level.
- Imports across the codebase are updated.
- `cargo check` and `cargo test` pass.

## Labels
refactor, P1
