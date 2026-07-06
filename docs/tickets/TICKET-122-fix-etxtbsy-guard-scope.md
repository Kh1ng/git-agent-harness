# TICKET-122: TICKET-117's ETXTBSY fix (PR #59) is incomplete — guard released before spawn

**Priority:** P0
**Profile:** gah

## Background

TICKET-117 asked for a lock around the write+chmod+spawn window to fix a flaky
`Text file busy` (ETXTBSY) test failure. PR #59 (branch `gah/gah-1783342786`) added
`EXEC_LOCK`/`ExecGuard` in `src/test_support.rs`, but scoped it wrong:

```rust
fn make_fake_bin(dir: &Path, name: &str) -> std::path::PathBuf {
    let _guard = ExecGuard::new();   // <-- dropped when this fn returns
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    ...
    path
}
```

`_guard` is a local variable inside `make_fake_bin` — it's dropped the instant the
function returns, which is *before* the caller ever calls `.spawn()` /
`run_backend()` on the binary this just created. The lock never actually covers the
exec() call, only the write+chmod, which was never the racy half.

**Confirmed still broken**: checked out PR #59's branch fresh and ran `cargo test`
5 times in a row. Run 5 failed with the identical error the ticket was supposed to
fix:
```
dispatch::tests::agy_second_backend_runs_with_agy_second_home_override --- FAILED
thread '...' panicked at src/dispatch.rs:3240:10:
    Text file busy (os error 26)
```

Do not merge PR #59 as-is (a comment explaining this is already posted on that PR).

## Task

1. Change `make_fake_bin` (in all three touched files: `src/dispatch.rs`,
   `src/runner.rs`, `src/provider.rs`) to **return** the `ExecGuard` alongside the
   binary path, instead of creating-and-dropping it internally. e.g.:
   ```rust
   fn make_fake_bin(dir: &Path, name: &str) -> (std::path::PathBuf, ExecGuard) {
       let guard = ExecGuard::new();
       let path = dir.join(name);
       fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
       ...
       (path, guard)
   }
   ```
2. Update every call site to bind the returned guard to a variable that lives for
   the rest of the test (i.e. until after the spawn/`run_backend`/`Command::output()`
   call completes) — `let (fake_bin, _guard) = make_fake_bin(...);` not
   `let fake_bin = make_fake_bin(...).0;` (the latter drops the guard immediately,
   same bug in a different shape).
3. Some tests construct their fake binary inline without going through
   `make_fake_bin` (e.g. `agy_second_backend_runs_with_agy_second_home_override`
   itself, which does its own `fs::write`/`chmod` directly per the diff seen in
   PR #59 — check whether this specific test needs its own inline `ExecGuard` too,
   since it's the one that actually failed in the reproduction above).
4. Re-verify empirically: run the full test suite at least 20 times in a row
   (`for i in $(seq 1 20); do cargo test --quiet || break; done`) and paste the
   actual output in the MR description — a claim of "verified" without the pasted
   output is not sufficient, this exact ticket already shipped once with an
   unverified claim that didn't hold up.

## Acceptance Criteria

- [ ] `ExecGuard` demonstrably held across the entire write→chmod→spawn window for
      every fake-binary test in `dispatch.rs`, `runner.rs`, `provider.rs`
- [ ] 20x repeated `cargo test` with zero ETXTBSY failures, actual output pasted in
      the MR, not just asserted
- [ ] Superseded PR #59 is closed once this lands (leave that to the human /
      controller reconciliation, don't force-close it yourself)

## Do NOT

- Do not just re-run the flaky test in isolation and call it verified — the bug only
  reproduces under the full parallel suite.
- Do not add `#[serial]`/`serial_test` crate or `--test-threads=1` as a shortcut —
  same constraint as TICKET-117: scope the lock, don't serialize everything.
