# TICKET-120: Don't wire up Clerk auth — rely on network-level access control instead

**Priority:** P3 (decision record / backlog — no active work needed right now)
**Profile:** gah

## Background

Original t3code (the project this web/desktop adaptation is based on) uses Clerk for
auth. As of this ticket, Clerk isn't actually present anywhere in this repo's adapted
code (`grep -rn clerk` across `apps/`/`packages/` and all `package.json` files: zero
hits) — so there's nothing to rip out yet. This is a decision record so future tickets
(TICKET-113/114/115 and beyond) don't accidentally wire it in while porting more of
t3code's surface area.

**Decision:** this is a personal/internal tool. Access control belongs at the network
layer, not the app layer — Tailscale VPN or Cloudflare Access in front of
`apps/server`'s WebSocket/HTTP endpoints, not a hosted auth provider with its own
account system, billing tier, and JS SDK bundled into the frontend.

## Task

Nothing to implement right now. When a future ticket touches auth/session-identity in
`apps/server` or `apps/web`:

- Do NOT add `@clerk/*` packages or a Clerk provider component.
- Assume the deployment sits behind Tailscale or Cloudflare Access already — the app
  itself doesn't need to authenticate users, just needs to trust that anything
  reaching it has already passed perimeter access control.
- If per-request identity is ever needed inside the app (not just "is this request
  allowed to reach us at all"), prefer reading whatever header/claim the chosen
  perimeter tool injects (e.g. Cloudflare Access's `Cf-Access-Jwt-Assertion`) over
  standing up a separate auth system.

## Acceptance Criteria

- [ ] N/A — revisit this ticket only if/when a future ticket proposes adding Clerk or
      similar hosted auth; close that proposal against this decision instead.
