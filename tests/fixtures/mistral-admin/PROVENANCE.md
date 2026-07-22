# Mistral Admin API fixture provenance

Issue #154. These fixtures back `src/usage/vibe_admin.rs`'s parsers for the
Mistral Admin API endpoints (`/api/admin/analytics/vibe/usage/by_workspace`,
`/api/admin/usage`, `/api/admin/rate-limit`, `/api/admin/spend-limit`).

Unlike the `quota-logs` fixtures (copied verbatim from real issue reports),
these endpoints require an authenticated Admin API key against a live
Mistral organization, which was not available in this environment. Each
fixture is therefore hand-built to conform exactly to the real, authoritative
response schema published by Mistral itself, rather than to any invented
shape:

Source: `mistralai/platform-docs-public`, file `openapi-public-doc.yaml`,
commit `2f0dd463a706828e3af653bfe89c356ae8183605` (fetched 2026-07-21).

| Fixture | Schema | Path/lines in source file |
|---|---|---|
| `rate_limit.json` | `RateLimitsOUT` / `TokenLimitsByModel` | `/api/admin/rate-limit` (~L12213); schema ~L28590-28618 |
| `spend_limit.json`, `spend_limit_reached_no_amount.json` | `LimitsOUT` / `LimitsContext` / `UsageLimits` | `/api/admin/spend-limit` (~L12233); schema ~L28648-28693 |
| `usage.json`, `usage_eur.json` | `UsageOUTJSON` | `/api/admin/usage` (~L12275); schema ~L28834-28913 |
| `vibe_workspace_usage.json` | `VibeWorkspaceStatsOUT` | `/api/admin/analytics/vibe/usage/by_workspace` (~L13127); schema ~L30424-30480 |

Every field name, nesting shape, and required/optional marker in these
fixtures matches that schema exactly (verified against the raw YAML, not
against the docs site's auto-generated example values, which use
placeholder text like `"ipsum eiusmod"` and are not representative). Numeric
values are plausible hand-picked stand-ins, not fabricated field shapes.

Do not add a fixture here without checking it against the live schema (or a
real captured response, once account access exists) first. If Mistral changes
these schemas, re-derive the fixtures from the updated spec rather than
hand-editing field names to match old code.

## Known gap: not yet real captures

Issue #154's AC4 ("real test fixtures backing the parser") is not fully met by
this directory today, unlike every fixture in `../quota-logs/`. These four
files are schema-conformant synthetic data, not a captured response from a
live Mistral organization -- no Admin API key/account was available in this
environment to make that call. Per this repo's own review-approval rule
("missing evidence is a human-review outcome"), this gap is intentionally
left visible here rather than papered over: replacing these four fixtures
with real captured `/api/admin/...` responses (and updating this file the
way `quota-logs/PROVENANCE.md` documents its captures) requires a human with
a live Mistral Admin API key to run `refresh_admin_data`
(`src/usage/vibe_admin.rs`) once against a real account and save the
responses verbatim.
