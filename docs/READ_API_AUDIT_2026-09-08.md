# Typed read API audit, 2026-09-08

[Issue #519](https://github.com/Kh1ng/git-agent-harness/issues/519) remains open.
Reviewed `ac544db` and PM plan work `57d4246`, including its integration at
`d8a6078`. This audit covers CLI/API parity, not MCP or device pairing.

The report-query follow-up on `2f5aef5` closes the grouping and series-default
gaps. Report and ledger-summary routes now accept all five Clap grouping
values through one shared vocabulary. Invalid scalar or structured values
return 400 before spawning the CLI. An omitted series window now uses `7d`.
The web chart still requests `14d` explicitly; existing web and MCP
`backend`/`model` requests remain valid. Ledger group arrays and nullable
counters now have shared TypeScript types.

The API already exposes status, quota snapshots, doctor, reports/series,
work history, sync, ledger summaries, availability, events, profiles, and
config projections. The PM change adds plan list/detail and publication
dry-run. These routes do not establish complete parity with the CLI.

| Missing HTTP operation | Existing CLI support and remaining work |
| --- | --- |
| `telemetry.aggregate` | JSON exists in `telemetry::cli::run_aggregate`. Preserve dimensions, date bounds, optional profile, failure/retry flags, and all attribution filters. No typed HTTP adapter or manifest response schema exists. |
| `claims.list` | JSON exists in `handle_claims_list`. Global results include a canonical profile scope; scoped results omit that field. Central claim leases represent different state. Listing also migrates and rewrites local claim state under the existing lock. |
| `external_approval.inspect` | JSON exists in `external_approval::inspect`. It requires profile, work ID, credential label, and operation kind. Its private `ScopeStatus` includes a local ledger path, so define the permitted remote projection and redaction before exposing it. |
| `quota.list` | JSON exists for persisted observations. `/api/quota` returns a computed profile snapshot instead. The quota-list follow-up now propagates store read failures, while a missing store remains empty. The typed HTTP adapter is still missing. |

CLI sources: [telemetry commands](../src/cli/commands/telemetry.rs),
[telemetry output](../src/telemetry/mod.rs),
[claim commands](../src/cli/commands/claims.rs),
[claim storage](../src/work_claim.rs),
[approval inspection](../src/cli/commands/external_approval.rs), and
[quota commands](../src/cli/commands/quota.rs).

Some requested reads need CLI work before a fixed-argv JSON wrapper can exist:

| Operation | Unsupported prerequisite |
| --- | --- |
| `telemetry.status` | No JSON mode or structured repository-status result. The CLI also accepts an arbitrary telemetry repository path. Remote access needs configured storage selection. |
| `policy.check` | Prints `allowed` or `blocked`, exits on denial, and requires a policy-file path. It needs a structured decision and configured policy resolution. |
| `profile.show` | Prints human-readable profile fields and has no JSON flag. `/api/config/effective` already provides a redacted profile projection, but is a different command/output contract. |
| Price checks | `price-guard` takes an arbitrary watchlist path and prints a decision. It has no JSON mode or capability-manifest entry. |

See [CLI arguments](../src/cli/args.rs), [policy execution](../src/policy.rs),
[price checks](../src/price_guard.rs), and
[profile output](../src/cli/commands/profile.rs). Other dry-run commands require
individual classification; PM dry-run and doctor readiness do not cover every
diagnostic mentioned by #519.

Remaining acceptance gaps also affect existing routes:

- The [capability generator](../src/cli/capabilities/generation.rs) generates
  manifest metadata, not operation payload contracts. Many schema references
  are null. [gah.ts](../packages/contracts/src/gah.ts) explicitly requires
  manual updates alongside Rust. A generated manifest alone does not satisfy
  the generated-contract criterion. `remote_available` currently includes
  operations with no HTTP route.
- Events and doctor still force a default profile while their CLI commands
  permit an omitted profile. Report/ledger `since` supports relative windows
  such as `7d` and `24h`; absolute date bounds belong to `telemetry.aggregate`.
- Array responses such as work history, sync, events, and profiles have no
  response metadata. Other payloads provide only some of node identity, schema
  version, and source timestamp. PM responses add version/time fields but no
  server identity. Request time must not impersonate an unknown source time.
- Most legacy read failures return raw subprocess or JSON-parse error text.
  Those messages can contain paths and CLI output. The PM adapter already
  hides raw errors; its handling does not protect the other adapters.
- [contracts_drift.rs](../tests/contracts_drift.rs) compares JSON shape and
  value kinds, not equal values. [gahFixtureEndpoints.test.ts](../apps/server/src/gahFixtureEndpoints.test.ts)
  checks selected fields against a fixture executable. Neither proves all
  CLI/API values, filters, unknowns, and provider differences match.

The next inventory task is to record each manifest read operation as
implemented, partial, or unavailable with a concrete reason. Test that the
inventory covers the generated manifest so future reads cannot disappear
silently. The report-query patch adds no new endpoints or response envelopes.

The next missing endpoint should be `telemetry.aggregate`: it already has a
structured result and configured-ledger reads. Reuse the existing subprocess
helper, accept only named query fields, and keep filesystem paths server-owned.
Resolve payload generation and versioned response metadata before claiming
the remaining #519 criteria. Replacing legacy arrays requires a coordinated
client migration or an additive versioned API.

The original audit used read-only source and issue inspection. The follow-up
adds [reportQuery.test.ts](../apps/server/src/reportQuery.test.ts), which
checks exact child argv and complete replayed JSON for all groupings,
default/explicit relative windows, and optional profiles. Invalid groupings
include repeated and structured query parameters. A separate local check
compared 39 native CLI JSON results with HTTP responses using an isolated
synthetic ledger. It made no provider calls. Fixture tests replay captures;
they do not run the native CLI or prove every #519 acceptance criterion.
