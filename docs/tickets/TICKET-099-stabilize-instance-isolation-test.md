# TICKET-099: Stabilize logical backend-instance test isolation

Goal: Fix the flaky `independent_state_per_instance_of_the_same_backend_name` test so it reliably passes under `cargo test` (parallel execution).

Difficulty: easy
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4-mini
Suggested MR Title: Stabilize backend-instance isolation test against parallel test races

## Problem

`tests/fake_backend_harness.rs::independent_state_per_instance_of_the_same_backend_name` fails intermittently under `cargo test` (which runs integration tests in parallel).

## Root Cause Analysis

**Not an instance-identity bug.** The two fake `"claude"` instances write to completely separate filesystem paths (different `TempDir` subdirectories). There is no code path where one instance's call counter or argv can appear in the other's file tree.

The flake is from parallel test execution:
- `TempDir::new()` in concurrent tests may return paths under the same temp root
- Concurrent directory creation/cleanup in other `fake_backend_harness.rs` tests can race with directory creation or file reads
- No test-level serial execution is enforced for this file

## Acceptance Criteria

1. Test passes reliably under `cargo test` (parallel)
2. Test passes reliably under `cargo test --test fake_backend_harness -- --test-threads=1` (serial)
3. Root cause is confirmed as parallel test interference — not a code bug
4. Fix is minimal: either `#[serial_test::serial]` on the specific test, or a module-level mutex
5. No changes to `FakeBackend`, `Scenario`, or `run()` logic
6. All other tests in the file continue to pass reliably

## Affected Files

- `tests/fake_backend_harness.rs` — Serialize test or add mutex
- `Cargo.toml` — Only if `serial_test` dependency needed

## Constraints

- Minimal change
- Do not change `FakeBackend` implementation
- Do not change `Scenario`
- Do not make all tests in the file serial unnecessarily
- Do not change `run()` helper

## Verification Commands

- `cargo test --test fake_backend_harness -- --test-threads=1 independent_state 2>&1 | tail -5`
- `cargo test --test fake_backend_harness 2>&1 | tail -5`
- `cargo test 2>&1 | tail -5`
