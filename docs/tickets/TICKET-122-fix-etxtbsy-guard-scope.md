# TICKET-122: Fix etxtbsy guard scope (check_duplicate_work repo filtering)

**Status**: In Progress
**Priority**: High
**Suggested MR Title**: Fix check_duplicate_work to scope ledger lookups to dispatching repo

## Problem

`check_duplicate_work` in `src/dispatch.rs` has the same cross-repo ledger scope bug that TICKET-097's fix addressed for `scan_available_tickets` in commit 00a24c5.

The ledger is one global file shared by every profile (`Defaults::ledger_path`), and a ticket's `work_id` is just a heading-derived string like "TICKET-090" with no repo namespace. Two unrelated repos (or even two ticket files in the same repo) can legitimately share that exact string.

Currently, `check_duplicate_work` iterates over all ledger entries matching the work_id without filtering by repo_id, which means entries from a different repo can incorrectly block dispatch.

## Background

Commit 00a24c5 fixed `scan_available_tickets` by adding:
```rust
if e.repo_id != profile.repo_id {
    continue;
}
```

This same fix needs to be applied to `check_duplicate_work`.

## Acceptance Criteria

- `check_duplicate_work` filters ledger entries by `entry.repo_id == profile.repo_id`
- New test case confirms that entries from a different repo with the same work_id do NOT block dispatch
- All existing tests continue to pass
- `cargo fmt` passes

## Related

- TICKET-097: Duplicate-work guard foundation
- Commit 00a24c5: "fix: scope ledger lookups to the dispatching repo, not the global ledger"
