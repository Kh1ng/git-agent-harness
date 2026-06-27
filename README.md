# git-agent-harness

`gah` is a CLI control plane for running AI coding agents against git repositories. It creates isolated git worktrees, runs a backend agent, commits any changes, and opens a draft MR/PR — all from a single command.

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
export LLM_API_KEY=...

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

# GitLab-only fields:
# provider_api_base   = "https://gitlab.example.com/api/v4"
# provider_project_id = "42"
```

## Environment variables

Secrets always come from env vars, never the config file:

| Variable | Purpose |
|---|---|
| `GITLAB_PAT` | GitLab personal access token (api + write_repository scopes) |
| `GITHUB_TOKEN` | GitHub token (optional if `gh auth login` is done) |
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
| `--target` | | Task hint or branch name (mode-dependent) |
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
3. `profile.openhands_profile` in config.toml
4. `defaults.llm_model_local` / `defaults.llm_model_cloud` + `defaults.llm_base_url`

**Modes:**

| Mode | What it does |
|---|---|
| `improve` / `fix` | Create worktree → run agent → commit+push → open draft MR |
| `pm` | Generate PM report: git log, test count, CI status, README excerpt |
| `review` | Diff vs target branch → run `claude -p` review → write report |
| `experiment` | Not yet implemented |

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

## Adding a new project

1. Clone the repo locally.
2. Add a `[profiles.<name>]` block to your config.toml (set `provider`, `repo`, `local_path`, `default_target_branch`).
3. For GitLab: run `glab auth login --hostname <your-instance>`.
4. Validate: `gah profile show <name>`.
5. Preview: `gah dispatch --profile <name> --mode pm --dry-run`.
6. Run: `gah dispatch --profile <name> --mode improve`.

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
