# GAH control surfaces

The owner confirmed these requirements on 2026-09-07.

| Surface | Manage the central node | Execute repository work |
| --- | --- | --- |
| Browser | Yes | Through registered workers |
| Windows Tauri app | Yes | Optional WSL worker on the same computer |
| iOS app | Yes | No |
| Android app | Yes | No |

Mobile control surfaces reuse the central dashboard and authenticated connections.
They do not install workers, repositories, Rust, Node, or agent CLIs.
Windows requires a native desktop experience. Its worker can use WSL for Linux tools.
Native Windows execution remains a separate compatibility requirement.

## Pair a browser or remote Tauri dashboard

1. Open the central dashboard with owner access. Select **Pair a device** below the access-token control.
2. Enter the central address that the other device can reach. Do not use localhost for another device.
3. Select **Generate pairing code**. The code expires after five minutes and works once.
4. Scan the QR with the device's camera, or open the displayed pairing link manually.
5. Check the server name, address, server ID, and requested access against the owner screen.
6. Enter a device name and select **Confirm server and pair**.
7. The browser receives a separate 30-day device session. Owner access can list and revoke each device individually.

The QR contains the server address, server ID, and pairing code in a URL fragment.
It contains no coordinator token, GitHub credential, or worker installation command.
The confirmation screen removes the fragment from browser history. Codes never enter HTTP query strings.
Expired, redeemed, wrong-server, and pre-restart codes cannot issue another credential.
Cancelling confirmation does not issue a credential. A copied unused code still grants access until expiry or redemption.

**Pair this device** also provides **Open a pairing link** for manual entry when scanning is unavailable.
A link for another server opens that address before confirmation. App builds contain no fixed LAN or Tailscale address.
This implementation uses the operating system's camera/link handling; it does not request camera permissions itself.

## Access and storage

A paired device gets broad dashboard access: projects, chat, agent work, and settings.
This is a trusted control surface, not a sandbox or a restricted capability role.
Pairing management and credential-bearing node/gateway bootstrap responses require owner access.
Existing backend policy still applies to work. Fine-grained capability scopes remain part of #532.

The browser stores its credential in a host-only, HttpOnly, SameSite=Strict cookie.
It persists across reloads and browser sessions until expiry, logout, or owner revocation.
JavaScript receives device metadata, never the credential. Cookies must be enabled.
The same cookie authenticates HTTP and WebSocket connections at that server origin.
Cookie requests require exact browser origin checks; bearer clients retain their existing transport policy.

The central stores only SHA-256 credential hashes and device metadata in `config/paired-devices.json`.
Use `GAH_DEVICE_STORE_PATH` to choose another path. Atomic replacements use file mode `0600`.
Use one central server process per device store. Protect its directory and backups as administrative state.
Pending codes exist only in process memory. Restarting central invalidates those codes while retaining paired devices.

Revocation rejects subsequent HTTP requests and upgrades, closes the device's existing sockets,
and rejects buffered WebSocket messages before dispatch. It does not cancel work already accepted.
A revoked or expired supplied cookie cannot fall back to local or trusted-LAN access.
Direct loopback access without credentials remains an owner access path under the existing local trust policy.

Use HTTPS through the existing local proxy or secure tunnel. Named server origins must appear in `GAH_WS_ALLOWED_ORIGINS`.
With the explicit `GAH_ALLOW_INSECURE_HTTP=1` setting, pairing can use trusted LAN HTTP.
That mode issues a non-Secure cookie so browsers can actually use it, and confirmation displays the transport warning.
HTTP exposes the cookie to the transport and other services on the same host; use HTTPS outside a trusted deployment.
Proxy logs must omit pairing request bodies and credential headers, including `Cookie`, `Set-Cookie`, `Authorization`, and `Sec-WebSocket-Protocol`.

## Pairing API

| Request | Authority | Result |
| --- | --- | --- |
| `POST /api/pairing/offers` with `origin` | Owner | Versioned offer, code, expiry, server identity, access description |
| `POST /api/pairing/inspect` with `code`, `server_id` | Pending code + same-origin transport | Confirmation details without consuming the code |
| `POST /api/pairing/redeem` with `code`, `server_id`, `name`, `confirm: true` | Pending code + same-origin transport | Device metadata and an HttpOnly session cookie |
| `GET /api/pairing/session` | Authenticated owner/device | Current principal kind and device ID when paired |
| `GET /api/pairing/devices` | Owner | Device metadata; never credential hashes or values |
| `DELETE /api/pairing/devices/:id` | Owner | Immediate credential revocation |
| `POST /api/pairing/logout` | Same-origin transport | Removes this browser's cookie |

Only inspect, redeem, and cookie clearing precede the common API authentication guard.
Pairing routes are unavailable on workers. Responses are not cached; requests are rate-limited.
The shared contract lives in `packages/contracts/src/pairing.ts`.

## Test boundaries

Automated tests cover real HTTP and WebSocket authentication, single-use codes, expiry,
restart invalidation, wrong server/origin, hash-only storage, owner restrictions, and live revocation.
A browser regression covers confirmation, QR/manual links, cookie persistence and attributes,
replay rejection, and a revoked live connection using mock dashboard data with production authentication.

Native iOS/Android scanners, platform keychains, installed webview cookie persistence,
and physical devices are not claimed as tested. Validate iOS and Android separately.
Browser viewport tests do not prove native builds or device behavior.

For physical testing, check camera refusal and manual entry, expired/replayed codes,
wrong servers, revoked credentials, unavailable networks, and browser/app relaunch.
Then check keyboard layout, chat streaming, background/resume, and Wi-Fi/cellular reconnection.
The existing iOS handoff starts with Safari validation; keep it independent from Windows worker installation.
