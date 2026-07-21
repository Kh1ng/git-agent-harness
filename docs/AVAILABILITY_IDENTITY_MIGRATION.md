# Availability and quota identity migration

Issue #507 adds execution-instance scope without assigning historical state to
an account that was never observed.

## Availability schema v2

`availability.json` keeps its append-only `records` array and adds one optional
field:

```json
{
  "version": 2,
  "records": [
    {
      "backend": "opencode",
      "backend_instance": "opencode-account-a",
      "model": "shared-model",
      "quota_pool": "shared-paid-pool"
    }
  ]
}
```

Version 1 loads as version 2 in memory. Its records keep
`backend_instance: null`; GAH never guesses an account from a path, HOME,
credential, backend, or quota pool. The next lock-protected state update writes
the version 2 envelope. Repeating that update produces the same migrated file.

Instance-aware reads consider the ordered union of:

- records for the exact `backend_instance`; and
- legacy records with no instance for the same logical backend.

Records for a different explicit instance never match. Pool records remain
pool-wide: a block or clear for a quota pool affects every identity configured
in that pool. Precedence remains pool, backend-wide, then model-specific.

Operators can clear one backend/model instance without affecting a sibling:

```text
gah availability clear --backend opencode \
  --backend-instance opencode-account-a --model shared-model
```

`--instance` is accepted as an alias. Instance and pool selectors are mutually
exclusive because they select different scopes.

## Quota observation compatibility

`quota_observations.jsonl` remains append-only. New rows may add
`backend_instance` and `quota_pool`; historical rows are not rewritten and
remain instance-unknown. Exact-instance readers may fall back to those legacy
rows, but a row belonging to another explicit instance is never selected.
Legacy backend aggregation only reads instance-unknown rows, preventing an
explicit account observation from being stripped of its identity and shown as
global data.

Explicit refresh example:

```text
gah quota refresh --backend codex --backend-instance codex-main \
  --quota-pool codex-subscription
```

Identity and pool labels are trimmed logical names. GAH rejects filesystem
paths, whitespace-bearing labels, control characters, and token-shaped values
before they enter either durable store.
