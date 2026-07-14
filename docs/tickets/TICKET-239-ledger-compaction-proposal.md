# TICKET-239: Ledger rotation / compaction proposal

Status: proposal only, not implemented.

## Goal

Preserve the current append-only trust model while removing unbounded read amplification from `ledger.jsonl`.

## Proposed shape

1. Keep `ledger.jsonl` as the current writable tail segment.
2. Rotate sealed segments to `ledger.jsonl.NNNNNN` when the active segment reaches a size or age threshold.
3. Maintain a compact manifest alongside the segments:
   - ordered list of sealed segment paths
   - active tail path
   - byte offsets / entry counts for the last indexed point
   - optional SQLite projection checkpoint state
4. Rebuild reads by replaying sealed segments in manifest order, then the active tail.
5. Keep the SQLite mirror as a derived projection only; never treat it as the source of truth without an explicit owner decision and a migration plan.

## Recovery semantics

1. If the active tail is truncated, repair only the torn tail record as today.
2. If a sealed segment is missing or unreadable, fail closed and surface the corruption explicitly.
3. If the manifest and segment set disagree, prefer safety over convenience: stop automatic rotation and require operator repair.
4. Replaying from segments must be deterministic and idempotent, so a restored workspace can reconstruct the same ledger state from the manifest plus segment files alone.

## Open decision

This proposal does not change the runtime path yet. It is the owner decision boundary for whether GAH should keep a JSONL primary log with rotation or move the ledger source of truth to a different storage model later.
