# Backend instance configuration migration

Issue #510 makes a concrete runner/account binding a first-class routing
instance. The shared registry belongs in `~/.config/gah/canonical.toml`
(`GAH_CANONICAL_CONFIG` overrides that path). A repository can add or replace
entries with the same syntax under its defaults or profile routing section.

Precedence is:

1. shared canonical `routing.backend_instances`;
2. repository `[defaults.routing.backend_instances.<name>]`;
3. `[profiles.<profile>.routing.backend_instances.<name>]`.

Maps merge by instance name; the later declaration replaces the complete
entry with that name. Candidate lists keep their existing replace-wholesale
semantics. Legacy candidates without `instance` continue to use the existing
profile paths, backend aliases, and AGY secondary-home compatibility behavior.

## Shared registry

Declare safe labels, never credentials. `executable` may be a CLI or a wrapper
that selects credentials outside GAH. `state_root` becomes that process's
`HOME` and must not be shared by independent accounts.

```toml
[routing.backend_instances.opencode-subscription]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/opt/gah/bin/opencode-subscription"
state_root = "/var/lib/gah/opencode-subscription"
account_label = "personal-subscription"
auth_source_label = "opencode-login"
quota_pool = "opencode-plan"
supported_models = ["openai/gpt-5"]

[routing.backend_instances.opencode-api]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/opt/gah/bin/opencode-api"
state_root = "/var/lib/gah/opencode-api"
account_label = "team-api"
auth_source_label = "env-openai-key"
quota_pool = "openai-api"
supported_models = ["openai/gpt-5"]
```

A project selects those bindings without repeating their runtime setup:

```toml
[[profiles.my-repo.routing.improve_candidates]]
backend = "opencode"
instance = "opencode-subscription"
model = "openai/gpt-5"
priority = 100
included_in_quota = true

[[profiles.my-repo.routing.improve_candidates]]
backend = "opencode"
instance = "opencode-api"
model = "openai/gpt-5"
priority = 10
marginal_cost_usd = 1.0
requires_approval = true
```

The two candidates remain separate routing, concurrency, availability, quota,
ledger, and telemetry destinations even though runner, logical backend, and
model are identical. The instance name is the durable destination key.

A local backend uses the same schema and no billing classification:

```toml
[routing.backend_instances.local-ollama]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/usr/local/bin/opencode"
state_root = "/var/lib/gah/local-opencode"
account_label = "local-ollama"
auth_source_label = "local-runtime"
supported_models = ["ollama/qwen3-coder"]

[[routing.improve_candidates]]
backend = "opencode"
instance = "local-ollama"
model = "ollama/qwen3-coder"
priority = 100
```

## Project override

To replace one global binding for one repository, redeclare that same key in
the repository config. The entire instance entry is replaced, so repeat every
field the project needs:

```toml
[profiles.my-repo.routing.backend_instances.opencode-subscription]
runner_kind = "opencode"
logical_backend = "opencode"
executable = "/workspace/my-repo/bin/opencode-wrapper"
state_root = "/workspace/my-repo/.gah/opencode-home"
account_label = "project-subscription"
auth_source_label = "opencode-login"
quota_pool = "opencode-plan"
supported_models = ["openai/gpt-5"]
```

`gah config show`, `gah status --json`, and `gah quota snapshot` expose only
safe labels and boolean runtime-binding indicators. Executable and state-root
paths are runtime-only and credentials are never accepted as identity labels.

## Validation and rollback

Run:

```text
gah doctor --profile my-repo --validate
gah config show --profile my-repo --json
gah status --profile my-repo --json
```

Approve an instance-scoped paid fallback with the exact destination printed by
the routing error:

```text
gah route-approval grant --profile my-repo ISSUE-42 --backend opencode \
  --instance opencode-api --model openai/gpt-5
```

Doctor rejects unknown instance references, missing/non-executable bindings,
unsafe labels, shared state roots, model/instance mismatches, and contradictory
cost declarations. To roll back, remove `instance` from candidates and remove
the registry entries; legacy backend paths and routing remain compatible. No
ledger, availability, quota, or telemetry rewrite is required.
