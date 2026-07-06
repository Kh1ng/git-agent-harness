# TICKET-110: Baseline disposition classifier

Status: COMPLETE

Renumbered from TICKET-074 on 2026-07-05 — TICKET-074 collided with the pre-existing
TICKET-074 (fix closed unmerged MR classification, PR #17, complete). This is the third
live collision found in this backlog (after TICKET-102/103/104) — see docs/MANAGER_MEMORY.md
Ticket ID Rules.

Difficulty: medium
Risk: medium
Recommended backend: codex
Recommended model: gpt-5.4

## Problem

A failing baseline currently does not provide enough semantic ownership for later
automation. `dispatch.rs::improve()` runs validation once on the pristine worktree, and if
it fails, only records the raw failure text and appends a warning to the task prompt —
execution always proceeds regardless of why the baseline failed.

## Goal

Classify baseline validation into explicit dispositions:

- `clean` — baseline validation passed
- `expected_red` — baseline fails in a way the profile has explicitly declared as known/
  expected (never auto-detected — see constraints)
- `harness_error` — the validation command itself could not run (missing binary, spawn
  failure)
- `environment_error` — the validation command ran but failed on a well-known
  dependency/connectivity signature (missing module, linker not found, connection refused)
- `unknown_red` — validation failed and none of the above signatures matched

## Acceptance Criteria

1. Pure function: `classify_baseline(text: &str, exit_code: Option<i32>, known_failure_markers: &[String]) -> BaselineDisposition`
2. `harness_error` detected from process-level signals (exit code 127 / command-not-found
   shape), not string-matching the validation command's own stdout
3. `environment_error` uses a small, explicitly justified set of well-known
   dependency/connectivity signatures (each with a comment explaining why it's safe/common),
   not a speculative catch-all
4. `expected_red` is reachable ONLY via `known_failure_markers` explicitly configured on the
   profile — the classifier must never infer "this is expected" on its own
5. Anything else that fails classifies as `unknown_red`
6. Deterministic and side-effect free — no LLM call, no network call
7. Existing baseline-failure recording/prompt-injection behavior is preserved for `clean`
   and non-stopping dispositions

## Constraints

- Do not let an LLM improvise baseline ownership (no LLM call in the classifier itself)
- No speculative environment_error patterns without a clear, common, justified signature
- Do not change the ledger schema in this ticket (see TICKET-111 for stop policy wiring)

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
