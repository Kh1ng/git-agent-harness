# git-agent-harness

`gah` is a CLI that runs coding agents against real repositories with guardrails around git worktrees, validation, pushing, draft MR/PR creation, PM ticket decomposition, session logging, and cleanup.

## Requirements

- `git`
- Rust toolchain (`cargo` and `rustup`)
- One backend CLI:
  - `codex`
  - `claude`
  - `openhands`
- Provider tooling:
  - GitHub: `gh`
  - GitLab: `glab` for PM MR preflight, plus token env vars for push/MR creation

## Install

Install the CLI and control-plane server through the deterministic host
installer from a clean checkout of the default branch:

```bash
scripts/install.sh
mkdir -p ~/.config/gah
cp config/gah-config.example.toml ~/.config/gah/config.toml
```

The control-plane server binds `0.0.0.0:3773` by default. To bind a single
interface instead (recommended, since the server's mutation routes have no
authentication yet — see issue #532), set `GAH_SERVER_HOST` before first
install:

```bash
GAH_SERVER_HOST=127.0.0.1 scripts/install.sh
```

This writes `/etc/gah/server.env`, read by `packaging/systemd/gah-server.service`
via `EnvironmentFile=`. Edit that file's `HOST=` value at any time to change
the bind address without touching the installed unit; reinstalls and `gah
update --restart-server` never overwrite an existing `/etc/gah/server.env`.
See `docs/OPERATIONS.md` for details, including the startup warning emitted
whenever the server binds a non-loopback address.

For every deployed upgrade, use the installed CLI to update the checkout,
replace the executable selected by `PATH`, rebuild the server, and restart the
system service only after all build steps succeed:

```bash
gah update --repo /path/to/git-agent-harness --restart-server
```

`cargo build --release` is a development build only. It updates
`target/release/gah`; it does not replace the Cargo-installed `gah` executable
or rebuild/restart the control-plane server.

The default product installation covers the Rust CLI and Node control-plane
server. Web, desktop, mobile, and other clients are separate packages with
independent build/deployment workflows.

Provider-specific examples:

- `config/gah-config.github.example.toml`
- `config/gah-config.gitlab.example.toml`
- `config/gah-config.gitlab-self-hosted.example.toml`

## What GAH Creates

- Per-run session directories under each profile `artifact_root/sessions/`
- Worktrees under `defaults.worktree_base`
- A JSONL session ledger at:
  - `$GAH_LEDGER_PATH`, if set
  - otherwise `defaults.artifact_root/ledger.jsonl`, if configured
  - otherwise `~/.config/gah/ledger.jsonl`

## Config Basics

GAH loads config from:

1. `--config`
2. `GAH_CONFIG`
3. `~/.config/gah/config.toml`

Minimal shape:

```toml
[defaults]
artifact_root = "/home/you/.local/share/gah/artifacts"
worktree_base = "/home/you/.local/share/gah/worktrees"
llm_base_url = "http://localhost:4000"
llm_model_local = "your/local-model"
llm_model_cloud = "your/cloud-model"

[profiles.my-repo]
display_name = "My Repo"
repo_id = "my-repo"
provider = "github"
repo = "owner/repo"
local_path = "/path/to/local/clone"
artifact_root = "/home/you/.local/share/gah/artifacts/my-repo"
default_target_branch = "main"
validation_commands = []
# Review inactivity is a stall; continuous progress may run longer.
review_timeout_seconds = 300
# Optional independent wall-clock ceiling; omit to disable.
review_hard_timeout_seconds = 3600
```

GitLab adds:

```toml
provider_api_base = "https://gitlab.example.com/api/v4"
provider_project_id = "12345"
```

Secrets do not go in config.

Routing precedence:

1. explicit CLI backend/model override
2. profile routing config
3. global `defaults.routing`
4. built-in fallback

Example:

```toml
[defaults.routing]
default_backend = "openhands"
review_backend = "claude"
allow_review_fallback = true

[profiles.my-repo.routing]
pm_backend = "claude"
improve_backend = "codex"
# NEEDS_FIX is repaired this many times before human escalation.
max_fix_attempts_per_mr = 3
```

Unless `max_review_cycles_per_ticket` is explicitly set too, GAH permits one
additional review beyond that repair cap: initial review, up to the configured
repairs, then a review of the final repair. This prevents a review budget from
silently cutting short the repair budget. The cap bounds routine reviews; each
explicit `escalatory_reviewers` backend/model retains one bounded attempt after
the cap so exhausted weak/routine cycles cannot skip every strong second
opinion. Paid escalation still requires its independent approval and budget.

`weak_review_backend` is legacy compatibility configuration. Do not use it
for normal review routing: use the ordered `review_candidates` pool and
`escalatory_reviewers`. A weak reviewer approval requires human attention;
a weak reviewer `NEEDS_FIX` consumes the same post-review repair budget as
any other `NEEDS_FIX` verdict.

### Subscription routing setup

For a profile that should use several subscription-backed workers, configure
explicit backend/model pairs in priority order. Higher `priority` wins; an
unavailable backend is skipped and the next eligible candidate is selected.
Candidates with the same priority form a balanced pool: GAH selects the pair
with the fewest executions in the last seven days (configuration order breaks
ties). After a genuine capability failure, the same work item advances past
backend/model pairs it already tried instead of selecting them again.
Do not use `model = "default"` for a route you care about: a backend default
can resolve through a global alias such as `defaults.llm_model_cloud`.

```toml
[profiles.my-repo.routing]
# Scalar fields are the legacy/default route. Keep them explicit too.
default_backend = "vibe"
default_model = "devstral-small"
pm_backend = "vibe"
pm_model = "devstral-small"
improve_backend = "vibe"
improve_model = "devstral-small"
review_backend = "vibe"
review_model = "mistral-medium-3.5"
allow_implementation_fallback = true
allow_review_fallback = true
max_implementation_failures_per_ticket = 8

# Preferred inexpensive implementation tier.
[[profiles.my-repo.routing.improve_candidates]]
backend = "vibe"
model = "devstral-small"
priority = 100

[[profiles.my-repo.routing.improve_candidates]]
backend = "agy"
model = "Gemini 3.5 Flash (Medium)"
priority = 100

[[profiles.my-repo.routing.improve_candidates]]
backend = "agy-second"
model = "Gemini 3.5 Flash (Medium)"
priority = 100

# Retained, but used less often than the preferred subscription tier.
[[profiles.my-repo.routing.improve_candidates]]
backend = "codex"
model = "gpt-5.4-mini"
priority = 50

[[profiles.my-repo.routing.improve_candidates]]
backend = "claude"
model = "haiku"
priority = 25

[[profiles.my-repo.routing.review_candidates]]
backend = "vibe"
model = "mistral-medium-3.5"
priority = 100
```

Repeat the implementation candidates under `pm_candidates` when planning work
should use the same worker tier. Give each independent account its own backend
instance (`agy` and `agy-second`, for example), even when the provider and
model are identical; this keeps quota, availability, and usage records
separate.

For a shared, provider-neutral registry of CLI wrappers, account labels,
isolated state roots, and quota pools—with per-project overrides—see
[`docs/BACKEND_INSTANCE_CONFIG_MIGRATION.md`](docs/BACKEND_INSTANCE_CONFIG_MIGRATION.md).

Routing is currently configured in TOML. Verify the selected config and its
prerequisites from the CLI before starting a loop:

```bash
gah profile show my-repo
gah doctor --profile my-repo --validate
gah status --profile my-repo --json
```

Paid implementation routes can be kept as terminal fallbacks without granting
the unattended loop permission to spend money:

```toml
[[profiles.my-repo.routing.improve_candidates]]
backend = "opencode"
model = "openai/gpt-paid-fallback"
priority = 10
included_in_quota = false
requires_approval = true
```

When all eligible non-paid routes are exhausted, GAH stops that work item and
prints the exact approval command. Grant or revoke the exact backend/model pair
without editing credentials or rewriting ledger history:

```bash
gah route-approval grant --profile my-repo ISSUE-42 \
  --backend opencode --model openai/gpt-paid-fallback
gah route-approval revoke --profile my-repo ISSUE-42 \
  --backend opencode --model openai/gpt-paid-fallback
```

The dashboard settings editor and effective-route display are tracked in
[#149](https://github.com/Kh1ng/git-agent-harness/issues/149). Until that
lands, inspect the profile's TOML directly and keep every production route's
model explicit.

## Auth

- GitHub: set `GITHUB_TOKEN` or `GH_TOKEN`
- GitLab: set `GITLAB_PAT` or `GITLAB_PAT2`
- LLM proxy: set `LLM_API_KEY` if needed

GAH keeps push auth in askpass; it does not embed tokens into remotes or push URLs.

## Setup

### GitHub

```bash
gh auth login
export GITHUB_TOKEN=...
gah doctor --profile my-repo
```

### GitLab.com

```bash
glab auth login --hostname gitlab.com
export GITLAB_PAT=...
gah doctor --profile my-repo
```

### Self-Hosted GitLab

```bash
glab auth login --hostname gitlab.example.com
export GITLAB_PAT=...
```

Set:

```toml
provider_api_base = "https://gitlab.example.com/api/v4"
```

GAH derives pushes from that base, including self-hosted domains.

## Onboarding

`gah init` writes a starter config or appends a profile block.

```bash
gah init \
  --profile my-repo \
  --display-name "My Repo" \
  --provider gitlab \
  --repo group/project \
  --local-path /path/to/repo \
  --default-target-branch main \
  --provider-api-base https://gitlab.example.com/api/v4
```

Preview without writing:

```bash
gah init ... --print
```

## Doctor

Check config and profile readiness:

```bash
gah doctor --profile my-repo
gah doctor
```

Doctor checks:

- config loads
- repo path exists and is a git repo
- provider CLI exists
- expected provider token env vars are present
- push URL can be derived
- artifact/worktree paths are writable
- `docs/MANAGER_MEMORY.md` exists
- generated-artifact publication patterns are valid

## First Dispatch

### Trusted issue intake

Issue bodies are worker-prompt input. Configure trusted humans and provider
bots independently for each GitHub or GitLab profile:

```toml
[profiles.my_profile.publishing]
trusted_issue_human_authors = ["alice", "teammate-login"]
trusted_issue_bot_authors = ["project_5_bot_deadbeef"]
issue_intake_mode = "canonical_autonomous_only"
canonical_autonomous_label = "exec:autonomous"
```

`canonical_autonomous_only` is opt-in and makes recurring discovery require the
canonical label. Owner-decision, blocked, and planning labels still win when
labels conflict. Explicit dispatch of a trusted but held or unlabelled issue
requires the visible `--issue-intake-override` flag; it never bypasses author
trust.

For backward compatibility, a GitHub profile without the new human list still
uses `github_issue_author_allowlist`; if neither list is configured, only the
repository owner is trusted. That compatibility field never grants GitLab
trust. GitLab project access-token users are recognized from the project-scoped
`project_<project-id>_bot_*` username and must still be listed exactly in
`trusted_issue_bot_authors`. Explicit empty lists deny that author class.

### Generated-artifact publication guard

Before GAH creates or pushes a commit, it rejects newly tracked files matching
the profile's generated-artifact deny patterns. The default covers nested
`node_modules`, Vite/Vitest caches, coverage, language caches, build targets,
and TypeScript build-info files. Existing tracked files are not removed or
rewritten. Override the complete list per profile, or set an explicit empty
list to disable the guard:

```toml
[profiles.my_profile.publishing]
generated_artifact_deny_patterns = [
  "**/node_modules/**",
  "**/.vite/**",
  "**/coverage/**",
  "**/target/**",
  "**/*.tsbuildinfo",
]
```

The effective list is included in `gah status --json`, and `gah doctor` prints
the active policy. A match fails before commit/push with the exact path,
pattern, and policy source; GAH does not silently delete the worker's files.

Start with a dry run:

```bash
gah dispatch --profile my-repo --mode improve --dry-run
```

Then run for real:

```bash
gah dispatch --profile my-repo --mode improve --backend codex --target "Fix flaky tests"
```

PM report without a manager backend:

```bash
gah dispatch --profile my-repo --mode pm
```

PM ticket decomposition:

```bash
gah dispatch --profile my-repo --mode pm --backend claude --target "#123"
gah pm publish --profile my-repo --plan artifacts/sessions/<run>/pm-plan-v1.json --dry-run
gah pm publish --profile my-repo --plan artifacts/sessions/<run>/pm-plan-v1.json
```

## PM Mode

PM mode with a target now injects preflight context before the manager runs:

- open trusted GitHub or GitLab issues
- open native PRs/MRs, including non-GAH branches
- recently merged native PRs/MRs
- existing `docs/tickets/*.md`
- current branch, dirty state, recent commits
- optional bounded project guidance

Project guidance is optional. By default GAH uses the first existing file from
`docs/PM_GUIDANCE.md`, `docs/project-guidance.md`,
`docs/pm-guidance.md`, or `PM_GUIDANCE.md`. Override that ordered search per
profile (or in routing defaults) when a repository uses another convention:

```toml
[profiles.my-repo.routing]
pm_guidance_paths = ["docs/PROJECT_BRIEF.md", "AGENTS.md"]
```

If issue or PR/MR discovery fails or reaches a provider query cap, PM
decomposition stops instead of treating the missing duplicate context as an
empty backlog. The generated plan is validated and written as
`pm-plan-v1.json` in the dispatch session. Planning never creates provider
issues or local ticket files.

Implementation, fix, and experiment workers instead receive the bounded
`docs/PROJECT_BRIEF.md` and a task-specific live task pack. This deliberately
keeps mutable manager state and unrelated backlog out of worker prompts; the
written `context-built.json` artifact records every prompt section and its
estimated token size.

With a target, PM mode asks the manager for provider-neutral structured JSON,
validates field/count/byte/dependency/overlap bounds and dedupes it against
native issues, existing tickets, open PRs/MRs, and recently merged PRs/MRs.
The separate `gah pm publish` operation rechecks the source issue before every
provider write, uses a stable plan fingerprint to resume partial publication
without duplicates, and uses provider issue numbers as the only work identity.

The recurring controller performs the same two phases automatically only for
trusted issues carrying a configured decomposition label. It claims the source
issue before planning, resumes a previously-written plan after interruption,
and records exact child issue numbers before releasing the claim. Publication
does not close the source issue. Later controller snapshots read native child
state and record reconciliation only after every child is terminal.

```toml
[profiles.my-repo.publishing]
pm_decomposition_labels = ["planning", "plan"]
pm_max_children = 12
pm_max_depth = 1
pm_max_attempts = 2
pm_timeout_seconds = 900
```

`pm_max_children` is capped at 24, depth at 8, attempts at 10, and timeout at
two hours even if a larger value is configured. The timeout is one real
wall-clock process-group deadline shared by all planning backend attempts,
independent of the normal progress-aware
idle timeout. Generated owner-decision children never receive the canonical
autonomous label and therefore never enter normal implementation routing.

PM publication applies only labels explicitly mapped in the profile and only
when those labels already exist at the provider. Autonomous work also uses the
profile's existing `canonical_autonomous_label`:

```toml
[profiles.my-repo.publishing.pm_difficulty_labels]
easy = "difficulty:easy"
medium = "difficulty:medium"
hard = "difficulty:hard"

[profiles.my-repo.publishing.pm_risk_labels]
low = "risk:low"
medium = "risk:medium"
high = "risk:high"

[profiles.my-repo.publishing.pm_execution_labels]
human_required = "exec:owner-decision"
supervised = "exec:supervised"
```

When `improve` or `fix` targets a ticket markdown file, GAH also reads ticket metadata such as difficulty, risk, recommended backend/model, affected files, and verification commands before routing the worker.

## Retries

`improve`/`fix` retry failed validation up to `--retries` times (default 2).
Between attempts the worktree is hard-reset (`git reset --hard` + `git clean -fd`)
so each attempt starts from a pristine tree. The retry prompt is rebuilt from
the base task with only the *latest* failure output (retry blocks are not
accumulated — that confuses smaller models). The failed attempt's diff is
saved to `sessions/<ts>/attempt-N/attempt-diff.patch` before the wipe.

If the validation failure is byte-identical to the previous attempt, the run
aborts early: an unchanged error means the agent's edits had no effect on it,
which almost always indicates an environment or config problem (missing tool,
bad validation command) that no retry can fix.

Validation commands run through `sh -c`, so shell syntax (`cd x && y`, pipes,
env vars) works.

Before attempt 1, validation runs once on the pristine worktree (baseline).
A failing baseline is recorded to `sessions/<ts>/baseline-validation-failure.txt`
and injected into the task prompt, so the agent knows whether a failure is
pre-existing. If the final failure is identical to the baseline, the error
message says so — the agent's changes never affected it, which means the
validation command or environment is broken, not the code.

`--allow-draft-fail` pushes a `[DRAFT-FAIL]` MR even if validation never passes.

## Experiment Mode

```bash
gah dispatch --profile my-repo --mode experiment --target "research question"
```

Runs the backend with a research prompt, collects untracked artifacts
(`*.ipynb`, `*.html`, `*.png`, `*.csv`, `*.parquet`) into the session dir,
asks an LLM judge whether the task was answered, and opens a draft MR only
if code changed.

## Env Files

Profiles may set `env_file` (dev credentials, loaded by default) and
`env_file_prod` (loaded only with `--prod`). Keep prod credentials out of
dev runs; `--prod` also switches policy enforcement to `git-push-prod`.

## Manager Agent

`docs/gah-manager-skill.md` is the system prompt / skill file for a manager
agent that orchestrates GAH: decomposes work via PM mode, dispatches workers,
tracks state in the target repo's `docs/MANAGER_MEMORY.md`, and escalates
failed tickets to stronger models.

## Review Gate

Review mode now produces:

- `review-report.md`
- `review-verdict.json`

Verdicts are:

- `APPROVE_STRONG`
- `APPROVE_WEAK`
- `NEEDS_FIX`
- `REJECT`
- `HUMAN_REVIEW`

Weak or fallback review always requires human review. No auto-merge is performed.

When the provider can be reached, GAH also posts a concise MR/PR comment and best-effort labels such as:

- `gah-ready-for-human`
- `gah-needs-fix`
- `gah-human-review`
- `gah-review-weak`

## Ledger

Inspect recent runs:

```bash
tail -n 20 ~/.config/gah/ledger.jsonl
jq . ~/.config/gah/ledger.jsonl | less
```

Fields include mode, backend, branch, session dir, validation status, commit/push/MR status, diff stats, error summary, and nullable usage/cost placeholders.

Summarize the ledger:

```bash
gah ledger summary --since 7d
gah ledger summary --profile my-repo --since 24h
```

Summary includes backend/mode counts, requested vs effective backend, fallback counts, validation and push rates, MR counts, average duration, and usage/cost totals when known.

## Sync

Use `gah sync --profile my-repo` for an explicit current and historical
classification of GAH-created MRs/PRs without dispatching anything new.

Current classifications include:

- `CI_FAILED`
- `NEEDS_REVIEW`
- `NEEDS_FIX`
- `READY_FOR_HUMAN`
- `MERGED`
- `STALE`
- `UNKNOWN`

This pass only prints state and recommended next action. It does not auto-merge or auto-dispatch fix runs.

## Prune

Remove old GAH-owned sessions and worktrees:

```bash
gah prune --dry-run --older-than 14
gah prune --profile my-repo --older-than 30
```

Prune only touches:

- `artifact_root/sessions/*`
- worktrees under `defaults.worktree_base` with GAH-owned naming prefixes

## Operating GAH Unattended

For running and repairing GAH as an unattended service (systemd units, token
scopes, state-file repair commands, notification/manager-wake setup, failure
triage, and the auto-merge safety model), see the operator runbook:
[`docs/OPERATIONS.md`](docs/OPERATIONS.md).

## Command Summary

- `gah init`
- `gah doctor`
- `gah dispatch`
- `gah ledger summary`
- `gah ledger repair-tail [--dry-run]`
- `gah prune`
- `gah sync`
- `gah profile list`
- `gah profile show <name>`
- `gah candidates`
- `gah price-guard`
- `gah policy-check`

## TODO / Backlog

Contract tests for the first two exist in `tests/gah_cli.rs` as `#[ignore]`d
tests — implement until they pass, then remove the ignore.

- **`gah sync --json`**: machine-readable MR classification so a manager agent
  can consume state without parsing pretty-print. See
  `sync_json_outputs_machine_readable_classification`.
- **`gah ledger summary --json`**: same for run history/costs. See
  `ledger_summary_json_outputs_machine_readable_counts`.
- **Populate ledger usage/cost fields**: `usage.*` is always null, so the
  routing cost caps (`max_known_estimated_cost_per_week` etc.) can never
  trigger. Implementation plan (TDD):
  1. Grab 2-3 real `backend-output.log` files from
     `artifact_root/sessions/*/attempt-*/` (openhands runs with `--json`, so
     the log is JSON event lines) and commit trimmed excerpts containing the
     token/cost fields as `tests/fixtures/usage-logs/*.log`. Do NOT guess the
     field names — read them from real logs.
  2. Add `pub fn parse_usage_from_log(log_path: &Path, backend: &str) ->
     LedgerUsage` in `runner.rs`: scan lines for JSON objects, take the last
     one containing usage keys (openhands: accumulated cost/token metrics in
     its event stream; claude: run with `--output-format json` and read
     `total_cost_usd` / `usage` from the final result object). Unknown
     format → `LedgerUsage::default()`, never an error.
  3. Call it after each `run_backend` in `dispatch.rs` (improve, pm, review,
     experiment) and assign to `ledger.usage`; set `usage_source` to the
     backend name.
  4. Unit tests against the fixtures; assert `gah ledger summary` then shows
     nonzero cost totals.
- **Fix strong-run heuristic**: `ledger::usage_summary_for_backend` counts
  every improve/fix/review run as "strong" unless `confidence_impact == low`.
  Strongness should be determined by model/backend (e.g. a configured
  strong-model list), not by mode.
- **Failure taxonomy + attempts in ledger**: add to `LedgerEntry`:
  `attempts: u32`, `baseline_validation: Option<String>` ("passed"/"failed"),
  and `failure_class: Option<String>` with values `harness_error` (validation
  command could not run / config bug), `env_error` (baseline failing and
  failure identical to baseline), `agent_no_progress` (failure identical
  across attempts), `agent_failure` (real failing validation), `backend_error`
  (nonzero backend exit). Set these at each bail/success site in
  `dispatch::improve`. Without this, model-economics stats bill config bugs
  to the model.
- **Outcome backfill**: a dispatch's real outcome (merged / closed / rotting)
  is only known later. Extend `gah sync` to join provider MR state back onto
  ledger entries by branch name and append a
  `{"type":"outcome","branch":...,"state":"merged|closed|open","merged_at":...}`
  record to the ledger (append-only, no rewriting). TDD against the existing
  fake-`gh` pattern in `tests/gah_cli.rs`.
- **`gah ledger models --since 30d`**: the economics report. Per
  (effective_backend, effective_model): dispatches, avg attempts, validation
  pass rate, harness-vs-agent failure split, MRs opened, MRs merged (from
  outcome records), total cost, and **cost per merged MR** — the number that
  answers "deepseek retries more but is still cheaper than codex". Requires
  the cost-parsing and outcome-backfill tickets. `--json` output included.
- **`gah doctor --validate`**: run the profile's `validation_commands` in the
  live repo (read-only) and `sh -n`-check their syntax, so broken validation
  config is caught at setup time, not inside a paid dispatch loop. Doctor
  passing should mean "dispatch will not waste money on config errors".
- **Smart MR titles from ticket**: Parse `Suggested MR Title:` field from the ticket file and use it as the MR title instead of the generic `[GAH] improve: <repo>`. Fall back to generic if field not present.
