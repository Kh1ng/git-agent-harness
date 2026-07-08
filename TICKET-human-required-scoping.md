# Ticket: Fix profile-wide freeze caused by ticket-scoped `human_required` ledger entries

## Problem
A ticket-level review result with `human_required = true` can currently freeze ALL work for a profile.
Observed incident (gah profile, ~2h freeze):
- a review for one MR returned `NEEDS_FIX`
- the corresponding ledger entry had `human_required: true`
- that entry became the most-recent GAH ledger entry
- `status.rs` treated that latest profile-wide entry as the effective `recent_ledger`
- the entire gah profile stopped dispatching unrelated tickets
- manually setting that single ledger record to `human_required: false` immediately unblocked the profile

Root cause: profile-level blocking state is derived from the single most-recent ledger entry rather than from correctly scoped work-item state.

Relevant logic observed around `status.rs:218-239` (recent_ledger selection) and `status.rs:291` (blocker push). **Inspect current HEAD — line numbers may have shifted.**

## Intended behavior
`human_required` produced by a ticket/MR review is **work-item scoped**. A blocked item must not freeze unrelated work.
Example: TICKET-A=human_required, TICKET-B/C=eligible → TICKET-A blocked, B and C dispatchable, profile operational.
Only genuinely profile-wide failures should stop all work: repo sync failure, invalid profile config, required infra unavailable, provider/auth failure with no viable route, or an explicitly modeled profile-level human gate. A ticket review result must NOT become a global blocker merely because it is the newest ledger record.

## Required fix
1. **Remove latest-entry global inference.** Do not derive profile-wide `human_required` from the single most-recent profile ledger entry (`latest_profile_entry.human_required` must not control eligibility for unrelated work items).
2. **Scope human-required state by work item.** When evaluating a ticket/MR, derive its effective `human_required` only from ledger history relevant to that work item: `entries_for_work_id(work_id) -> reduce to effective state -> block that work item if human_required`. Use EXISTING canonical state-reduction helpers (e.g. `LedgerEntriesByWorkId` / `ledger_lookup_for_ticket`) — do NOT introduce a second ad hoc interpretation of ledger history.
3. **Preserve genuine profile-wide blockers.** Do NOT solve this by globally ignoring `human_required`. Keep existing legitimate profile-wide blocking behavior. If the data model mixes profile-scoped and work-scoped records, make the distinction explicit so ticket-level review records cannot block the profile.
4. **Continue scanning after blocked work items.** The scheduler/controller must keep evaluating other eligible work after a human-blocked item. A blocked ticket is skipped, not a reason to terminate the profile loop.
5. **Correct status reporting.** `gah status` must distinguish profile-wide blockers from work items awaiting human action. A profile with one human-blocked ticket + other runnable work must NOT report a global `human-required` blocker. The blocked work item must remain visible in ticket/work-item state.

## Regression tests (add focused tests)
- Ticket-scoped human block does not freeze profile: TICKET-A human_required=true, TICKET-B eligible → A not dispatchable, B dispatchable, no profile-wide block.
- Most-recent ledger entry belongs to another blocked ticket: ledger order B-eligible then A-human_required written later → B remains eligible. (Directly reproduces the incident.)
- Human-blocked ticket remains blocked: TICKET-A human_required=true → A not redispatched.
- Multiple blocked + eligible coexist (A,C blocked; B,D eligible) → A,C blocked, B,D dispatchable, profile operational.
- Genuine profile-wide blocker still stops dispatch (use existing explicit profile-scoped blocker mechanism) → all work blocked, status reports profile-wide blocker.
- Later work-item state clears prior human requirement (where existing ledger semantics support it): TICKET-A human_required then later cleared → effective state no longer blocked.

## Constraints
- Do NOT edit `ledger.jsonl` as part of the implementation.
- Do NOT globally ignore `human_required`.
- Do NOT weaken human-review safety behavior.
- Do NOT broaden into reviewer-tier or merge-policy refactoring.
- Preserve append-only ledger semantics.
- Keep fix centered on state scoping, status correctness, dispatch behavior.
- Prefer existing canonical per-work-item state derivation over new bespoke logic.
- Inspect ALL consumers of `recent_ledger`, profile-level `human_required`, and related status/blocker derivation to ensure the same profile-wide scoping bug isn't duplicated elsewhere.

## Validation (must pass before reporting done)
```
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## Acceptance criteria
- a ticket-level `human_required=true` record blocks ONLY that work item
- unrelated eligible tickets continue dispatching
- the newest ledger entry no longer implicitly defines profile-wide `human_required`
- genuine profile-wide blockers still halt the profile
- status output accurately separates profile blockers from blocked work items
- the observed freeze scenario is covered by a regression test

## Repo / workflow
- Repo: /home/khing/workspace/agent-lab/repos/github/Kh1ng/git-agent-harness (on branch `main`)
- Create a feature branch (e.g. `fix/ticket-scoped-human-required`) — do NOT push, do NOT merge, do NOT open a PR. Leave the branch local for human review.
- The repo currently has untracked `IMPLEMENTATION_PLAN.md` — leave it alone.
- Do NOT run `gah loop` or touch running supervisors. This is a source-code fix only.
- Report: which files changed, the test results (fmt/clippy/test output summary), and the branch name.
