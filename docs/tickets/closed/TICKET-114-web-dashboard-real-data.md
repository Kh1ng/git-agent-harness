# TICKET-114: Verify apps/web renders real GAH state end-to-end

Status: archived; the current dashboard integration supersedes this legacy ticket.

**Priority:** P2 (depends on TICKET-113 landing — nothing real to render before then)
**Profile:** gah

## Background

`apps/web`'s `DashboardPage`/`SessionsPage`/`ProvidersPage` and `WebSocketContext` are
already written against the `packages/contracts` message shapes, but have only ever run
against a server that fakes its data (see TICKET-113). Once TICKET-113 makes the server push
real `gah status`/`dispatch` output, this ticket is about actually running the dev server and
confirming the UI shows real profile state (MRs, tickets, ledger, availability) rather than
assuming the existing components are correct just because the types line up.

## Task

1. Run the dev stack (`npm run dev` / `pnpm dev`, per whatever TICKET-112 settled on) against
   a real profile (worldcup-props or gah).
2. In a browser, confirm: the dashboard shows the real MR/ticket/availability tables from
   `gah status --json` for that profile, not empty/placeholder state.
3. Trigger a dispatch from the UI and confirm the session view streams real stdout lines and
   ends with a real draft-MR link (or a real failure), matching what running `gah dispatch` by
   hand in a terminal would show.
4. Fix whatever's broken in the existing page components to make the above true — this is
   integration debugging, not a rewrite. Don't add new pages/features.

## Acceptance Criteria

- [ ] Dashboard shows real per-profile data end-to-end
- [ ] A dispatch triggered from the UI streams real output and reaches a real terminal state
- [ ] Draft MR with screenshots or terminal output proving the above (screenshots preferred
      since this is a UI verification ticket)

## Do NOT

- Do not add new UI features beyond making the existing pages correctly show real data.
