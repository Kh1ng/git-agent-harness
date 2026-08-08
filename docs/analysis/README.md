# One-off analysis snapshots

Point-in-time outputs of `scripts/analysis/*.py`, kept for reference
alongside the findings they fed into (issues, PR comments, handoff notes).
Not regenerated automatically -- re-run the corresponding script against a
live ledger for current numbers.

- `model-vs-complexity-2026-08-08.txt` -- backend/model success rate by
  ticket difficulty, from `scripts/analysis/model_vs_complexity.py`. Fed
  issue #907 (`gah report --group-by difficulty`, the permanent version
  of this cross-tab) and the 2026-08-08 handoff notes in
  `docs/MANAGER_MEMORY.md`.
