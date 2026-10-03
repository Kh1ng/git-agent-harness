# Mistral Admin API fixture provenance

Issue #154. These fixtures back `src/usage/vibe_admin.rs`'s parsers for the
Mistral Admin API endpoints (`/v1/admin/analytics/vibe/code/usage/by_workspace`,
`/v1/admin/usage`, `/v1/admin/rate-limit`, and `/v1/admin/spend-limit`).

Unlike the `quota-logs` fixtures (copied verbatim from real issue reports),
these endpoints require an authenticated Admin API key against a live
Mistral organization, which was not available in this environment. The
fixture files below are copied from the concrete response examples published
on Mistral's official Admin API docs pages, not from the OpenAPI spec:

| Fixture | Docs source |
|---|---|
| `usage.json` | `https://docs.mistral.ai/api/endpoint/beta/admin/billing` example response for `GET /v1/admin/usage` |
| `vibe_workspace_usage.json` | Original `https://docs.mistral.ai/api/endpoint/beta/admin/analytics` example, now documented at `https://docs.mistral.ai/api/endpoint/beta/admin/vibe-code-analytics` for `GET /v1/admin/analytics/vibe/code/usage/by_workspace` |
| `rate_limit.json` | `https://docs.mistral.ai/api/endpoint/beta/admin/billing` playground example response for `GET /v1/admin/rate-limit` |

The spend-limit exact-ratio math branch is covered directly in a unit-test
helper because the public docs only publish a placeholder spend-limit example,
so that payload is embedded inline in the parser test rather than stored as a
fixture.

On 2026-10-02, the current [OpenAPI specification](https://docs.mistral.ai/openapi.yaml)
confirmed the `/v1/admin` paths and integer token and prompt counts. The
[authentication guide](https://docs.mistral.ai/admin/admin-api/authentication)
requires a dedicated Backoffice Admin key in `x-api-key`. The generated
playground still shows a Bearer header, so the collector follows the
authentication guide. The current billing schema describes `vibe_usage` as
a legacy field that is always zero. The parser leaves cost unknown for
that value instead of treating it as a current zero bill.
