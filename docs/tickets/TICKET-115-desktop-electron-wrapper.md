# TICKET-115: Get the Electron desktop app actually launching

**Priority:** P3 (depends on TICKET-114 — desktop just wraps the web app + server)
**Profile:** gah

## Background

`apps/desktop` already has `src/main/main.ts`, `src/main/preload.ts`, vite configs for
main/preload/renderer, and an electron-builder config — but like the rest of `apps/*` it has
never actually been run (`electron` isn't installed, nothing has been typechecked or
launched). The design intent (per `main.ts`) appears to be: spawn/manage the Node server
(`apps/server`) as a child process and open a `BrowserWindow` pointed at the built
`apps/web` output.

## Task

1. Get `apps/desktop` actually launching in dev mode (`npm run dev:desktop` /
   `pnpm dev:desktop`) once TICKET-112's tooling fix is in place.
2. Confirm the Electron window shows the same dashboard as the browser version (TICKET-114),
   backed by the same real `gah` CLI data — not a separate/parallel implementation.
3. Fix whatever's broken in `main.ts`/`preload.ts`/vite configs to make that true. This is
   integration debugging against already-written code, not new feature work.
4. Confirm `electron-builder` can produce a packaged build locally (doesn't need to be signed
   or distributed — just prove the packaging step itself runs).

## Acceptance Criteria

- [ ] `dev:desktop` launches a real Electron window showing real profile data
- [ ] A local `electron-builder` package build succeeds
- [ ] Draft MR with a screenshot of the running desktop app

## Do NOT

- Do not add new desktop-only features (tray icon, native notifications, etc.) — just get
  the existing scaffolding working.
