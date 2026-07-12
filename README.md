# git-agent-harness

`gah` is a CLI that runs coding agents against real repositories with guardrails around git worktrees, validation, pushing, draft MR/PR creation, PM ticket decomposition, session logging, and cleanup.

## Requirements

- `git`
- Rust toolchain (`cargo build --release`)
- One backend CLI:
  - `codex`
  - `claude`
  - `openhands`
- Provider tooling:
  - GitHub: `gh`
  - GitLab: `glab` for PM MR preflight, plus token env vars for push/MR creation

## Install

```bash
cargo build --release
mkdir -p ~/.config/gah
cp config/gah-config.example.toml ~/.config/gah/config.toml
```

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
weak_review_backend = "codex"
allow_review_fallback = true

[profiles.my-repo.routing]
pm_backend = "claude"
improve_backend = "codex"
```

### Subscription routing setup

For a profile that should use several subscription-backed workers, configure
explicit backend/model pairs in priority order. Higher `priority` wins; an
unavailable backend is skipped and the next eligible candidate is selected.
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

Routing is currently configured in TOML. Verify the selected config and its
prerequisites from the CLI before starting a loop:

```bash
gah profile show my-repo
gah doctor --profile my-repo --validate
gah status --profile my-repo --json
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

## First Dispatch

### GitHub issue intake allowlist

GitHub issue bodies are worker-prompt input. By default, GAH accepts issues
only from the owner in `profile.repo` (for example, `alice/repo` accepts
`alice`). To define the exact trusted authors for one profile, set the
allowlist under its publishing policy:

```toml
[profiles.my_profile.publishing]
github_issue_author_allowlist = ["alice", "teammate-login"]
```

Configuring the allowlist replaces the owner-only default, so include `alice`
when they should remain trusted; add or remove team members by editing this
list. An explicit empty list disables GitHub issue intake for that profile.
The same policy applies to loop discovery and an explicit
`gah dispatch --target #123`.

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
gah dispatch --profile my-repo --mode pm --backend claude --target "Break this work into atomic tickets"
```

## PM Mode

PM mode with a target now injects preflight context before the manager runs:

- `docs/MANAGER_MEMORY.md`
- open GitLab MRs
- recently merged GitLab MRs
- existing `docs/tickets/*.md`
- current branch, dirty state, recent commits

Missing `docs/MANAGER_MEMORY.md` is a hard failure for PM decomposition.

Implementation, fix, and experiment workers instead receive the bounded
`docs/PROJECT_BRIEF.md` and a task-specific live task pack. This deliberately
keeps mutable manager state and unrelated backlog out of worker prompts; the
written `context-built.json` artifact records every prompt section and its
estimated token size.

With a target, PM mode now asks the manager for structured JSON, validates it, dedupes it against existing tickets/open MRs/recent merged MRs, assigns ticket IDs, and then writes the ticket markdown files itself.

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

Use `gah sync --profile my-repo` to classify open GAH-created MRs/PRs without dispatching anything new.

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
