# git-agent-harness

`gah` is a CLI control plane for running AI coding agents against git repositories. It creates isolated git worktrees, runs a backend agent, validates output, commits any changes, and opens a draft MR/PR — all from a single command.

## Install

```bash
cargo build --release
# then put target/release/gah on PATH, or symlink it
```

## Quick start

```bash
# 1. Copy and edit the config
mkdir -p ~/.config/gah
cp config/gah-config.example.toml ~/.config/gah/config.toml
# edit it: add your repos under [profiles.<name>]

# 2. Set secrets in your environment (or .env)
export GITLAB_PAT=...
export LLM_API_KEY=***

# 3. Run
gah profile list
gah dispatch --profile my-repo --mode improve --dry-run
gah dispatch --profile my-repo --mode improve
```

## Config file

Location (in order of precedence):

1. `$GAH_CONFIG` env var
2. `~/.config/gah/config.toml`

```toml
[defaults]
artifact_root   = "/path/to/artifacts"
worktree_base   = "/path/to/worktrees"
llm_base_url    = "http://your-litellm-proxy:4000"
llm_model_local = "litellm_proxy/local-model"
llm_model_cloud = "litellm_proxy/cloud-model"

[profiles.my-repo]
display_name          = "My Repo"
repo_id               = "my-repo"             # used in branch names and artifact paths
provider              = "github"              # "github" or "gitlab"
repo                  = "owner/repo-name"
local_path            = "/path/to/local/clone"
artifact_root         = "/path/to/artifacts/my-repo"
default_target_branch = "main"

# Optional extra CLI args passed to each backend:
openhands_args = []                                      # e.g. plugin/skill flags
codex_args     = ["-c", "model=gpt-4o"]                 # codex exec overrides
claude_args    = ["--allowedTools", "Edit,Write,Bash"]   # limit claude's tools

# KEY=VALUE env file injected into the backend's environment.
# Use this for API keys, DB URLs, and other secrets (never commit this file).
env_file = "/path/to/secrets.env"

# File name patterns used to count test files for PM reports.
# Default (when omitted): test_*.py, *_test.py, *.test.ts, *.spec.ts, etc.
test_file_patterns = ["test_*.py", "*_spec.rb", "*.test.ts"]

# Commands run in the worktree after each agent attempt.
# All must pass (exit 0) before commit/push is allowed.
# On failure, output is fed back to the agent and the attempt retried.
validation_commands = ["cargo test --quiet", "cargo clippy -- -D warnings"]

# GitLab-only fields:
# provider_api_base   = "https://gitlab.example.com/api/v4"
# provider_project_id = "42"
```

## Environment variables

Secrets always come from env vars, never the config file:

| Variable | Purpose |
|---|---|
| `GITLAB_PAT` / `GITLAB_PAT2` | GitLab personal access token (api + write_repository scopes) |
| `GITHUB_TOKEN` / `GH_TOKEN` | GitHub token (optional if `gh auth login` is done) |
| `LLM_API_KEY` | LLM proxy API key |
| `LLM_BASE_URL` | Override LLM base URL (takes precedence over config) |
| `LLM_MODEL` | Override model entirely (takes precedence over config and OH profile) |
| `GAH_CONFIG` | Override config file path |

## Subcommands

### `gah dispatch`

Run an agent job against a profile.

```bash
gah dispatch --profile <name> --mode <mode> [options]
```

| Flag | Default | Description |
|---|---|---|
| `--profile` | required | Profile name from config |
| `--mode` | required | `improve`, `fix`, `pm`, `review`, `experiment` |
| `--backend` | `auto` | `openhands`, `cloud-coder`, `codex`, `claude`, `auto` |
| `--oh-profile` | config default | OpenHands profile name (`~/.openhands/profiles/<name>.json`) |
| `--target` | | Task hint, path to `candidates.json`, or ticket description (mode-dependent) |
| `--retries` | `2` | How many times to retry after validation fails |
| `--allow-draft-fail` | | Push and open draft MR even if validation still fails after all retries |
| `--dry-run` | | Print plan without making any changes |
| `--config` | | Override config file path |

**Backends:**

| Backend | Binary | Notes |
|---|---|---|
| `openhands` | `openhands` | Headless agent; uses local LLM from config/OH profile |
| `cloud-coder` | `openhands` | Same binary; uses `defaults.llm_model_cloud` |
| `codex` | `codex` | OpenAI Codex CLI (`codex exec <task>`) |
| `claude` | `claude` | Claude CLI (`claude -p <task>`) |
| `auto` | first available | Tries `openhands`, then errors |

**OpenHands LLM resolution order** (most to least specific):

1. `LLM_*` env vars always win
2. `--oh-profile <name>` → reads `~/.openhands/profiles/<name>.json`
3. `defaults.llm_model_local` / `defaults.llm_model_cloud` + `defaults.llm_base_url`

**Modes:**

| Mode | What it does |
|---|---|
| `improve` / `fix` | Create worktree → run agent → validate (retry on failure) → commit+push → open draft MR |
| `pm` | Without `--target`: static repo report (git log, test count, CI status, README). With `--target <ticket>`: dispatches a PM agent to decompose the ticket into atomic sub-tasks saved to `docs/tickets/TICKET-NNN-<slug>.md` |
| `review` | Diff vs target branch → bundle patch → run `claude -p` review → write review-report.md |
| `experiment` | Provisions a worktree for research/exploratory tasks. Runs the agent, then collects untracked artifacts (*.ipynb, *.csv, *.png, *.html — bypasses `.gitignore`), runs an LLM judge to evaluate task completion, and commits output + opens draft MR with judge verdict. Ideal for web scraping, ML experiments, data exports, and prototyping. |

### `gah profile`

```bash
gah profile list              # list all profiles
gah profile show <name>       # show profile details; warns if openhands_profile file is missing
```

### `gah candidates`

Convert CI/gate artifact findings into prioritized backlog candidates.

```bash
gah candidates --gate-artifact <path> --out-root <dir> [--include-warnings]
```

### `gah price-guard`

Check whether a model is on the price watchlist.

```bash
gah price-guard --watchlist <path> --model <name>
```

### `gah policy-check`

Evaluate a repo policy config against a requested action.

```bash
gah policy-check --config <path> --action <action>
```

## Provider CLI requirements

- **GitHub**: `gh` CLI, authenticated via `gh auth login`. No token env var needed if authed.
- **GitLab**: `glab` CLI, authenticated for your instance via `glab auth login --hostname gitlab.example.com`. The `local_path` must be a clone whose remote points at that instance.

*Note:* `gah` also supports direct API access via PAT env vars (`GITLAB_PAT2`, `GH_TOKEN`) for push/create-MR operations without the provider CLI.

## Adding a new project

1. Clone the repo locally.
2. Add a `[profiles.<name>]` block to your config.toml (set `provider`, `repo`, `local_path`, `default_target_branch`).
3. For GitLab: run `glab auth login --hostname <your-instance>`.
4. (Optional) Point the profile's `env_file` at your secrets file so backends have API access.
5. Validate: `gah profile show <name>`.
6. Preview: `gah dispatch --profile <name> --mode pm --dry-run`.
7. Run: `gah dispatch --profile <name> --mode improve`.

## Retry & Validation Loop

When `validation_commands` is set, the harness runs them in the worktree after each agent attempt. If any command exits non-zero:

1. The full stdout+stderr is saved to `validation-failure.txt`
2. The failure output is appended to the agent's task prompt (up to 8,000 chars)
3. A fresh agent process is cold-started with the accumulated context
4. Up to `--retries + 1` attempts are made

If all retries are exhausted, the harness bails with a clear error. Pass `--allow-draft-fail` to push the draft MR anyway (useful for partial progress).

## Experiment Artifact Collection

In `experiment` mode, untracked files with the following extensions are copied to the session artifacts directory:

`.ipynb` `.html` `.png` `.jpg` `.jpeg` `.csv` `.parquet`

Collection intentionally **ignores** `.gitignore` — research repos commonly gitignore large data files. The files remain untracked in git and are not committed to the repo; they are preserved in the session artifacts for review.

After collection, an LLM judge (Claude if available, otherwise artifact-count heuristic) evaluates whether the agent produced a meaningful answer. The judge verdict is included in the draft MR description.

## OpenHands profiles

Pass `--oh-profile <name>` at runtime to specify which LLM profile OpenHands uses. The profile file lives at `~/.openhands/profiles/<name>.json`:

```json
{
  "model": "litellm_proxy/local-qwen3-coder",
  "api_key": "",
  "base_url": "http://192.168.5.248:4000",
  "num_retries": 5
}
```

`LLM_*` env vars always override the profile file. List available profiles:

```bash
ls ~/.openhands/profiles/
```
