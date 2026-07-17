# Execution Identity: Canonical Contract (1/5)

Status: **proposed — pending owner sign-off** (see [Sign-off](#sign-off)).
Scope: documentation and executable golden fixtures/tests only. This ticket
does not introduce a new production type and does not change routing,
ledger, or telemetry behavior. Parts 2/5–5/5 thread the canonical type
through production against this contract.

## Problem

Runner kind, executable, logical backend, instance/account, quota pool,
authentication source, requested model, and effective model are represented
by overlapping strings scattered across `src/config.rs`, `src/routing/`,
`src/availability.rs`, `src/ledger/`, and `src/usage_attribution.rs`. Nothing
enforces that these strings agree, so a migration without an approved
contract risks silently changing routing behavior or corrupting historical
usage/cost attribution. This document is the contract that migration must
preserve, plus the golden fixtures that pin today's behavior as the
before-state.

## 1. Canonical identity fields

| Field | Meaning | Today's representation | Owner |
|---|---|---|---|
| `runner_kind` | The agent CLI family GAH knows how to invoke (`claude`, `codex`, `openhands`, `vibe`, `opencode`, `agy`). | Implicit in `runner::backend_command_name()`; not a distinct value today. | `src/runner/resolve.rs` |
| `executable` | The resolved on-disk path GAH actually invokes for a `runner_kind` in a given profile. | `runner::resolve_backend_executable()` / `ExecutableResolution`. | `src/runner/resolve.rs` |
| `logical_backend` | The string used everywhere as "backend": `requested_backend`, `effective_backend`, `CandidateConfig.backend`, `AttemptRecord.backend`. May diverge from `runner_kind` for multi-account runners (`agy-second` shares the `agy` runner) or historical aliases (`cloud-coder` shares the `openhands` runner). | Plain `String` throughout config/routing/ledger. | `src/config.rs`, `src/routing/` |
| `instance_account` / `quota_pool` | Which credential/session/home a `logical_backend` used. | Config: `Profile.agy_second_home`, `CandidateConfig.quota_pool`. Runtime: `RouteDecision.effective_quota_pool`, `AvailabilityRecord.quota_pool`. Telemetry: `LedgerUsage.account_label`, folded into `LedgerUsage.backend_instance`. | `src/config.rs`, `src/availability.rs`, `src/usage_attribution.rs` |
| `auth_class` / `usage_classification` | `quota_backed` \| `api_key_backed` \| `local_unmetered` \| `unknown` \| `mixed` \| `mixed_or_unknown`. | `LedgerUsage.usage_classification`, derived in `usage_attribution::normalize_attempt_usage`. | `src/usage_attribution.rs` |
| `provider` | Inferred model vendor (`anthropic`, `openai`, `google`, `mistral`, `deepseek`, `z-ai`, `tencent`, `local`), never an account identifier. | `LedgerUsage.provider`, derived in `usage_attribution::provider_for_model`. | `src/usage_attribution.rs` |
| `requested_model` | What routing was asked for, before any runtime substitution. | `RouteRequest.requested_model`, `RouteDecision.requested_model`, `LedgerEntry.requested_model`. | `src/routing/`, `src/ledger/entry.rs` |
| `effective_model` | What routing decided to dispatch for this attempt/entry, after fallback and any `--model` override. | `RouteDecision.effective_model`, `LedgerEntry.effective_model`, `AttemptRecord.effective_model`, `AttemptRoutingRecord.effective_model`. | `src/routing/`, `src/ledger/entry.rs` |
| `actual_model` | What the backend itself reported using, post-execution. May differ from `effective_model` when a proxy/alias substitutes a different concrete model. | `LedgerUsage.actual_model` (+ `actual_model_unknown_reason`). | `src/usage_attribution.rs` |
| `fallback_used` / `routing_reason` / `routing_diagnostics` | Whether/why `effective_*` diverged from `requested_*`. | `RouteDecision.fallback_used`, `.routing_reason`, `.routing_diagnostics: Option<RoutingDiagnostics>`. | `src/routing/` |

Companion "unknown reason" fields (`actual_model_unknown_reason`,
`provider_unknown_reason`, `token_usage_unknown_reason`,
`quota_unknown_reason`, `cost_unknown_reason` — all on `LedgerUsage`) are part
of the contract, not incidental: whenever the corresponding fact is `None`,
its reason field must be populated. A fact with no reason and no value is a
bug, not a legitimate state.

## 2. Ownership boundaries

```
config.rs           declares intent: runner paths/homes, CandidateConfig
                     {backend, model, quota_pool, included_in_quota,
                     requires_approval}. Declares "what may be used and
                     with what cost policy," never "what was used."
        |
routing/             resolves requested -> effective for one route,
                     including fallback and RoutingDiagnostics
                     (selected_cost_class, selected_quota_pool, ...).
        |
availability.rs      durable per-(backend, model, quota_pool) eligibility
                     state (AvailabilityRecord). Read-only input to
                     routing, written by runner failures/manual ops.
        |
runner/               resolves logical_backend + config -> executable.
        |
usage_attribution.rs  turns a RouteDecision + raw backend usage output
                     into LedgerUsage's identity + classification fields
                     (backend_instance, usage_classification, provider,
                     actual_model, quota_window, ...).
        |
ledger/entry.rs      durable schema (LedgerEntry, AttemptRecord,
                     AttemptRoutingRecord, LedgerUsage) written to
                     ledger.jsonl. This is the source of truth.
        |
ledger/sqlite.rs,    read-only derived views. Must read the already-
status.rs, sync.rs,  normalized fields and must never re-derive identity
report.rs,           facts from raw CLI strings independently.
telemetry/
```

Rule: identity facts flow strictly downward through this chain. A consumer
below `ledger/entry.rs` (SQLite mirror, status, sync, report, telemetry)
must not compute its own notion of `backend_instance` or
`usage_classification` — it reads the ledger's.

## 3. Normalization rules

- **Alias folding.** `config::canonical_backend_name()` merges known
  `logical_backend` aliases that execute the same `runner_kind` — today only
  `cloud-coder` → `openhands` (`src/config.rs:911`). Applied both where new
  dispatches are routed and when grouping historical ledger data, so
  pre-existing entries recorded under the old alias are merged too.
  `auto` is deliberately never folded: its effective backend is resolved
  per-attempt by `routing::decide`, not a fixed alias.
- **`backend_instance` composition.** `logical_backend` alone when no
  `quota_pool` is set, else `"{logical_backend}:{quota_pool}"`
  (`usage_attribution::normalize_attempt_usage`). Never includes a raw
  account identifier, file path, or secret.
- **No implicit case-folding.** Backend/model strings are recorded verbatim
  as configured/observed; the only normalization is the explicit alias
  table above. Configs must use the canonical literal spelling.
- **Provider inference precedence.** Exact model-substring match first
  (`claude`/`sonnet`/`haiku`→`anthropic`, `gemini`→`google`,
  `mistral`/`devstral`→`mistral`, `deepseek`→`deepseek`,
  `glm`/`z-ai`→`z-ai`, `hy3`/`tencent`→`tencent`, `gpt-`/`openai`→`openai`,
  `ollama`/`local/`→`local`), falling back to a fixed
  `logical_backend`→provider table only when the model string does not
  resolve (`usage_attribution::provider_for_model`).

## 4. Unknown semantics

- Absence (`None`) means "not observed." For `actual_model`, `provider`,
  token usage, `quota_*`, and cost, absence is always paired with the
  matching `<field>_unknown_reason` string.
- `usage_classification: "unknown"` is an explicit, distinct value for a
  `logical_backend` with no recognized cost-class mapping. It must never be
  treated as `local_unmetered` or zero-cost.
- Aggregating across attempts (`usage_attribution::aggregate_attempt_usage`)
  uses `"mixed"` when every attempt reported a different concrete value, and
  `"mixed_or_unknown"` when values differ **and** at least one is absent —
  it never silently picks one attempt's value as the summary.
- A `None`/absent identity or usage fact must never be coerced to `0`, `""`,
  or a default enum variant anywhere downstream (ledger write, SQLite
  mirror, report aggregation, telemetry export). This is the same rule the
  ledger schema history already encodes: `LEDGER_SCHEMA_VERSION` bumped from
  `1`→`2` specifically because plain `u32` attempt counters coerced
  "unknown" to a literal `0` (`src/ledger/entry.rs:240-253`, issue #240).

## 5. Secret-safe labels

Only `backend_instance`, `account_label`, `quota_pool`, and `provider` may
appear in logs, the ledger, telemetry, or the dashboard. These are always
operator-chosen logical strings from config (e.g. `"claude-main"`,
`"nous-portal-api"`), never raw API keys, tokens, or filesystem paths.
`executable` and `*_path`/`*_home` config fields (`claude_path`,
`agy_second_home`, ...) are resolution *inputs* only and must never be
copied into `LedgerUsage` or telemetry. `src/redact.rs` remains the last
line of defense scrubbing token-shaped strings (`gh[pousr]_...`, `glpat-...`,
`sk-...`, `Authorization: Bearer ...`) out of any raw backend output before
it can reach a durable sink; canonical identity fields must be constructed
from config-declared labels so they never need redaction in the first place.

## 6. Equality / keying rules

- **Availability / quota-pacing scope key:**
  `(logical_backend, model: Option<String>, quota_pool: Option<String>)` —
  matches `AvailabilityRecord`'s three optional-narrowing fields and
  `BlockScope::{BackendWide, ModelSpecific, QuotaPool}`
  (`src/availability.rs:90-134`).
- **Routing dedup / attempt-tracking key:** `CandidateIdentity { backend,
  model }` (derives `Eq`/`Hash`, `src/routing/types.rs:40-53`). This
  deliberately does **not** include `quota_pool` — today two accounts of the
  same runner sharing a model (`agy` vs `agy-second` both on the same
  model) are treated as the *same* "already attempted" candidate for
  retry-diversity purposes. 2/5 must explicitly decide whether to widen
  this key to `(backend, model, quota_pool)`; this contract intentionally
  does not silently change it.
- **Cross-system join key for one execution:** `(work_id, attempt_number)`
  for attempt-scoped identity (`AttemptRecord`, `AttemptRoutingRecord`);
  `(work_id)` alone for the top-level/aggregated identity. Never
  `(backend, model)` alone — the same pair recurs across many work items.

## 7. Auth / cost class taxonomy

Four classes, distinguished **without ever storing credentials**:

| Class | Meaning | Trigger (today, `usage_attribution::normalize_attempt_usage`) | Cost fields |
|---|---|---|---|
| `quota_backed` | Subscription/included-quota execution. | `selected_cost_class == "included_quota"`, or any of the built-in subscription backends (`claude`, `codex`, `vibe`, `agy`, `agy-main`, `agy-second`) with no cost class signal. | `actual_cost_usd`/`estimated_cost_usd`/`pricing_source`/`pricing_version` are always cleared — a provider-reported "API-equivalent" dollar figure from a subscription CLI is never recorded as spend. |
| `api_key_backed` | Metered, pay-per-use execution. | `selected_cost_class == "paid"`. | Cost fields are preserved when the backend/pricing table reports them; otherwise `cost_unknown_reason` explains why. |
| `local_unmetered` | Local/self-hosted model with no metered charge. | `opencode` backend with an `ollama`/`local/` model. | `actual_cost_usd = Some(0.0)` with `pricing_source = "local_unmetered"` — a **known** zero, not a missing value. |
| `unknown` | No recognized classification. | Any other `logical_backend` with no cost-class signal. | `cost_unknown_reason` is always set; cost fields stay `None`. |

`mixed`/`mixed_or_unknown` are aggregate-only values (see §4), never
per-attempt classifications.

## 8. Requested vs. effective vs. actual model, and fallback attribution

- `requested_backend`/`requested_model` — the caller's ask (CLI
  `--backend`/`--model`, or `"auto"`).
- `effective_backend`/`effective_model`/`effective_quota_pool` — what
  routing actually selected after availability/approval/fallback
  (`RouteDecision`).
- `actual_model` (usage-level) — what the backend itself reports after
  execution; may still diverge from `effective_model` for proxy/alias
  backends whose CLI model string is itself a routing alias for a different
  concrete underlying model (e.g. OpenCode's
  `"nous-portal/z-ai/glm-5.2"`).
- **Attempt-level truth is authoritative for retries.** `AttemptRecord`
  and the parallel `AttemptRoutingRecord` are captured per attempt
  specifically so a mid-dispatch retry that changed backend keeps *each
  attempt's own* identity instead of being overwritten by the final
  attempt's values. `LedgerEntry.effective_backend`/`.effective_model` at
  the top level reflect only the **last attempt with a non-empty
  `effective_backend`** (`src/sync.rs:299-318`) — they are a summary, not
  per-attempt authoritative truth.
- `fallback_used` is `true` exactly when `effective_backend`/
  `effective_model` differ from `requested_backend`/`requested_model`
  because the originally requested candidate was unavailable/unapproved and
  a lower-priority candidate was substituted. `routing_reason` and
  `routing_diagnostics.human_summary` carry the human-readable why;
  `routing_diagnostics.selected_over` lists what was skipped.

## 9. Legacy compatibility

| System | Current shape | Compatibility rule |
|---|---|---|
| JSONL ledger (`ledger.jsonl`) | Source of truth. `LedgerEntry`/`AttemptRecord`/`LedgerUsage` fields added since `LEDGER_SCHEMA_VERSION 1` are all `#[serde(default)]`. | Historical lines missing any identity field must keep deserializing to `None` (never a default identity value). `schema_version` (default `1` when absent) is itself part of the contract and must not be inferred from field presence. |
| SQLite mirror (`ledger/sqlite.rs`) | Derived, **non-authoritative** projection with a narrow column subset (`backend`, `effective_backend`, `effective_model`, `requested_model`, ...). | Migration must either extend these columns to carry the new canonical fields, or explicitly document that newly-canonical fields (e.g. `usage_classification`, `backend_instance`) remain JSONL-only and are not queryable via the SQLite mirror yet. Never treat the mirror as authoritative for identity (see `src/ledger/mod.rs:23-28`). |
| Availability (`availability.json`) | `AvailabilityRecord{backend, model, quota_pool}`, append-only. | Already the closest existing shape to the canonical `(logical_backend, model, quota_pool)` key (§6); migration is additive only — no field renames. |
| Quota (`quota.rs`, `quota_store.rs`, `quota_snapshot.rs`) | Pools keyed by string, `config::canonical_backend_name` applied ad hoc at read sites (e.g. `quota_snapshot.rs:175,534`). | Must resolve `logical_backend`/`quota_pool` through the same normalization function everywhere; migration should centralize the current per-callsite `canonical_backend_name()` calls rather than duplicate the alias table again. |
| Config (`config.rs`) | `CandidateConfig{backend, model, quota_pool, included_in_quota, requires_approval}` is the declared-intent boundary. | `auth_class` is a **runtime-derived** fact (from `included_in_quota`/cost class), not something config stores directly — migration must not add a redundant `auth_class` config field that could disagree with the derived value. |
| Status (`status.rs`) | `most_recent_effective_backend`/`most_recent_effective_model` are read-only projections of the ledger's last entry. | Must keep reading the already-normalized ledger fields; never re-infer identity from raw CLI strings. |
| Telemetry / report / sync (`telemetry/`, `report.rs`, `sync.rs`) | Group/aggregate by `effective_backend`/`effective_model`/`usage.*`. | Must key on the equality rules in §6 (especially `backend_instance` for account-level breakdowns) once the canonical type is introduced; aggregation must preserve the `mixed`/`mixed_or_unknown` distinction (§4), never collapse it. |

## 10. Golden fixture cases

Implemented in `tests/execution_identity.rs` (`cargo test execution_identity`).
Each fixture is a legacy-shaped JSON/struct value (as actually written by
production today) plus the canonical value the compatibility adapter (§11)
must produce from it.

1. **Two accounts, one runner** — `agy` and `agy-second` share the same
   `runner_kind`/executable but are distinct `logical_backend`,
   `instance_account`, and `quota_pool` values. Test:
   `execution_identity_golden_two_accounts_one_runner`.
2. **One model through subscription and API** — the same model routed once
   with `selected_cost_class = "included_quota"` (→ `quota_backed`, cost
   fields cleared) and once with `"paid"` (→ `api_key_backed`, cost fields
   preserved). Test: `execution_identity_golden_subscription_vs_api_same_model`.
3. **Proxies/aliases** — `cloud-coder` folds to the `openhands` runner via
   `config::canonical_backend_name` (a real call into production code, not
   a re-implementation); OpenCode's `"nous-portal/z-ai/glm-5.2"` model
   string is a proxy path whose inferred `provider` (`z-ai`) differs from
   its `logical_backend` (`opencode`). Test:
   `execution_identity_golden_proxy_alias`.
4. **Fallback substitution** — `requested_backend != effective_backend`,
   `fallback_used = true`, with distinct requested vs. effective identity
   preserved at both the top level and per-attempt
   (`AttemptRoutingRecord`). Test:
   `execution_identity_golden_fallback_substitution`.
5. **Legacy unknowns** — a minimal, pre-`LEDGER_SCHEMA_VERSION 3` JSON
   ledger line missing every new identity/usage field deserializes through
   the **real** `git_agent_harness::ledger::{LedgerEntry, AttemptRecord,
   LedgerUsage}` types with those fields as `None`, never a coerced
   default. Test: `execution_identity_golden_legacy_unknown`.

## 11. Compatibility adapter and byte-for-byte equivalence

Because this ticket does not thread a new type into production,
`tests/execution_identity.rs` defines a small, test-local
`ExecutionIdentity` struct and an `adapt_legacy_usage()` function that maps
each golden fixture above onto it, mirroring exactly the rules in §§1–9 (the
mapping is transcribed from, not a stand-in for, the production logic in
`usage_attribution::normalize_attempt_usage`/`provider_for_model` and
`config::canonical_backend_name`, the latter called directly since it is a
public function). This is the adapter parts 2/5–5/5 must reproduce when the
canonical type is threaded through production.

Separately, `execution_identity_route_decision_regression_baseline` runs one
real end-to-end dispatch through `ScenarioHarness` (the same harness used by
`tests/usage_telemetry_regression.rs`) and asserts the exact current ledger
output (`requested_backend`, `effective_backend`, `effective_model`,
`fallback_used`, `usage.backend_instance`, `usage.usage_classification`)
field-for-field. This is the concrete "before" snapshot: 2/5 must keep this
test passing unmodified (or update it only alongside an explicit,
reviewed behavior change) to demonstrate route decisions remain
byte-for-byte equivalent under the migration.

## 12. Sign-off

Owner: **pending** — recorded via the review verdict/approval on issue #504
(this document + `tests/execution_identity.rs`), per the project's evidence
gate (missing evidence is a human-review outcome, not an autonomous
approval). The reviewer confirms, before 2/5 begins:

- [ ] Every field in §1 has an unambiguous owner and no two owners can write
      conflicting values for the same field.
- [ ] The auth/cost class taxonomy in §7 covers every `logical_backend`
      GAH currently dispatches to (`claude`, `codex`, `openhands`, `vibe`,
      `opencode`, `agy`/`agy-main`/`agy-second`) without a credential ever
      appearing in a canonical field.
- [ ] The equality/keying rule in §6 for `CandidateIdentity` (excludes
      `quota_pool`) is an explicit, accepted decision — not an oversight —
      for 2/5 to build on or revisit.
- [ ] The legacy compatibility table in §9 has no system GAH persists
      identity/usage to that is left undocumented.
- [ ] `cargo test execution_identity`, `cargo test --test
      usage_telemetry_regression`, and `cargo test` all pass on this
      branch.
