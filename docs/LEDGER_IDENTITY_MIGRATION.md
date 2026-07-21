# Ledger execution-identity migration

Ledger schema 9 adds an optional canonical `identity` object to each
`attempt_routing` record. It contains only secret-safe route facts:
runner kind, requested/effective backend and model, backend instance, account
label, auth-source label, and quota pool. The resolved executable is explicitly
excluded from serialization.

The parallel `attempts` list remains wire-compatible. Consumers join an
attempt to its canonical route identity by `attempt_number`; historical rows
without an identity keep that value unknown. They are never reconstructed from
backend strings, filesystem paths, or credential conventions.

New usage projections are additive nullable fields:

- `auth_source_label`
- `quota_pool`
- `provider_attribution_source`

SQLite remains a derived mirror of JSONL. On open, GAH idempotently adds
nullable columns for repository, runner, instance, account, auth source,
model provider, provider attribution source, auth class, quota pool, and
actual model. Existing rows stay `NULL`; rebuilding the mirror from JSONL is
safe and remains the recovery path.

Telemetry schema 9 publishes the same nullable dimensions and adds runner,
auth-class, and quota-pool aggregation. Historical absence is reported as
`unknown` only as a display bucket; it is not written back as a canonical
identity or substituted into the ledger.

Rollback requires no data rewrite. Older binaries ignore additive JSON fields.
If an older SQLite reader cannot tolerate the expanded mirror, delete only the
derived `.db` file and let that version continue reading authoritative JSONL.
