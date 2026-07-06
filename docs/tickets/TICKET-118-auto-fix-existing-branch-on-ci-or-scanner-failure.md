# TICKET-118: Auto-dispatch a fix on the same branch when a draft PR/MR gets CI or scanner feedback

**Priority:** P0
**Profile:** gah

## Background

`classify()` in `src/sync.rs:137` already correctly classifies a draft PR/MR as
`CI_FAILED` for *any* failed check in the status rollup — this already includes
security-scanner checks, not just `cargo test`. Confirmed live: PR #56
(`gah/gah-1783341289`, TICKET-114) is already reported by `gah status --profile gah
--json` as `"classification": "CI_FAILED", "recommended_action": "REUSE_BRANCH"`
because GitHub's CodeQL code-scanning check came back FAILURE
(two real findings: insecure randomness in `apps/server/src/sessions/SessionManager.ts`,
missing `permissions:` block in `.github/workflows/CI.yml` — see TICKET-119 for the
actual fix to those).

So the classification signal is already right, for both GitHub and GitLab (`ci_failed`
is provider-abstracted in `SyncMr`). The gap is entirely on the execution side:

- `decide_next_action` (`src/controller.rs` ~line 177) explicitly routes
  `CI_FAILED | NEEDS_FIX` to `NextAction::HumanRequired` with the reason
  `"existing-branch continuation is unsupported"` — it never tries to fix it.
- `worktree::create` (`src/worktree.rs:39`) only supports creating a **new** branch
  from `origin/<target_branch>`. There's no path that checks out an **existing**
  remote branch (e.g. `gah/gah-1783341289`) into a worktree to continue work on it.

This means today, ANY CI failure or scanner finding on a draft GAH-created PR/MR —
whether it's `cargo test` failing, Dependabot, CodeQL, or GitLab's SAST/Dependency
Scanning reports — dead-ends at `HumanRequired` instead of being fixed automatically.
That's the behavior to change: "draft PRs that get feedback should retrigger the
pipeline."

## Task

1. Add a worktree constructor that checks out an *existing* branch (fetch + `git
   worktree add <path> <branch>`, no `-b`) instead of always branching fresh from
   `default_target_branch`. Reuse as much of `worktree::create`'s fetch/validation
   logic as makes sense — this is the same operation minus `-b`.
2. Change `decide_next_action`'s `CI_FAILED | NEEDS_FIX` arm to return a real
   dispatchable action (extend `NextAction::FixMr` or add a new variant — check
   whichever is less disruptive to existing match arms in `execute_action` and the
   TUI's `is_dispatchable` list in `src/tui_state.rs`) carrying the branch name.
3. Wire `execute_action`'s `FixMr` arm (`src/controller.rs` ~line 468, currently a
   stub: `"FixMr decided for branch '{branch}' ... but fix mode does not yet support
   ..."`) to actually: checkout the existing branch into a worktree, run the profile's
   fix-mode dispatch task (same shape as `dispatch::run`'s "fix" mode, minus creating a
   new branch), validate, commit, push to the *same* branch/PR (not a new one).
4. This must work identically for GitHub (`gh`) and GitLab (`glab`) — don't special-case
   one provider, `classify()`/`SyncMr` are already unified.
5. Guard against infinite fix-loops: reuse the existing `AUTO_RETRY_CAP` /
   prior-attempt-count pattern already used for ticket retries (`src/controller.rs`,
   search `AUTO_RETRY_CAP`) so a branch that keeps failing eventually still falls back
   to `HumanRequired` instead of looping forever.

## Acceptance Criteria

- [ ] A draft PR with a failing check (test failure OR a security-scanner finding)
      gets a new fix attempt pushed to the *same* branch/PR automatically via
      `gah loop --once` (or equivalent controller-driven dispatch), no `HumanRequired`
      stop, on both GitHub and GitLab profiles
- [ ] Repeated failures past `AUTO_RETRY_CAP` still stop at `HumanRequired` (no
      infinite loop)
- [ ] `cargo test` green; new tests cover the CI_FAILED → fix-dispatch path in
      `controller.rs` and the existing-branch worktree checkout in `worktree.rs`

## Do NOT

- Do not create a brand-new branch/PR for the fix — must reuse the existing one so
  review history and CI comparisons stay attached.
- Do not special-case "security finding" vs "test failure" — treat any failed check
  in the rollup the same way, `classify()` already does this correctly.
