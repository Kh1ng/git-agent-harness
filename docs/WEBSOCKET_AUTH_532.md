# HTTP and WebSocket authentication for #532

Central WebSocket upgrades require the existing `COORDINATOR_TOKEN` or a valid paired-device cookie outside
verified direct local access. Workers require the coordinator token. An unauthorized
upgrade receives no welcome payload and cannot enter the message handlers.

The browser and Tauri dashboard share the **Central access token** control above
the dashboard content. Saving reconnects the live connection and refreshes
mounted REST panels, including after their first unauthenticated request failed.
The token stays in `sessionStorage` for that tab. Add a Node uses the same saved
token. Native fleet connections retain their `Authorization: Bearer` header.

Browser WebSockets send `gah.v1` and a `gah-auth.<base64url-token>` subprotocol.
The server selects only `gah.v1`. The credential never enters a request URL or
response protocol. Proxy logs must omit credential-bearing headers, including
`Authorization` and `Sec-WebSocket-Protocol`.

All ordinary `/api` HTTP routes use that same authentication boundary, including legacy
configuration, dispatch, loop, profile, and manager-chat endpoints. This also protects
new routes by default. Remote reads require credentials because configuration and chat
responses can expose private data. `/health` remains public for connection checks.
The narrow pairing code exchange and cookie clearing exception is described in [CONTROL_SURFACES.md](CONTROL_SURFACES.md).

Web, desktop, and MCP callers send the existing bearer token. Custom integrations that
previously called unguarded routes must send `Authorization: Bearer TOKEN` and use TLS
or the explicit HTTP opt-in below. Direct same-origin loopback access is unchanged.

## Deployment settings

- The default rejects unauthenticated remote WebSockets. Local CLI and same-origin
  loopback browsers keep token-free central access. Worker WebSocket upgrades require a token
  even over loopback. HTTP retains the direct-local exemption for both roles.
- Remote transport requires TLS, or the existing explicit
  `GAH_ALLOW_INSECURE_HTTP=1` opt-in. That option does not disable authentication.
- A local TLS proxy may supply `X-Forwarded-Proto: https`. Remote peers cannot
  assert TLS through that header. Forwarded requests never inherit local access.
- Browsers using literal IP addresses or localhost must match the target origin.
  For named hosts or a deliberate cross-origin frontend, set comma-separated
  exact origins in `GAH_WS_ALLOWED_ORIGINS`, for example
  `https://gah.example.test,http://localhost:3000`. Include the server's named
  origin as well as any separate frontend origin. Do not configure wildcard or
  untrusted origins. Native bearer clients without Origin need no browser-origin
  exception. Tauri opens the remote dashboard origin; its local command window
  needs no exception.

## Explicit compatibility mode

`GAH_WS_AUTH_MODE=trusted_lan` enables unauthenticated central browser access for
live status, local manager chat, and provider operations. The dashboard displays
an undismissed warning while this mode is enabled. The transport and browser
origin checks still apply. Invalid supplied credentials are rejected instead of
falling back to compatibility access.

Compatibility sockets cannot call `session.*`: those operations use the fleet
coordinator even without an explicit remote node. Supply a token to start, stop,
or send commands to sessions. This mode never weakens worker, registry, claims,
or node-setup authentication. It applies only to WebSockets. Remote HTTP reads and
mutations still require owner or paired-device credentials, so this mode alone does not provide a usable
unauthenticated dashboard. Save a token or pair the device to use dashboard controls.

## Verification and remaining work

Real-upgrade tests cover rejection before welcome/handlers, native bearer and
browser protocol credentials, public protocol selection, origin/rebinding checks,
proxy TLS, worker policy, and compatibility downgrade rejection. A real handler
test checks the visible mode and proves that compatibility session operations do
not call the fleet coordinator. Browser checks cover token reconnect, first
successful connection restoring a failed REST panel, and Add a Node retaining
the shared token. The connection control and warning are checked at desktop and
390-pixel widths.

A real HTTP regression verifies legacy routes, every mutation method, and unknown
future routes reject unauthenticated requests. It also checks valid and invalid tokens,
TLS policy, local access, and public health checks. The regression fails before the
common API guard is applied.

This does not complete #532. Fine-grained device scopes, capability policy, idempotency,
policy parity, and append-only operation auditing remain open. Paired devices can be
revoked individually, including existing sockets. Changing the coordinator token still
affects new upgrades only; it does not revoke already-open owner sockets. Physical Windows, iOS, and Android validation remains pending.
