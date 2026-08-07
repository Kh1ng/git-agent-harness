//! Issue #765: rate-limited periodic hook for self-healing stale
//! availability records, called once per real recurring `gah loop` tick
//! (`run_once`'s `run_periodic_probes` flag -- never a bounded `gah loop
//! --once` or the one direct in-process `run_once` unit test).
//!
//! Issue #761's account-level quota refresh (`quota_store::
//! refresh_stale_quota_observations`) was built and tested to share this
//! exact hook, and is deliberately NOT called from here. Three real
//! problems surfaced empirically while actually wiring it in, escalating
//! each time a fix for the previous one was tried:
//!
//! 1. `refresh_codex_and_store`/`refresh_vibe_admin_and_store` had zero
//!    subprocess supervision -- an isolated-HOME test made `codex status
//!    --json` hang, blocking the tick. Fixed: both now use
//!    `runner::process::arm_child_pdeathsig` plus their own bounded
//!    timeout (`usage::CODEX_STATUS_TIMEOUT` / curl `--max-time`) and
//!    `runner::process`'s new `register_supervised_child`/
//!    `kill_all_supervised_children` PID registry, addressing (2) below
//!    too. This part is solid and worth keeping regardless.
//! 2. The refresh still really invokes the profile's configured
//!    `codex`/`vibe` executable. Several `--once`-based integration tests
//!    substitute a fake binary on `PATH` that reacts to any invocation
//!    (counts calls, or touches a sentinel file) -- an unattended
//!    background probe call was indistinguishable from a real dispatch
//!    worker to those tests. The `run_periodic_probes` gate (recurring
//!    loop only) fixes every `--once`-based site.
//! 3. `tests/recurring_loop_parent_death.rs` spawns a REAL recurring loop
//!    (not `--once`, so (2)'s fix doesn't cover it) specifically to test
//!    that gah's own process and its dispatched worker both die when its
//!    launcher dies. With the refresh wired in, that test failed
//!    consistently -- but the actual cause traced back further than
//!    orphaned-child cleanup: the launcher log showed `gah loop` itself
//!    refusing to start at all ("parent PID is 1" --
//!    `controller::ownership::arm_parent_death_signal`'s own fail-closed
//!    check). The test's shell launcher backgrounds `gah loop`, polls a
//!    sentinel file for ANY invocation of the fake `codex`/`vibe` binary
//!    (again not distinguishing a real worker from a background probe),
//!    and exits the instant it sees one -- and the refresh's added
//!    per-tick overhead (thread spawn, mutex-guarded PID registry) was
//!    consistently enough to shift that already-tight launcher/child
//!    startup race such that the launcher exited before gah loop finished
//!    arming its own parent-death signal. This is a timing-sensitivity
//!    problem in a pre-existing, narrowly-timed test fixture, not a
//!    correctness bug in the supervision built for (1)/(2) -- but
//!    resolving it properly (e.g. gah loop confirming it has armed
//!    parent-death protection before any code path can satisfy the
//!    launcher's wait condition) is its own separately-scoped
//!    investigation, not something to bolt onto a routing/data-model
//!    change.
//!
//! `gah quota refresh` (manual) remains the only way to populate
//! `quota_observations.jsonl` until this lands;
//! `routing::policy::route_candidates`'s live-data fallback already reads
//! whatever's there regardless of how it got there.

use time::OffsetDateTime;

pub fn run(now: OffsetDateTime) -> anyhow::Result<()> {
    crate::availability::reprobe_stale_unavailable_records(
        &crate::availability::resolve_state_path(),
        now,
    )?;
    Ok(())
}
