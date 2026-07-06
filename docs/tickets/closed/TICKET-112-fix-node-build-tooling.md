# TICKET-112: Fix Node/pnpm build tooling for the t3code adaptation (apps/*)

**Priority:** P0 (blocks TICKET-113 through TICKET-117)
**Profile:** gah

## Background

The t3code-style adaptation (`apps/server`, `apps/web`, `apps/desktop`, `packages/contracts`,
`packages/shared`) was committed but has never actually been built on this machine:

- `pnpm` is not installed anywhere on the system (`which pnpm` fails), yet every script in
  root `package.json` and every `apps/*/package.json` shells out to `pnpm`.
- Root `package.json`'s `build:rust` script is `cd rust-backend && cargo build --release` —
  there is no `rust-backend/` directory. The real Rust crate lives at the repo root
  (`Cargo.toml`, `src/`).
- No lockfile exists anywhere (no `pnpm-lock.yaml`, no `package-lock.json`).
- `scripts/dev-runner.js` and `packages/contracts`/`packages/shared` exist and look real
  (not stubs) but have never been exercised.

Node itself is available (`node` v22.23.1 via nvm), so the fix is tooling/config, not a
missing runtime.

## Task

1. Decide pnpm vs npm workspaces and make it consistent:
   - If pnpm: it must actually get installed (`corepack enable` is the standard way to get
     pnpm without a global npm install, since Node 22 ships corepack) — verify this works in
     this environment, don't just assume it.
   - If npm: convert `workspaces` array (already present in root `package.json`) to work with
     plain `npm install`, and rewrite every `pnpm run`/`pnpm install` invocation in
     `package.json` scripts and `scripts/dev-runner.js` to the npm equivalent.
   - Pick whichever you can prove works end-to-end in this sandbox; document the choice in
     the PR description with the actual command output showing a clean install.
2. Fix `build:rust` (and any other `rust-backend/`-relative reference) to point at the repo
   root, where `Cargo.toml` actually is.
3. Run a real install (`npm install` or `pnpm install`, per your choice above) from the repo
   root and confirm it completes without errors. Commit the resulting lockfile.
4. Run `typecheck` (or the per-app `tsc` scripts) across `apps/server`, `apps/web`,
   `apps/desktop`, `packages/contracts`, `packages/shared` and fix any compile errors that
   surface now that the packages are actually being built for the first time. Do not silence
   errors with `any`/`@ts-ignore` — fix the actual type mismatch, or if something is
   genuinely unfinished stub code, leave a clear one-line comment saying so (don't invent new
   functionality here, this ticket is tooling, not features).
5. Confirm `cargo build` and `cargo test` still pass (they should be unaffected by this
   ticket, but validate before/after to be sure nothing in package.json touches the Rust
   build path in a way that breaks it).

## Acceptance Criteria

- [ ] `npm install` or `pnpm install` (whichever was chosen) succeeds from a clean checkout
- [ ] A lockfile is committed
- [ ] `build:rust` script path is correct (repo root, not a nonexistent `rust-backend/`)
- [ ] Typecheck passes (or documents exactly what's still broken and why, with no papered-over `any`)
- [ ] `cargo build` / `cargo test` still pass
- [ ] Draft MR with the above validation output attached

## Do NOT

- Do not add new features or UI changes — this ticket is strictly "make the existing code
  installable and typecheckable."
- Do not delete `apps/server`'s provider driver files — that's TICKET-113's scope, not this
  one's.
