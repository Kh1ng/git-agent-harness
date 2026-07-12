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

## Verification

Run the ticket's explicit verification commands first. For broad Rust changes,
use `cargo fmt --check`, focused `cargo test`, and `cargo clippy --all-targets
--all-features -- -D warnings` when practical. For dashboard/contracts changes,
run the relevant npm typecheck/build commands. Report commands and results in
the handoff.
