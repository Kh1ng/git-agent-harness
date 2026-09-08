# PM plan API

The server exposes saved PM plans through the shared contracts in
`packages/contracts/src/pm-plan.ts`. Every response has `schema_version: 1`.
Planning still uses PM dispatch. These endpoints inspect or publish its saved artifact.

| Request | Result |
| --- | --- |
| `GET /api/pm/plans?profile=P&limit=25&cursor=ID` | Plan summaries, per-plan read errors, and `next_cursor` |
| `GET /api/pm/plans/ID?profile=P` | Plan artifact, source identity, dependency graph, saved publication state, and failures |
| `POST /api/pm/plans/ID/dry-run` with `{ "profile": "P" }` | Provider validation and proposed actions without publication |
| `POST /api/pm/plans/ID/publish` with `{ "profile": "P", "approve": true, "plan_fingerprint": "..." }` | Explicit publication through the existing Rust policy checks |

A plan ID names one dispatch session in the configured profile's artifact directory.
IDs cannot contain directory separators. Unknown profiles, foreign artifacts, and symlinked
sessions, artifacts, or publication files are rejected. The API accepts no filesystem path,
configuration path, raw command arguments, repository override, or provider override.

All routes use the shared authentication boundary and a limit of 30 requests per minute.
Responses use `Cache-Control: no-store`. Remote access requires a coordinator token and
TLS, unless the server explicitly permits authenticated plain HTTP.

## Review and publish

1. List plans for the configured profile.
2. Read the selected plan and inspect its tickets and `depends_on` graph.
3. Submit a dry run and inspect `output`, `success`, and `error`.
4. Submit `approve: true` with the detail response's `publication.plan_fingerprint`.

Rust checks the fingerprint while holding the existing profile lock. A changed artifact
requires a new review. Approval does not override backend publication policy.
Profile locks coordinate GAH writers; they do not sandbox local users with artifact-directory write access. The same
publisher preserves native GitHub/GitLab issue markers, partial progress, and retry behavior.
A failed publish can return HTTP 409 with `success: false`, a redacted error, and partial
publication state. Clients must retain that response rather than display an empty plan.

Lists use ascending session-ID order. `limit` is 1–100. Pass `next_cursor` unchanged to
request another page. `updated_at` is the artifact file's modification time; it does not
claim to be the original creation time. Read errors remain in `errors`.

Details include up to 20 recorded failures matched to the session or plan fingerprint.
Legacy failures without either identifier cannot be attributed to a plan. Read and list
requests do not contact GitHub or GitLab. Dry runs contact the configured provider for
validation but do not publish issues or save publication state.

Malformed requests and missing approval return HTTP 400. Authentication errors use the
shared HTTP policy. A plan/configuration read failure returns HTTP 422 without raw CLI
output or local paths. Rate limits return HTTP 429.

## CLI compatibility

`gah pm plans --profile P --json` and `gah pm show --profile P --plan-id ID --json`
use the same projection as HTTP. Remote publication uses
`gah pm publish --profile P --plan-id ID --expected-fingerprint HASH --json`.
Add `--dry-run` to validate without publication; no fingerprint is required for a dry run.

The existing local command `gah pm publish --profile P --plan PATH [--dry-run]` keeps
its text output. HTTP never exposes that arbitrary-path option.

## Integration checks

The integrated PM API and common HTTP guard pass all 337 server checks and full
workspace typechecks. Rust contract drift, CLI capabilities, and all 23 source-structure
checks pass. The worker regression confirms `/api/pm/plans` is central-only.
Generated capability metadata was checked against the combined Rust implementation.

Seven browser scenarios cover first-token recovery in Overview, Chat, Settings, Git,
and Telemetry. The regression checks fail without their reconnect subscriptions.
The memory editor retains unsaved settings after reconnect; labels identify its URL
and key fields. Seven affected Settings component checks also pass.
