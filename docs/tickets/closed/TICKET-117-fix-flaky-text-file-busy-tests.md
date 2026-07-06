# TICKET-117: Fix flaky "Text file busy" (ETXTBSY) test failures

**Priority:** P1
**Profile:** gah

## Background

Intermittent test failures with `Text file busy` (ETXTBSY) have shown up twice this
session on different, unrelated tests (`run_backend_omits_print_timeout_for_unmapped_model`
in `src/dispatch.rs`, `agy_empty_output_with_quota_log_detected_as_error` in
`src/runner.rs`) — never the same test twice, only under parallel `cargo test`, never
when the failing test is rerun alone. This is real concurrency noise, not a per-test bug.

**Root cause (verify before assuming this is the whole picture, but it's the standard
explanation for this exact symptom in multi-threaded Rust test binaries):** many test
modules (`src/dispatch.rs`, `src/runner.rs`) call a `make_fake_bin`/`make_recording_bin`
helper that does `fs::write` + `chmod +x` on a freshly created temp file, then almost
immediately `std::process::Command::spawn()`s it. `cargo test` runs tests in parallel
*threads within one process*. `fork()` (which `Command::spawn` uses under the hood on
Linux) duplicates the whole process's file descriptor table into the child at the
instant it's called — including any other thread's momentarily-open write-mode fd to
an unrelated temp binary. If thread B forks while thread A's write-fd to its own temp
binary is still open, and thread A (or anyone) tries to exec that inode before the
child either exits or execs (closing the inherited fd), the kernel returns ETXTBSY.

The project already hit an analogous process-wide-mutation race for `PATH` and fixed it
with a single shared lock — see `src/test_support.rs`'s `PATH_LOCK` and its doc comment
("every module that touches PATH in tests must go through this single lock"). This
ticket is the same fix pattern applied to "write + immediately exec a temp binary"
instead of "mutate PATH".

## Task

1. Add a second process-wide test-only `Mutex<()>` in `src/test_support.rs` (e.g.
   `EXEC_LOCK`), analogous to `PATH_LOCK`.
2. Have `make_fake_bin` (`src/dispatch.rs` ~line 3180) and `make_recording_bin`/
   `make_fake_bin` (`src/runner.rs` ~line 924) — or better, whatever single shared
   spawn entry point they funnel through — hold that lock across the
   write-file → chmod → spawn window, so two threads can never have that race
   simultaneously. Check whether the lock should be held only around
   write+chmod+spawn-call, or needs to extend until the child process is confirmed
   started (spawn() returning `Child` should be enough — exec has completed by then).
3. Do NOT just serialize the entire test suite (`--test-threads=1`) as the fix — that
   papers over the race and slows every future test run. The lock must be scoped to
   only the write+spawn critical section.
4. Stress-test the fix: run the full suite repeatedly under parallel threads (e.g.
   `for i in $(seq 1 20); do cargo test || break; done`) and confirm zero ETXTBSY
   failures across all runs. A single passing run is not sufficient evidence, this bug
   is flaky by definition.

## Acceptance Criteria

- [ ] New shared lock added, used by both `src/dispatch.rs` and `src/runner.rs`
      fake-binary helpers
- [ ] 20x repeated `cargo test` run with zero ETXTBSY failures (paste the loop output
      in the MR description as evidence, not just a claim)
- [ ] No change to `--test-threads` or other global suite-parallelism settings
- [ ] `cargo fmt --check` / `cargo test` still green

## Do NOT

- Do not disable/ignore the flaky tests (`#[ignore]`) as a workaround.
- Do not serialize the whole test binary (`--test-threads=1`).
