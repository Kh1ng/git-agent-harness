# Mistral Admin API fixture provenance

Issue #154. These fixtures back `src/usage/vibe_admin.rs`'s parsers for the
Mistral Admin API endpoints (`/api/admin/analytics/vibe/usage/by_workspace`
and `/api/admin/usage`).

Unlike the `quota-logs` fixtures (copied verbatim from real issue reports),
these endpoints require an authenticated Admin API key against a live
Mistral organization, which was not available in this environment. The
fixture files below are copied from the concrete response examples published
on Mistral's official Admin API docs pages, not from the OpenAPI spec:

| Fixture | Docs source |
|---|---|
| `usage.json` | `https://docs.mistral.ai/api/endpoint/beta/admin/billing` example response for `GET /api/admin/usage` |
| `vibe_workspace_usage.json` | `https://docs.mistral.ai/api/endpoint/beta/admin/analytics` example response for `GET /api/admin/analytics/vibe/usage/by_workspace` |

The remaining Mistral Admin parser branches are exercised with inline test
payloads because the public docs pages only publish the generic response
examples above.
