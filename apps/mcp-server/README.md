# gah-mcp-server

MCP (Model Context Protocol) server for GAH (Issue #862). A thin stdio
adapter: each tool call is translated into an HTTP request against
`apps/server`'s already-documented REST API (`docs/openapi.yaml`) — it does
not re-implement any `gah`-calling logic of its own.

## Running

Requires `apps/server` to already be running (defaults to
`http://127.0.0.1:3773`, override with `GAH_SERVER_URL`).

```
GAH_SERVER_URL=http://127.0.0.1:3773 GAH_PROFILE=<profile> node dist/bin.js
```

Point an MCP client (Claude Code, Claude Desktop, etc.) at this command over
stdio. `GAH_PROFILE` sets the default profile for tools that accept one but
don't specify it per-call.

## Auth

Not authenticated on its own — it inherits apps/server's loopback trust
boundary. Running this against a non-loopback `GAH_SERVER_URL` means
sending unauthenticated requests over the network; don't do that without
first enabling auth on the target server (see `authMiddleware.ts`).

## Tools

One tool per HTTP endpoint (`gah_status`, `gah_quota`, `gah_doctor`,
`gah_report`, `gah_profiles`, `gah_work_history`, `gah_sync`,
`gah_ledger_summary`, `gah_ledger_clear_attempts`, `gah_availability`,
`gah_availability_clear`, `gah_hold`, `gah_hold_set`, `gah_hold_clear`,
`gah_dispatch`). `gah_dispatch` returns as soon as the session is created —
it does not block for or stream the dispatch run; poll `gah_status` or use
the dashboard to watch progress.
