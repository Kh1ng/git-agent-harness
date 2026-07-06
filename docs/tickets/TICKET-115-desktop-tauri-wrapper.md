# TICKET-115: Strip Electron, rebuild apps/desktop on Tauri

**Priority:** P3 (depends on TICKET-114 — desktop just wraps the web app + server)
**Profile:** gah

## Background

`apps/desktop` is currently an unrun Electron scaffold (`src/main/main.ts`,
`preload.ts`, vite configs for main/preload/renderer, electron-builder config).
**Standing policy: no Electron.** Rip it out and rebuild the desktop wrapper on
Tauri instead (Rust host process + the existing `apps/web` build as the webview
frontend — no separate renderer stack needed).

## Task

1. Delete the Electron scaffold: `main.ts`, `preload.ts`, `vite.main.config.ts`,
   `vite.preload.config.ts`, `electron-builder.config.js`,
   `scripts/start-electron.js`, and the `electron`/`electron-builder` deps in
   `apps/desktop/package.json`.
2. Scaffold `apps/desktop` as a Tauri app (`src-tauri/` with `tauri.conf.json`,
   `Cargo.toml`, minimal `main.rs`) pointing its `frontendDist` at the existing
   `apps/web` build output — no duplicate web UI.
3. Wire whatever `main.ts` was doing (spawning `apps/server` as a child process
   before the window opens) into Tauri's Rust side instead — this project already
   has a working Rust toolchain (`rust-backend/`), so no new language dependency.
4. Confirm the same dashboard (TICKET-114) renders inside the Tauri window,
   backed by the same real `gah` CLI data.
5. Confirm `tauri build` (or `cargo tauri build`) produces a local package —
   doesn't need signing/distribution, just prove packaging runs.

## Acceptance Criteria

- [ ] No `electron` or `electron-builder` references remain anywhere in `apps/desktop`
- [ ] `dev:desktop` launches a real Tauri window showing real profile data
- [ ] A local `tauri build` succeeds
- [ ] Draft MR with a screenshot of the running desktop app

## Do NOT

- Do not add new desktop-only features (tray icon, native notifications, etc.) —
  just get the wrapper working.
- Do not keep Electron as a fallback "in case Tauri doesn't work" — full rip-out.
