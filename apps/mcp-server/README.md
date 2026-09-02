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

For loopback servers, auth inherits apps/server's loopback trust boundary. For
remote/Tailscale servers, set `GAH_SERVER_TOKEN` to the server's coordinator
token; the adapter sends it as a Bearer token. Do not send a token over plain
HTTP.

## Tools

The adapter exposes control-plane and usage tools (`gah_info`, `gah_status`,
`gah_quota`, `gah_usage_rollup`, `gah_doctor`, `gah_report`, `gah_profiles`,
`gah_work_history`, `gah_sync`,
`gah_ledger_summary`, `gah_ledger_clear_attempts`, `gah_availability`,
`gah_availability_clear`, `gah_hold`, `gah_hold_set`, `gah_hold_clear`,
`gah_events`, `gah_controller_activity`, `gah_loop_status`, `gah_dispatch`).
`gah_usage_rollup` supports a 30-day monthly view. `gah_dispatch` waits for the
terminal push event by default; set `waitForCompletion=false` to return as soon
as the session is created.
