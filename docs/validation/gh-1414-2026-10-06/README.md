# Issue #1414 investigation

Validation host: Linux (WSL2), not macOS. No issue comments were posted.

The unmodified parallel `cargo test --lib` reproduced the active-reviewer
failure (1,985 passed, 8 failed):

```text
thread 'runner::review::tests::run_review_backend_active_reviewer_completes_beyond_idle_window' panicked at src/runner/review.rs:585:9:
assertion `left == right` failed: after 8.32s, last progress Some(8.315106579); stdout tail: Some("line 60"); stderr: ""
  left: IdleTimeout
 right: Success
```

The review supervisor sampled stdout before synchronous worktree/process
probes and evaluated idle time after those probes. A slow probe could leave
that stream observation stale and kill a reviewer that emitted output or
completed during the probe. Stream sampling now follows the probes, and the
idle branch checks for completion before killing. The idle budget is unchanged.

Wizard fake effects also invoked the real `refresh_path()` without the shared
PATH lock. PATH refresh now belongs to real `SystemEffects::run`. This removes
that test isolation violation; it does not establish PATH as the cause of the
reported macOS failures.

The PTY failure did not reproduce. Its error previously discarded both script
stderr and the typescript. Nonzero exits now include both in the error. A Linux
regression test exercises a fake Claude exit 42 and verifies its error output
survives. macOS script exit-status semantics differ, so that regression is Linux
only.

Outstanding acceptance evidence: a reproduction or captured failure of the
original PTY test, a confirmed fix for its cause, and 20 consecutive full lib
suite runs on macOS. This issue is not fully verified by this Linux run.

## Verification

- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `cargo test --lib claude_monitor`: passed (7 tests, before adding the
  diagnostic regression; the regression subsequently passed in full runs).
- `cargo test --lib runner::review`: passed (11 tests after the supervisor fix).
- `cargo clippy --all-targets --all-features -- -D warnings`: passed after
  the supervisor fix.
- `XDG_STATE_HOME=/tmp/1414-state cargo test`: failed, 1,992 lib tests passed,
  two failed. Both focus tests and the diagnostic regression passed. Failures:
  `dispatch::attempts::tests::decide_route_classifies_no_eligible_backend_as_backend_error`
  and `dispatch::attempts::tests::exact_route_deferral_preserves_requested_identity_in_ledger_diagnostics`.
  Both also failed on the unmodified baseline. The command stopped at lib
  failures, before running integration tests.
- `XDG_STATE_HOME=/tmp/1414-state cargo test --test '*'`: failed in
  `controller_refill_regressions` (11 passed, two failed):
  `parallel_loop_refills_immediately_after_a_fast_completion` and
  `parallel_loop_reviews_a_finished_ticket_while_a_slow_sibling_still_runs`.
  Later integration targets were not run by Cargo after that failure.

The initial baseline additionally hit read-only concurrency state files and
subsequent poisoned locks. A writable `XDG_STATE_HOME` eliminated those
failures in the final run. All tests do not pass on this host; no unrelated
routing or scheduler changes were included.
