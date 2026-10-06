# Git Agent Harness project brief

GAH is a Rust control plane for dispatching repository work to multiple AI
backends, recording execution telemetry, and presenting that state in a web
dashboard. Prefer correctness, observability, and safe failure over throughput.

## Source of truth

- Runtime state and attempts belong in the ledger and session artifacts.
- Ticket requirements, acceptance criteria, and verification commands are the
  authority for a dispatched task.
- `docs/MANAGER_MEMORY.md` is live manager/PM operational state. Worker agents
  do not receive it because it can contain stale status and unrelated backlog.

## Repository shape

- `src/`: Rust CLI, dispatch, routing, ledger, controller, and backend adapters.
- `apps/server/`: control-plane server.
- `apps/web/`: dashboard frontend.
- `packages/contracts/`: shared TypeScript API contracts.
- `docs/tickets/`: tracked work definitions.

## Working rules

- Work in the assigned worktree and branch only. Do not push or create pull
  requests; GAH owns those lifecycle steps.
- Make the smallest coherent change that satisfies the assigned ticket. Do not
  absorb unrelated cleanup or other backlog items.
- Preserve unknown telemetry as unknown; never turn unavailable usage, cost,
  quota, model, or outcome data into zero.
- Backend instance identity, requested model, actual model, and usage class are
  distinct facts and must remain distinguishable.
- Review approval requires concrete evidence for the changed behavior and any
  relevant compatibility boundary. Missing evidence is a human-review outcome.

## Design rules

After *A Philosophy of Software Design* (Ousterhout). These rules shape the
code the ticket needs; they are not a reason to widen the change. Refactor only
code the ticket already touches, and file an issue for design debt beyond it.
Only implementation and fix prompts receive this section; reviews gate on
evidence, not on these rules.

- Prefer deep modules: a small interface over a substantial implementation.
- Each layer offers a different abstraction. Do not add a function, type, or
  wrapper that only forwards to another.
- Hide each design decision in one place. When a format, default, or rule you
  change is spelled out in two modules, make the copy you touch the owner and
  file an issue for the other.
- Pull complexity downward: handle the hard case inside the module instead of
  adding a parameter, flag, or config knob for every caller.
- Define errors out of existence where the caller can do nothing useful, and
  handle the rest once, close to the cause. Never hide a failure the operator
  must see. Missing telemetry is not an error to define away: keep unknown
  usage, cost, quota, model, or outcome data unknown.
- Choose precise names. A name that fits two different things is wrong; use
  one word per concept.
- Comments say what the code cannot: why, invariants, units, and what a
  caller must know. Do not restate the code.
- When a design choice is not obvious, sketch a second design before writing
  the first, and say in the handoff why you chose this one.
- When you modify code, leave its design as if the change had been planned
  from the start, and only in code the ticket already touches.

## Verification

Run the ticket's explicit verification commands first. For broad Rust changes,
use `cargo fmt --check`, focused `cargo test`, and `cargo clippy --all-targets
--all-features -- -D warnings` when practical. For dashboard/contracts changes,
run the relevant npm typecheck/build commands. Report commands and results in
the handoff.
