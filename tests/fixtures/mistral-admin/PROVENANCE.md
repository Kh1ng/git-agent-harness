# Mistral Admin API fixture provenance

Issue #154. These fixtures back `src/usage/vibe_admin.rs`'s parsers for the
Mistral Admin API endpoints (`/api/admin/analytics/vibe/usage/by_workspace`,
`/api/admin/usage`, `/api/admin/rate-limit`, and `/api/admin/spend-limit`).

Unlike the `quota-logs` fixtures (copied verbatim from real issue reports),
these endpoints require an authenticated Admin API key against a live
Mistral organization, which was not available in this environment. The
fixture files below are copied from the concrete response examples published
on Mistral's official Admin API docs pages, not from the OpenAPI spec:

| Fixture | Docs source |
|---|---|
| `usage.json` | `https://docs.mistral.ai/api/endpoint/beta/admin/billing` example response for `GET /api/admin/usage` |
| `vibe_workspace_usage.json` | `https://docs.mistral.ai/api/endpoint/beta/admin/analytics` example response for `GET /api/admin/analytics/vibe/usage/by_workspace` |
| `rate_limit.json` | `https://docs.mistral.ai/api/endpoint/beta/admin/billing` playground example response for `GET /api/admin/rate-limit` |
| `spend_limit.json` | `https://docs.mistral.ai/api/endpoint/beta/admin/billing` playground example response for `GET /api/admin/spend-limit` |

The spend-limit exact-ratio math branch is covered directly in a unit-test
helper because the public docs only publish a non-numeric spend-limit example.
