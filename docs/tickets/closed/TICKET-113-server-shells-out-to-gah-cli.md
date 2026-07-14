# TICKET-113: apps/server executes work via the real `gah` CLI, not duplicate driver stubs

Status: archived; the current server integration supersedes this legacy ticket.

**Priority:** P1 (depends on TICKET-112 landing first for tooling, but can be developed against
the current uncompiled tree in parallel — just needs TICKET-112's install/typecheck to verify)
**Profile:** gah

## Background

`apps/server/src/provider/Drivers/` has 8 driver files: `CodexDriver.ts`, `ClaudeDriver.ts`,
`CursorDriver.ts`, `OpenCodeDriver.ts`, `GrokDriver.ts`, `OpenHandsDriver.ts`, `AGYDriver.ts`,
`VibeDriver.ts` (plus `GitHubDriver.ts`/`GitLabDriver.ts`). `builtInDrivers.ts` literally
comments them as "stub drivers for t3code providers we don't fully implement yet." These
duplicate, in half-finished TypeScript, logic that the Rust CLI in this exact repo already
does correctly and with a real track record: backend routing with fallback candidates,
quota/availability tracking, retries, worktree management, ledger recording, MR creation.

`apps/server/src/rustBackend.ts` is the intended bridge to that Rust logic, but it's not
functional: `sendCommand()` writes a line to the child process's stdin and immediately
resolves with the literal string `'Command sent'` — it never reads a real response. The
file's own comment admits this: "we'll need proper JSON-RPC for real bidirectional
communication."

Rather than build that JSON-RPC protocol (a new integration surface to design, implement, and
debug), the CLI itself is already a complete, scriptable interface:
- `gah status --profile <p> --json` → a full machine-readable `StatusSnapshot` (MRs, tickets,
  ledger, availability) — this already exists and is tested (`src/status.rs`).
- `gah dispatch --profile <p> --mode <m> --backend <b> --target <t>` → runs synchronously,
  streaming human-readable progress to stdout, creates a draft MR on success. This is the
  exact command a human operator runs today.
- `gah events --profile <p> --since <ts> --json` → controller event tail.

A session in the web UI IS one `gah dispatch` invocation. There is no need for a persistent
driver abstraction per coding agent — GAH's own `--backend`/`auto` routing already picks and
falls back between backends.

## Task

1. Delete the 8 stub drivers under `apps/server/src/provider/Drivers/` (Codex, Claude,
   Cursor, OpenCode, Grok, OpenHands, AGY, Vibe) and their registration in
   `builtInDrivers.ts`/`ProviderRegistry.ts`. Keep `GitHubDriver.ts`/`GitLabDriver.ts` only if
   `apps/server` actually needs them for something `gah status`'s JSON doesn't already expose
   (check before assuming — it likely doesn't, since `gah status --json` is already meant to
   be the single source of truth here).
2. Replace `rustBackend.ts`'s broken stdin/stdout child-process bridge with a small
   `gahCli.ts` module:
   - `runStatus(profile: string): Promise<StatusSnapshot>` — spawns
     `gah status --profile <p> --json --config <path>`, parses stdout as JSON.
   - `runDispatch(profile, mode, backend, target, onLine: (line: string) => void): Promise<{exitCode: number}>`
     — spawns `gah dispatch ...`, calls `onLine` for each stdout line as it arrives (so the
     caller can forward it as a `session.stdout` WS message), resolves when the process
     exits.
   - `runEvents(profile, sinceIso): Promise<ControllerEvent[]>` — same JSON-parse pattern as
     `runStatus`.
   - Locate the actual `gah` binary the same way `rustBackend.ts` already tries to (release
     build, debug build, then system PATH) — reuse that lookup logic, don't reinvent it.
3. Rewire `SessionManager.ts`/`ProviderService.ts`/`wsServer.ts` to use `gahCli.ts` instead of
   the deleted drivers: starting a session = calling `runDispatch` and forwarding its stdout
   as `session.stdout` messages + a final `session.status` on exit; provider/availability
   data = mapped from `runStatus()`'s availability table, not a live per-driver health check.
4. Keep the existing `ServerMessage`/`ClientMessage`/`Session`/`ProviderInstance` contract
   shapes in `packages/contracts/src/ws.ts` — `apps/web` already expects those exact shapes
   (see `apps/web/src/ws/WebSocketContext.tsx`), so this is a backend implementation swap, not
   a protocol redesign. If a field genuinely can't be sourced from `gah status --json`/
   `gah events --json` output, say so in the MR rather than fabricating a value.

## Acceptance Criteria

- [ ] The 8 stub driver files (and their `builtInDrivers.ts` registrations) are deleted, not
      left in place unused
- [ ] `rustBackend.ts`'s fake `sendCommand` is gone, replaced by `gahCli.ts` calling real
      `gah` subcommands and parsing real JSON output
- [ ] Starting a session from the WS server actually runs `gah dispatch` and streams its real
      output
- [ ] Typecheck passes for `apps/server`
- [ ] Draft MR describing which contract fields are populated from real GAH state vs. still
      unavailable

## Do NOT

- Do not implement a new binary/JSON-RPC protocol between Node and a long-running Rust
  process — shelling out to the existing CLI per action is the whole point of this ticket.
- Do not touch `src/server.rs` (the native Rust WebSocket server) — it's out of scope here;
  this ticket makes the Node server the integration point, not the Rust one.
- Do not modify `packages/contracts/src/ws.ts`'s message shapes unless a field is provably
  impossible to source from the CLI — ask in the MR description rather than guessing.
