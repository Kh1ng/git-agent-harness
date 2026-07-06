# TICKET-119: Fix the two open CodeQL findings

**Priority:** P0
**Profile:** gah

## Background

`gh api repos/Kh1ng/git-agent-harness/code-scanning/alerts` shows two open findings
on `main`, both confirmed real by reading the flagged code:

1. **Insecure randomness (high)** — `packages/shared/src/utils.ts:9` and `:13`:
   ```ts
   export function generateSessionId(): SessionId {
     return `session_${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;
   }
   export function generateRequestId(): string {
     return `req_${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;
   }
   ```
   `Math.random()` is not cryptographically secure and these IDs are used to key
   session lookups (`apps/server/src/sessions/SessionManager.ts`) — predictable
   session IDs are a session-hijacking risk if these are ever exposed to a client.

2. **Workflow missing permissions (medium)** — `.github/workflows/CI.yml` has no
   top-level `permissions:` block, so the job runs with the default (often
   overly-broad) `GITHUB_TOKEN` scope.

## Task

1. Replace `Math.random().toString(36).substring(2, 9)` in both functions in
   `packages/shared/src/utils.ts` with `crypto.randomUUID()` (Node/browser built-in,
   no new dependency). Keep the `session_`/`req_` prefixes and `Date.now()` component
   if useful for readability/sorting, but the random component must not be
   `Math.random()`-derived.
2. Add a `permissions:` block to `.github/workflows/CI.yml`. This workflow only
   checks out and runs tests — it doesn't need write access. Use
   `permissions:\n  contents: read` at the top level (job-level is fine if that's
   cleaner given the single `test` job).
3. Re-run `gh api repos/Kh1ng/git-agent-harness/code-scanning/alerts` after merge to
   confirm both alerts move to `state: "dismissed"` (auto-closed once the fixed code
   lands on `main` and CodeQL re-scans).

## Acceptance Criteria

- [ ] `Math.random()` no longer used for session/request ID generation
- [ ] `CI.yml` has an explicit `permissions:` block
- [ ] `cargo test` / `cargo fmt --check` still green (no Rust changes expected, but
      standing validation bar)
- [ ] `tsc -b` clean on `packages/shared` and any consumer that imports these
      functions

## Do NOT

- Do not touch the electron/vite Dependabot alerts (18 of them, all pre-existing
  transitive deps) — those go away once TICKET-115 rips out Electron; don't spend
  effort patching a dependency tree that's getting deleted.
