//! Issue #765: rate-limited periodic hook for self-healing stale
//! availability records, called once per `run_once` tick.
//!
//! Issue #761's account-level quota refresh was deliberately NOT wired in
//! here alongside it, despite both being designed to share this one hook
//! (see `quota_store::refresh_stale_quota_observations`'s doc comment,
//! which is fully built and tested). Discovered empirically while adding
//! it: `refresh_codex_and_store`/`refresh_vibe_admin_and_store` invoke the
//! profile's actual configured `codex`/`vibe` executable, and several
//! existing integration tests (e.g.
//! `tests/controller_refill_regressions.rs`) substitute a fake binary on
//! `PATH` that tracks its own invocation count as a proxy for "how many
//! real dispatch workers ran." An unattended, automatic quota-refresh call
//! on every tick invokes that same fake binary outside of any dispatch, so
//! it inflates those counts and breaks tests whose fake-binary contract
//! implicitly assumed codex/vibe are only ever invoked for real work. Fixing
//! this properly means auditing every fake-binary-based integration test in
//! this repo (grep found at least 6 more `codex`-tracking fakes beyond the
//! one that surfaced this) before it's safe to call automatically -- real,
//! separately-scoped work, not something to rush alongside this hook.
//!
//! Until that lands, `gah quota refresh` (manual) remains the only way to
//! populate quota_observations.jsonl; `routing::policy::route_candidates`'s
//! live-data fallback already reads whatever's there, manual or automatic.

use time::OffsetDateTime;

pub fn run(now: OffsetDateTime) -> anyhow::Result<()> {
    // Corrects any stored unavailable record whose unavailable_until is
    // implausible; write-time capping (dispatch/attempts.rs) already keeps
    // new records honest, this catches ones written before that existed.
    // Zero new outbound calls -- the next real dispatch attempt after the
    // cap is the de facto re-probe.
    crate::availability::reprobe_stale_unavailable_records(
        &crate::availability::resolve_state_path(),
        now,
    )?;
    Ok(())
}
