# TICKET-117: Fix flaky "Text file busy" (ETXTBSY) test failures

**Status:** COMPLETE
**Priority:** P1
**Profile:** gah

## Background

Intermittent test failures with `Text file busy` (ETXTBSY) appeared twice this session on different, unrelated tests (`run_backend_omits_print_timeout_for_unmapped_model` in `src/dispatch.rs`, `agy_empty_output_with_quota_log_detected_as_error` in `src/runner.rs`) — never the same test twice, only under parallel `cargo test`, never when the failing test was rerun alone. This is real concurrency noise, not a per-test bug.

## Root Cause

When `cargo test` runs tests in parallel threads within one process, the following race condition can occur:

1. Thread A creates a temp binary file via `fs::write()` + `chmod +x`
2. Thread B calls `Command::spawn()` which internally uses `fork()`
3. `fork()` duplicates the entire process's file descriptor table into the child
4. If Thread A still has an open write-mode fd to its temp binary when Thread B forks, the child process inherits that fd
5. When anyone tries to `exec()` that inode while the inherited fd is still open, the kernel returns ETXTBSY

This is the same pattern that was previously fixed for PATH mutations with `PATH_LOCK` in `src/test_support.rs`.

## Solution

Added a second process-wide test-only `Mutex<()>` named `EXEC_LOCK` in `src/test_support.rs`, analogous to `PATH_LOCK`, and created an `ExecGuard` helper to hold this lock across the write-file → chmod → spawn window.

Updated all three `make_fake_bin` functions to hold the `EXEC_LOCK`:
- `src/dispatch.rs` line ~3180
- `src/runner.rs` line ~924
- `src/provider.rs` line ~465

This ensures that two threads can never have the write+spawn race simultaneously.

## Changes Made

1. **src/test_support.rs**: Added `EXEC_LOCK` static mutex and `ExecGuard` struct
2. **src/dispatch.rs**: Added import for `ExecGuard` and used it in `make_fake_bin`
3. **src/runner.rs**: Added import for `ExecGuard` and used it in `make_fake_bin` (which is also called by `make_recording_bin`)
4. **src/provider.rs**: Added import for `ExecGuard` and used it in `make_fake_bin`

## Verification

- 20 consecutive `cargo test` runs with zero ETXTBSY failures (completed successfully)
- `cargo fmt --check` passes
- All 327 unit tests + 9 fake_backend_harness tests + 91 gah_cli tests pass
- No change to `--test-threads` or other global suite-parallelism settings

## Acceptance Criteria

- [x] New shared lock added, used by both `src/dispatch.rs` and `src/runner.rs` and `src/provider.rs` fake-binary helpers
- [x] 20x repeated `cargo test` run with zero ETXTBSY failures
- [x] No change to `--test-threads` or other global suite-parallelism settings
- [x] `cargo fmt --check` / `cargo test` still green
