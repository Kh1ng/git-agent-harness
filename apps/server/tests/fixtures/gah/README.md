# Fake `gah` fixture (issue #635)

`gah` in this directory is a fake CLI binary (a Node ESM script with a
`#!/usr/bin/env node` shebang, `chmod +x`'d) that replays recorded `--json`
output instead of running the real Rust `gah`. `apps/server`'s tests point
`GAH_BINARY` at this file so `/api/*` endpoint tests exercise the real
Express handlers and the real `gahCli.ts` spawn/parse/error logic, without
depending on a built `gah` binary, a configured profile, or network access.

## Provenance

The files under `responses/` are **real captured output** from the actual
Rust `gah` binary built at commit `a19292b2a85b4b1db0ef90fd748da41bb21594f5`
of this repo, run against a synthetic profile (`repo =
"Kh1ng/git-agent-harness"`, i.e. this repo itself) with an isolated, empty
state directory (`XDG_STATE_HOME`/`GAH_AVAILABILITY_PATH`/`GAH_LEDGER_PATH`/
`GAH_QUOTA_STORE_PATH` all pointed at a scratch temp dir) so the snapshot is
deterministic and free of any real operator's local state. `local_path` and
`artifact_root` were then hand-edited from the capturing machine's real
absolute paths to generic `/home/operator/...` placeholders; no other field
was modified.

| Response file | Command |
|---|---|
| `responses/status.json` | `gah status --profile fixture --json` |
| `responses/quota.json` | `gah quota snapshot --profile fixture --since 7d --json` |
| `responses/report.json` | `gah report --json --since 7d --group-by backend` |
| `responses/config-show.json` | `gah config show --json` |
| `responses/profile-list.json` | `gah profile list --json` |

## Recapturing

Only recapture if a response shape genuinely changed (a new field, a
renamed one) -- not routinely. From a clean checkout with a debug build
(`cargo build`):

```sh
tmp=$(mktemp -d)
cat > "$tmp/config.toml" <<'EOF'
[defaults]
worktree_base = "/home/operator/gah-worktrees"

[profiles.fixture]
display_name = "Fixture"
repo_id = "fixture-repo"
provider = "github"
repo = "Kh1ng/git-agent-harness"
local_path = "/home/operator/git-agent-harness"
artifact_root = "/home/operator/gah-artifacts"
default_target_branch = "main"
EOF
export GAH_CONFIG="$tmp/config.toml"
export XDG_STATE_HOME="$tmp/state"
export GAH_AVAILABILITY_PATH="$tmp/state/availability.jsonl"
export GAH_LEDGER_PATH="$tmp/state/ledger.jsonl"
export GAH_QUOTA_STORE_PATH="$tmp/state/quota_observations.jsonl"

gah status --profile fixture --json
gah quota snapshot --profile fixture --since 7d --json
gah report --json --since 7d --group-by backend
gah config show --json
gah profile list --json
```

Check the output for anything identifying (local paths, hostnames) before
committing it -- `report.json`'s `ledger_path` in particular echoes back
whatever `GAH_LEDGER_PATH`/scratch dir you captured from, and needs the same
`/home/operator/...`-style placeholder treatment `local_path`/`artifact_root`
already got.

## Forcing a failure

Set `GAH_FIXTURE_FAIL=<name>` (one of `status`/`quota`/`report`/`config-show`/
`profile-list`) before spawning the server process to make that one
subcommand exit non-zero instead of replaying its recorded response --
this is how the 5xx/stderr-propagation tests work. Optional
`GAH_FIXTURE_FAIL_CODE` (default `1`) and `GAH_FIXTURE_FAIL_MESSAGE` (default
a generic message) customize the failure.

## Overriding the profile list

Set `GAH_FIXTURE_PROFILE_LIST=<path.json>` to serve an arbitrary profile
array for `profile list --json` instead of the recorded response. The
WP2 chat-session tests use this to point `repo_id`/`local_path`/
`worktree_base` at a real temp git repository so worktree creation runs
against actual git. `repo_id` and `worktree_base` were also added to the
recorded response (post-capture edit: those fields postdate the capture
commit) because ProfileSummary now carries them.
