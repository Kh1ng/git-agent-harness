# git-agent-harness

`gah` is a CLI that runs coding agents against real repositories with guardrails around git worktrees, validation, pushing, draft MR/PR creation, PM ticket decomposition, session logging, and cleanup.

## Requirements

- `git`
- Rust toolchain (`cargo build --release`)
- One backend CLI:
  - `codex`
  - `claude`
  - `openhands` for `openhands` or `cloud-coder`
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

## Ledger

Inspect recent runs:

```bash
tail -n 20 ~/.config/gah/ledger.jsonl
jq . ~/.config/gah/ledger.jsonl | less
```

Fields include mode, backend, branch, session dir, validation status, commit/push/MR status, diff stats, error summary, and nullable usage/cost placeholders.

## Prune

Remove old GAH-owned sessions and worktrees:

```bash
gah prune --dry-run --older-than 14
gah prune --profile my-repo --older-than 30
```

Prune only touches:

- `artifact_root/sessions/*`
- worktrees under `defaults.worktree_base` with GAH-owned naming prefixes

## Command Summary

- `gah init`
- `gah doctor`
- `gah dispatch`
- `gah prune`
- `gah profile list`
- `gah profile show <name>`
- `gah candidates`
- `gah price-guard`
- `gah policy-check`
