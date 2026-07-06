# TICKET-121: PR/MR descriptions need the "why", not a file list

**Priority:** P1
**Profile:** gah

## Background

Live user complaint on PR #54: the description was a bare file list ("21 file(s)
changed... apps/server/src/bin.ts, apps/server/src/gahCli.ts, ... and 9 more") with no
explanation of the actual design decisions or reasoning behind the change. GitHub/GitLab
already have a full interactive diff view for "what files changed" — repeating that as
text in the description is wasted space, not useful information.

Root cause: `build_metadata_rich_mr_body` (`src/dispatch.rs` ~line 5555) composes the
PR body entirely from **static** ticket metadata (the ticket file's own Problem/Goal/
Acceptance Criteria, written before the backend ever ran) plus mechanical bookkeeping
(`render_changed_files`, attempts count, branch names). None of it comes from what the
backend actually *did* — there is no capture anywhere of the backend's own final
summary/reasoning. Confirmed: the commit message (`src/dispatch.rs` ~line 1305-1312) is
equally generic — literally `"gah: fix changes for gah"` for every single dispatch,
regardless of what the change was.

Every backend (vibe, codex, claude, etc.) already produces a final natural-language
summary of what it did and why as the last thing it writes to
`attempt-N/backend-output.log` before exiting. That's the missing input. Precedent for
reading log tail content already exists: `judge_experiment` (`src/dispatch.rs` ~line
1618) feeds `&log[log.len().saturating_sub(3000)..]` into a prompt — reuse that same
"tail of the log" extraction, no new parsing needed.

## Task

1. In whichever function currently reads `result.log_path` after a backend run
   (`src/dispatch.rs` ~line 1005 and ~1498 are existing read sites), capture the last
   ~1500-2000 chars of `backend-output.log` as a `backend_summary: String`.
2. Add `backend_summary: &str` to `MrRenderContext` and thread it through to
   `build_metadata_rich_mr_body` and `build_standard_mr_body`.
3. Replace the `## Changes` section (built by `render_changed_files`) with a
   `## What changed and why` section built from `backend_summary` instead. Drop the
   file list entirely — do not add a truncated/summarized file list either, GitHub/GitLab
   already render this natively.
4. Remove the `## Branch` section (`src/dispatch.rs` ~line 5634-5637 in
   `build_metadata_rich_mr_body`, ~line 5691-5694 in `build_experiment_mr_body`) —
   source/target branch is already shown natively by both providers' PR/MR UI.
5. Keep `## Backend / Model` and `## Attempts` sections as-is (explicitly called out as
   useful).
6. Fix the commit message (`src/dispatch.rs` ~line 1305-1312) the same way: keep the
   short mechanical first line (`"gah: {mode} changes for {repo_id}"`) but append a
   body paragraph built from the same `backend_summary` extraction, so `git log` isn't
   uniformly useless either.

## Acceptance Criteria

- [ ] New PR/MR descriptions have a `## What changed and why` section with real
      backend-generated content, not a file list
- [ ] No `## Changes` or `## Branch` sections remain
- [ ] Commit messages have a real body beyond the generic one-liner
- [ ] `cargo test` green; update/add tests around `build_metadata_rich_mr_body` and
      `build_experiment_mr_body` (existing tests assert on the old section names —
      e.g. `assert!(body.contains("Summary: 2 file(s) changed...`
      around line 4300 — these need to change to match, not just gain new assertions)

## Do NOT

- Do not re-summarize the file list in a shorter form — remove it, don't shrink it.
- Do not call out to an LLM to generate the summary (no `judge_experiment`-style
  extra API call) — the backend's own log tail already has this content for free.
