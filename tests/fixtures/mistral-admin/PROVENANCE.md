# Mistral Admin API fixture provenance

Issue #154. These fixtures back `src/usage/vibe_admin.rs`'s parsers for the
Mistral Admin API endpoints (`/api/admin/analytics/vibe/usage/by_workspace`,
`/api/admin/usage`, `/api/admin/rate-limit`, `/api/admin/spend-limit`).

Unlike the `quota-logs` fixtures (copied verbatim from real issue reports),
these endpoints require an authenticated Admin API key against a live
Mistral organization, which was not available in this environment. Each
fixture below is a schema-conformant example checked against Mistral's
public OpenAPI specification and the docs reference pages, rather than an
invented response shape:

Source: `mistralai/platform-docs-public`, file `openapi-public-doc.yaml`,
commit `2f0dd463a706828e3af653bfe89c356ae8183605` (fetched 2026-07-21).

| Fixture | Schema | Path/lines in source file |
|---|---|---|
| `rate_limit.json` | `RateLimitsOUT` / `TokenLimitsByModel` | `/api/admin/rate-limit` (~L12213); schema ~L28590-28618 |
| `spend_limit.json`, `spend_limit_reached_no_amount.json` | `LimitsOUT` / `LimitsContext` / `UsageLimits` | `/api/admin/spend-limit` (~L12233); schema ~L28648-28693 |
| `usage.json`, `usage_eur.json` | `UsageOUTJSON` | `/api/admin/usage` (~L12275); schema ~L28834-28913 |
| `vibe_workspace_usage.json` | `VibeWorkspaceStatsOUT` | `/api/admin/analytics/vibe/usage/by_workspace` (~L13127); schema ~L30424-30480 |

Every field name, nesting shape, and required/optional marker in these
fixtures matches that schema exactly. If Mistral changes these schemas,
re-derive the fixtures from the updated spec rather than hand-editing field
names to match old code.
