# TICKET: GAH must skip stalled tickets and keep dispatching the rest

## Problem (recurring GAH stall)

`decide_next_action` in `src/controller.rs` short-circuits to a **profile-wide
`HumanRequired`** the moment it sees ANY ticket with `prior_attempt_count >=
AUTO_RETRY_CAP` (currently 2). The supervisor then parks the *entire* profile for
1800s instead of dispatching the other eligible tickets.

Real evidence (2026-07-09): GAH profile has 16 eligible tickets, but
`TICKET-113` (`prior_attempt_count = 2`, `failed 2 time(s) with no active MR`)
causes `decide_next_action` to return `HumanRequired` every cycle. The other 15
tickets never get dispatched. The WC profile (same harness) flows fine because it
has no exhausted ticket at the front of the scan order.

The prior freeze fix (PR #131, TICKET-human-required-scoping) handled the ledger
`human_required` path correctly (per-work-item scoping). This is a *second*
short-circuit that was not fixed: the retry-cap loop at controller.rs:283-297.

## Exact fix location

File: `src/controller.rs`
Function: `decide_next_action` (lines 160-368)

### Change 1 — retry-cap loop (lines 283-297)

Current:
```rust
    for ticket in &failed_tickets {
        if ticket.prior_attempt_count >= AUTO_RETRY_CAP {
            return NextAction::HumanRequired {
                reason: format!(
                    "{} failed {} time(s) with no active MR; stopping automatic retries",
                    ticket.work_id.as_deref().unwrap_or(&ticket.ticket_path),
                    ticket.prior_attempt_count
                ),
                reference: ticket
                    .work_id
                    .clone()
                    .or_else(|| Some(ticket.ticket_path.clone())),
            };
        }
    }
```

New behavior: do NOT return. Instead collect the exhausted ticket work_ids into a
set and let the loops below skip them. The exhaustion of one ticket must NOT freeze
the profile. Only if NO eligible ticket remains anywhere should the function fall
through to `WaitUntil`/`NoOp` (which it already does at lines 350-367).

Concretely:
- Compute `let exhausted: std::collections::HashSet<_> = failed_tickets.iter()
   .filter(|t| t.prior_attempt_count >= AUTO_RETRY_CAP)
   .filter_map(|t| t.work_id.clone())
   .collect();`
- In the Escalate loop (298-313) and Retry loop (314-329): skip any ticket whose
  work_id is in `exhausted` (so we don't escalate/retry a cap-exhausted ticket).
- Remove the early `return` entirely.

### Change 2 — final profile-wide HumanRequired guard (new, optional)

After the `undispatched` dispatch loop (lines 331-348) and after the WaitUntil scan
(350-363), the function already ends with `NextAction::NoOp { "nothing actionable" }`
at 365-367. This is correct: if every ticket is exhausted/skipped, we NoOp (and the
supervisor sleeps 600s) rather than parking on HumanRequired. **Do not** change the
NoOp fallthrough.

NOTE: the existing `human_required` per-ticket scoping (lines 279, 337 filters and
`snapshot.blocked_work_items`) stays as-is. We are only removing the *profile-wide
early return* on retry-cap so other tickets flow.

## Stuck-loop detection (controller.rs:380-412 `detect_stuck_loop`)

Leave the function logic as-is (it is work-item-scoped and correct). Do NOT change
it. The only requirement: a profile-wide `HumanRequired` (work_id=None) must not be
treated as "stuck" in a way that re-parks the profile — and it already isn't, because
it carries work_id=None and `detect_stuck_loop` returns None for None work_id.

## Tests

The existing test `retry_cap_exceeded_forces_human_required` (controller.rs:1203-1216)
asserts `AUTO_RETRY_CAP` ticket -> `human_required` when it is the ONLY ticket. That
test must still pass (single exhausted ticket = nothing else to do = correctly
human_required via the NoOp path? NO — it currently returns HumanRequired directly).

IMPORTANT: adjust so the contract is preserved:
- `retry_cap_exceeded_forces_human_required` (single exhausted ticket, nothing
  else): keep asserting `human_required`. Achieve this by having the retry-cap loop,
  when it is the ONLY ticket and nothing else is actionable, still return
  HumanRequired. Simplest: only suppress the return when there is ANOTHER eligible
  ticket. i.e. keep the `return NextAction::HumanRequired` IF
  `undispatched.is_empty()` AND no escalate/retry candidate exists. Rewrite the
  loop to: if ticket is exhausted -> record it; after the loop, if `exhausted` is
  non-empty AND there is no other actionable ticket (no undispatched, no escalate,
  no retry), then return HumanRequired for the first exhausted ticket. Otherwise
  continue to dispatch/escalate/retry the others.

Add a NEW regression test `exhausted_ticket_does_not_block_others`:
- snapshot with TICKET-113 (prior_attempt_count=2, has_active_mr=false,
  human_required=false) AND TICKET-128 (prior_attempt_count=0,
  human_required=false).
- assert `decide_next_action` returns `DispatchTicket` for TICKET-128 (or another
  eligible ticket), NOT `human_required`.

Use the existing test helpers `empty_snapshot()` and `ticket(path, work_id,
prior_attempt_count, failure_class, has_active_mr, human_required)` (see ~line 995+
and ~line 1237+). Match their signatures.

## Validation (must pass before reporting done)

From repo root `/home/khing/workspace/agent-lab/repos/github/Kh1ng/git-agent-harness`:

```
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

All three must exit 0. Do NOT edit `ledger.jsonl` or `events.jsonl`. Do NOT run
`gah loop`. Do NOT push or open a PR. Commit the change on a feature branch
`fix/gah-skip-stalled-tickets` and report the branch name + test count.

## Safety invariants to preserve
- Genuine profile-wide blockers (sync failure, invalid config, infra unavailable,
  auth failure) still return HumanRequired (handled at controller.rs:170-184 via
  `snapshot.blockers` — unchanged).
- Per-ticket `human_required` (ledger) still scopes to the work item, not the
  profile (unchanged).
- A ticket that genuinely exhausted its retry cap is NOT silently redispatched; it
   is skipped and the human still sees it via `blocked_work_items` / status output.

<!-- Implemented in PR #133: exhausted-ticket skip logic lives at src/controller.rs:402-449. Moved to docs/tickets/closed/ as resolved. -->
