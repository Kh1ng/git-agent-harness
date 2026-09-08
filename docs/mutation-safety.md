# HTTP mutation safety

This is a partial implementation of #532. It does not establish policy parity
across HTTP, WebSocket, MCP, and the Rust CLI.

## Protected operations

These POST routes require an `Idempotency-Key` after authentication:

| Route | Operation | Access |
| --- | --- | --- |
| `/api/loop/start` | `loop.start` | Owner or paired device |
| `/api/loop/stop` | `loop.stop` | Owner or paired device |
| `/api/hold/set` | `hold.set` | Owner or paired device |
| `/api/hold/clear` | `hold.clear` | Owner or paired device |
| `/api/availability/clear` | `availability.clear` | Owner or paired device |
| `/api/ledger/clear-attempts` | `ledger.clear_attempts` | Owner |

Keys contain 16–128 ASCII letters, digits, underscores, or hyphens. Generate one
key per intended action. The web and MCP clients supply keys and do not retry
these actions automatically. External callers must supply their own keys.

A receipt binds the key to the authenticated actor, node, operation, and request
digest. JSON object key order does not matter. Changing request values with an
existing key returns `409 idempotency_conflict`.

An exact duplicate returns `409 mutation_already_accepted`, including while the
first request runs and after a server restart. It never repeats the action or
replays a saved response. Refresh the operation's current status before taking
another action. A lost HTTP response does not mean the action failed.

## Owner access

Paired devices can control work and choose chat models. Owner access is required
for registry changes, profile CRUD, global configuration and chat settings,
gateway settings, attempt-history clearing, chat reclamation, and admin updates.
Pairing management and credential export also require the owner.

The current owner identity is the coordinator bearer token or the existing
trusted local request exemption. This is not a per-node capability token system.
In particular, workers that possess the coordinator token still have owner access.

## Durable records

The server stores receipts and `audit.jsonl` under `config/mutations`, relative to
its working directory. `GAH_MUTATION_STORE_PATH` overrides that directory. Files
use mode `0600`; newly created directories use `0700`.

The journal records authenticated requests reaching the six guards, including
missing keys, owner denials, duplicates, acceptance, and HTTP outcomes. Each
record includes actor, node, operation, operation ID, request digest, timestamp,
result, and a fixed reason code. Request bodies, free-text reasons, response
bodies, and credentials are excluded. The digest identifies the target request
without storing its potentially sensitive fields.

The server persists a receipt and acceptance record before starting the action.
Storage failures return `503` and block new execution. If the action completes
but its outcome cannot be recorded, the response reports
`mutation_outcome_unknown`; the receipt still prevents another execution.

A partial receipt or torn final audit line fails closed. Preserve these records
for diagnosis. Do not clear this directory to resolve an API error: removing
receipts allows old requests to execute again. There is no automatic retention
or archival policy yet.

## Remaining work in #532

- Apply capability checks and audit coverage to all mutation entry points,
  including authentication failures and WebSocket actions.
- Bind dispatch request IDs to actor and payload across HTTP and WebSocket.
- Define safe recovery for streamed worker/chat operations and unknown outcomes.
- Replace shared owner-token access with explicit node capabilities.
- Complete previews, secret-reference handling, and CLI policy parity.

The owner guards outside the six listed routes do not yet add journal or
idempotency protection. They must not be treated as complete #532 coverage.
