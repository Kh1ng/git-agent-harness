Add one function that augments every loop-run entry with git state before and a "success" flag.

Goal: Automatic population for ledger rows when cron starts and ends.

Exact expected behavior
- capture_git_state uses subprocess to call git command and returns parsed dict.
- enrich_ledger_entry adds keys if missing; overwrites branch/success if passed.

Test or validation command
- pytest tests/test_ledger_enrichment.py
- python -c "from ops.entrypoint import capture_git_state; print(capture_git_state())"

Acceptance criteria
- In a real repo, git state fields are populated.
- In a temp dir without git repo, function raises or returns None (handled by caller).

Non-goals
- Does not write to DB itself.

Risk: low

----------
Local-coder suitability: ready_for_local_coder

Ticket road map for parent issue "Add loop run ledger":
1) Design schema for loop ledger
2) [this ticket] Wire ledger into cron entrypoint

Parent issue: #23
