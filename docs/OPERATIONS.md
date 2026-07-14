# GAH Operator Runbook

Operating and repairing GAH when it runs unattended (recurring `gah loop`,
`gah server` systemd unit, auto-merge policy, manager wake). This is the
operator-facing counterpart to `docs/MANAGER_MEMORY.md` (which is agent-facing
state) and `README.md` (which covers CLI basics and first-run setup).

Standing rule that governs everything below: **never hand-edit GAH state files**
(`availability.json`, `work_claims.json`, holds, the ledger,
`validation_check.json`). Use a documented repair command where one exists.
Editing JSON by hand races the running loop and can corrupt durable trust
state; if there is no repair command, stop and escalate rather than guessing.
The repair commands and current limitations are in section 3.

Every command here was checked against `gah --help` on the current binary. When
in doubt, re-run `gah <command> --help`; that output is truth, this document is
a summary.

---

## 1. Deployment

### Deterministic CLI/control-plane update

Do not assume a `cargo build --release` updates the `gah` command on PATH. A
host can have a stale Cargo-installed binary at `$CARGO_HOME/bin/gah` while
`target/release/gah` is current. Use the built-in updater for the CLI and Node
control plane:

```bash
gah update --repo /path/to/git-agent-harness --restart-server
```

It refuses a dirty or non-default-branch checkout, pulls with `--ff-only`,
replaces the actual Cargo-installed CLI with `cargo install --path . --force`,
installs the lockfile-pinned Node dependencies, builds `apps/server`, and
installs/reloads the `gah-loop@.service` user-unit template, and optionally
restarts `gah-server.service`. It does not
build or deploy web, desktop, TUI, mobile, or other client packages.

`--restart-server` refuses to run while any `gah loop --profile …` process is
active. The loop has its own systemd user cgroup and must be stopped cleanly
first, then rerun the update. The restart also requires passwordless `sudo`
permission for `systemctl`; configure that deliberately for unattended hosts.

Run `gah update` itself as the operator user, not via `sudo` — it installs the
loop's systemd *user* unit under that user's own `$HOME`/`$XDG_CONFIG_HOME`.
Only the two `--restart-server` sub-steps that touch the system-level
`gah-server.service` need root, and `gah update` escalates those internally
via its own `sudo systemctl …` calls. Running the whole command under `sudo`
would resolve `HOME` to root's and silently install/reload the loop unit for
root's systemd instance instead of yours.

For a fresh CLI/control-plane host installation:

```bash
scripts/install.sh
```

### Upgrade procedure

```bash
gah update --repo /path/to/git-agent-harness --restart-server
```

The updater never starts or restarts a recurring `gah loop`; with
`--restart-server` it also refuses to restart the service while one is active.

### systemd units

- **`gah-server`** — runs `gah server`, the WebSocket server backing the
  desktop/web dashboard. Defaults: `--host 127.0.0.1 --port 3773`. Keep it bound
  to loopback (or a Tailscale interface); network-layer access control
  (Tailscale / Cloudflare Access) is the auth model, there is no app-level
  login. Verify: `gah server --help`.
- **`gah-loop@<profile>`** — the recurring bounded controller. Each iteration
  is one observe → classify → decide → execute-one-action → persist cycle.
  It is a systemd *user* unit, and is the sole parent of that profile's worker
  pool. `KillMode=control-group` ensures an operator stop or parent failure
  kills every concurrent backend child; do not wrap it in a shell supervisor
  or start a detached `gah loop` by hand.

The checked-in server template is
`packaging/systemd/gah-server.service`. Before installing it, edit its `User`,
`WorkingDirectory`, `GAH_CONFIG_PATH`, Node path, and `PATH` values for the
host; its explicit toolchain `PATH` is required for dashboard-dispatched work.
Then install it as a system service:

```bash
sudo install -m 0644 packaging/systemd/gah-server.service /etc/systemd/system/gah-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now gah-server
```

`gah-server` (and hence the dashboard's Start/Stop buttons) drives `systemctl
--user` for the loop, which requires that user's systemd *user* manager to be
running even without an active login session. If `gah-server` runs as a
system service under `User=…` (the documented setup above), enable linger for
that user once, or every `systemctl --user` call fails with an opaque "Failed
to connect to bus":

```bash
sudo loginctl enable-linger <user>
```

Install the loop template once for the user that runs GAH, then the dashboard
Start/Stop buttons manage `gah-loop@<profile>` rather than creating a detached
process:

```bash
gah update --repo /path/to/git-agent-harness
systemctl --user start gah-loop@gah
```

The template reads the profile's configured `max_parallel_workers`; do not
add another supervisor or a second worker count at the service layer. Inspect
the entire process tree with `systemd-cgls --user` when validating a run.

Unlike the old dashboard-spawned loop, this unit does not inherit
`gah-server`'s process environment, so provider tokens (`GITHUB_TOKEN`/
`GH_TOKEN`, `GITLAB_PAT`) and LLM proxy config (`LLM_API_KEY`, `LLM_BASE_URL`,
`LLM_MODEL`, section 2) are not automatically present unless the profile sets
its own `env_file` in `gah`'s config. For any profile that doesn't, create
`~/.config/gah/gah-loop.env` (picked up automatically, `chmod 600` it) with
those values, or edit the unit's `Environment=`/`PATH` lines directly via
`systemctl --user edit gah-loop@<profile>` for a host-specific toolchain path.

Inspect and control units with the usual systemd verbs:

```bash
sudo systemctl status gah-server
sudo journalctl -u gah-server -n 100 --no-pager
systemctl --user status gah-loop@gah
journalctl --user -u gah-loop@gah -f
```

If the loop needs to be paused for a human (e.g. while triaging), stopping the
unit is the blunt instrument; `gah hold set` (section 3) is the surgical one
that pauses auto-merge for a single work item without stopping all work.

---

## 2. Required credentials & scopes

GAH never embeds tokens into git remotes or push URLs; push auth goes through
askpass. Secrets do not go in `config.toml`.

### GitHub

- Env: `GITHUB_TOKEN` or `GH_TOKEN`, and/or `gh auth login`.
- Scopes:
  - `repo` — required for normal PR create / push / merge.
  - `workflow` — **required to push any commit that touches
    `.github/workflows/*.yml`.** The operating token has historically lacked
    this, so workflow-file changes fail to push. Grant with:
    `gh auth refresh -h github.com -s workflow`.
  - `project` + `read:project` — **required to read/write GitHub project
    boards.** The operating token has historically lacked these; the token
    cannot even list projects without them. Grant with:
    `gh auth refresh -s project,read:project`.

Verify current scopes:

```bash
gh auth status
```

### GitLab

- Env: `GITLAB_PAT` (or `GITLAB_PAT2` for a second account), and/or
  `glab auth login --hostname <host>`.
- PAT scope: `api` (covers push, MR create/merge, and MR preflight via `glab`).
- Self-hosted: set `provider_api_base` in the profile
  (`https://gitlab.example.com/api/v4`); GAH derives pushes from that base.

### LLM proxy

- Env: `LLM_API_KEY` (only if the proxy requires it), `LLM_BASE_URL` /
  `LLM_MODEL` override the config defaults when set.

### Backend auth locations

Each backend authenticates through its own CLI, not through GAH:

- **codex** — ChatGPT-subscription auth via the `codex` CLI; verify with
  `codex doctor` (websocket connect + auth). Account-level quota is subscription,
  not API-metered.
- **claude** — `claude` CLI login; configured executable path allowed via
  profile `claude_path`.
- **agy / agy-main / agy-second** — separate AGY instances isolated by distinct
  `HOME`/state roots (and `agy_second_home`), each a distinct authenticated
  account / quota pool.
- **vibe**, **opencode**, **openhands** — their own respective CLI auth.

Validate that a profile's declared backends and tokens are actually present
before trusting an unattended run:

```bash
gah doctor --profile <profile> --validate
```

`doctor --validate` checks: config loads, repo path is a git repo, provider CLI
exists, expected token env vars are present, push URL derivable, artifact/worktree
paths writable, backend executables present, and validation commands resolve.

---

## 3. State files & repair commands

Most GAH durable control state lives under `$XDG_STATE_HOME/gah/` (fallback
`~/.local/state/gah/`). The append-only ledger, reconciliation log, event
stream, and manager-wake logs instead follow `GAH_*_PATH` overrides or
`defaults.artifact_root` (falling back to `~/.config/gah/`). **Do not edit any
of these by hand** — use the command listed.

### Availability — `$XDG_STATE_HOME/gah/availability.json`

Durable backend/model/quota-pool availability (quota exhaustion, auth failure,
manual disable). A stale entry keeps GAH skipping a backend that is actually
healthy again.

```bash
gah availability                    # human-readable current state
gah availability --json             # machine-readable

# Clear a stale block once the backend is confirmed healthy (issue #179):
gah availability clear --backend codex                     # whole backend
gah availability clear --backend codex --model gpt-5.4-mini # one model
gah availability clear --backend claude --quota-pool claude-main # a pool
```

`availability clear` appends a `status: available, source: manual` record
through the same lock-protected read-modify-write as every other write, so it is
safe against concurrent parallel workers.

### Work claims — `$XDG_STATE_HOME/gah/work_claims.json`

Active-ownership records used by the duplicate-work guard. A leaked/stale claim
blocks a work_id from being re-dispatched. Claims are normally released when a
controller process finishes. There is **no operator claims CLI yet** (tracked
in issue #234), and `gah ledger clear-attempts` does *not* clear a work claim.
If a work ID remains claimed after confirming no controller/dispatch process is
running, preserve the state file and escalate it as a harness defect; do not
hand-edit the file.

### Review holds

Manager-session review hold: tells GAH's own auto-merge loop to leave a
work_id's PR alone while a human or supervising agent reviews it out of band.
GAH's own loop never sets a hold; only a manager session does. A hold
self-expires after `REVIEW_HOLD_STALE_AFTER_HOURS`, or clear it explicitly:

```bash
gah hold set --profile <profile> <WORK_ID> --reason "human reviewing PR #123"
gah hold clear --profile <profile> <WORK_ID>
```

A leaked hold silently prevents auto-merge of an otherwise-ready PR — check for
one when a green, approved PR is not merging.

### Ledger

Append-only run history (dispatch/attempt/retry/review/outcome). Path
resolution: `$GAH_LEDGER_PATH`, else `defaults.artifact_root/ledger.jsonl`, else
`~/.config/gah/ledger.jsonl`.

```bash
gah ledger summary --since 7d                       # backend/mode/validation/cost rollup
gah ledger summary --profile <profile> --since 24h
gah ledger work <WORK_ID>                           # full chronological history for one item
gah ledger reconcile --profile <profile>            # backfill later MR merged/closed outcomes

# Mark all prior attempts for a work_id stale so it becomes dispatchable again
# (issue #95 — appends a tombstone, does NOT rewrite history):
gah ledger clear-attempts --profile <profile> <WORK_ID>
gah ledger clear-attempts --profile <profile> <WORK_ID> --dry-run
```

### Validation check — `$XDG_STATE_HOME/gah/validation_check.json`

Records the self-verification of a profile's `validation_commands` against a
fresh worktree (the validation gate). If a genuine `VALIDATION GATE FAILED`
error is understood and accepted, a run can be forced past it with
`--skip-validation-gate` on `gah dispatch` / `gah loop` — only after
acknowledging the failure, never as routine practice.

### Stale worktrees / sessions

Old GAH-owned worktrees and session dirs accumulate (a real incident hit 59GB).
Prune touches only `artifact_root/sessions/*` and worktrees under
`defaults.worktree_base` with GAH-owned naming prefixes:

```bash
gah prune --dry-run --older-than 14
gah prune --profile <profile> --older-than 30
```

### Concurrent Rust workers and disk capacity

GAH gives every dispatch session its own writable `CARGO_TARGET_DIR` under
`<profile.artifact_root>/build-cache/cargo-targets/`. All attempts in one
session reuse that directory, but concurrent worktrees never share it. Cargo's
registry/source cache remains shared normally; only compiled outputs are
isolated. This is required for correctness: Cargo's internal locks serialize
individual writes, but a shared target directory can still make one worktree
execute a same-package test binary produced from another worktree's source.

The session owner holds an advisory lock for the target's lifetime and removes
the directory at dispatch completion. Automatic pruning removes any unlocked
target left by SIGKILL, a host crash, or an older binary, so isolation does not
reintroduce the stale multi-gigabyte artifact leak.

Before creating a worktree, GAH also requires at least 10 GiB free on both the
worktree filesystem and the temporary filesystem. It fails before spending an
agent attempt when that floor is not met; reclaim terminal worktrees with
`gah prune` and inspect temporary files before retrying.

### Torn final ledger record

If an abrupt stop or full filesystem leaves `ledger.jsonl` with an incomplete
final line, GAH fails closed rather than treating the missing data as zero.
Repair only that specific physical failure with the guarded command below:

```bash
gah ledger repair-tail --dry-run
gah ledger repair-tail
```

The command only removes an invalid record that is both final and missing its
newline terminator. It saves those rejected bytes as a sibling
`ledger.jsonl.corrupt-tail-*` file before truncating. Newline-terminated or
mid-file corruption is never altered automatically and requires investigation.

---

## 4. Notification & manager-wake setup

GAH can notify an operator (and optionally wake a manager agent) on high-signal
events, without any external wrapper.

### `notify_command` (per profile)

Set `notify_command` on a profile; GAH pipes a single one-line message to that
command's stdin (shell-executed, like `validation_commands`) on:

- `HumanRequired` decided (reason + reference)
- MR/PR created (url, work_id, backend/model)
- review verdict recorded
- MR/PR auto-merged
- dispatch failed terminally (failure_class + work_id)
- backend killed by the idle watchdog (stalled → rerouting)

Routine events (observation, wait, no-op) emit nothing, to avoid spam. A failing
or missing `notify_command` is logged to stderr and swallowed — it never fails
the loop/dispatch. Example (Telegram via a helper script):

```toml
[profiles.my-repo]
notify_command = "/home/you/bin/telegram-notify"
```

### Manager wake (opt-in autonomy)

A Telegram ping still needs a human to act. To have GAH additionally spawn a
manager agent CLI headlessly on the same events, set two things:

- `defaults.current_manager` — global: which agent CLI is on call. One of
  `claude`, `codex`, `hermes`. Unset/unknown ⇒ no wake even if a profile opts in.
- `profiles.<name>.manager_wake_autonomy` — per profile:
  - `off` (default) — no wake; `notify_command` behavior unchanged.
  - `review_only` — woken agent reviews and comments, must not merge or write.
  - `full` — woken agent may act on its own judgment (review + merge if CI green
    and review passed, fix/escalate failures) under standing authorization.
    Must be opted in per profile; never the default.

```toml
[defaults]
current_manager = "claude"

[profiles.my-repo]
manager_wake_autonomy = "review_only"
```

Wakes are fire-and-forget but **always logged**: stdout/stderr of the spawned
agent go to a timestamped file under the wake log dir
(`GAH_MANAGER_WAKE_LOG_DIR`, else `artifact_root/manager-wake-logs`). Inspect
after the fact to see exactly what an unsupervised agent did — a wake must never
be unobservable. `MrMerged` never wakes (nothing left to act on).

---

## 5. Failure triage

GAH tags each failed attempt with a `failure_class` (visible via
`gah ledger work <id>` / `gah ledger summary` / `gah events`). What each means
and what to do:

| failure_class        | Meaning                                                        | Operator action |
|----------------------|---------------------------------------------------------------|-----------------|
| `harness_error`      | GAH/config bug: a validation command couldn't run, bad config | Stops work. Fix config / validation command; `gah doctor --validate`. Not the model's fault — do not escalate. |
| `environment_error`  | Baseline already red; failure identical to baseline           | Stops work. Fix the environment (missing tool, broken dep). Do not escalate the model. |
| `backend_error`      | Backend runtime failure (nonzero exit, empty output, quota/auth) | Reroute, not escalate. Check `gah availability`; if a quota/auth block is stale, `gah availability clear`. Never treat empty output as success. |
| `agent_no_progress`  | Failure byte-identical across attempts                        | The agent's edits aren't affecting the error — usually env/config, not the model. Investigate before re-dispatching. |
| `agent_failure`      | Real, changing validation failures                            | Genuine agent-capability miss. This is the only class where capability escalation to a stronger backend is appropriate (`--escalate`, or the loop's Escalate action). |
| `validation_failure` | Validation never passed after all retries                     | Inspect the session diff/logs; consider `--escalate` or a manual fix. |
| `human_blocked`      | Explicitly requires a human                                   | Human gate. Automation stops here by design. |
| `unknown`            | Unclassified                                                  | Stops by default. Inspect session logs before overriding. |

Escalation rule: **only `agent_failure` (genuine agent-performance failure)
justifies escalating model strength.** Never escalate for harness, environment,
auth, or quota failures — reroute or fix the underlying cause instead.

Primary triage commands:

```bash
gah status --profile <profile> --json    # single machine-readable snapshot of all state
gah sync   --profile <profile> --json    # classify open GAH-created PRs/MRs
gah events --profile <profile> --since 7d # controller event stream
gah ledger work <WORK_ID>                # full history for one work item
```

`gah sync` classifications: `CI_FAILED`, `NEEDS_REVIEW`, `NEEDS_FIX`,
`READY_FOR_HUMAN`, `MERGED`, `STALE`, `UNKNOWN`. It only reports; it does not
auto-merge or auto-dispatch.

Two safety invariants to keep in mind while triaging: a failed *observation* is
not a healthy empty state, and a closed-unmerged PR/MR is terminal, not active
work.

---

## 6. Safety model summary

### What can auto-merge

Autonomous merge happens only when the profile's `merge_policy` allows it and
**all** policy conditions pass at once:

- implementation completed
- validation passed
- no blocking review findings
- `human_required == false`
- no unresolved controller ambiguity
- no duplicate-work conflict
- review policy requirements satisfied (required reviewer capabilities available)
- PR/MR not superseded
- no active review hold on the work_id (`gah hold`)

`merge_policy` values (profile or `defaults`):

- `auto` (default) — GAH merges when all conditions above pass.
- `stop_for_human` — GAH never auto-merges; every ready PR waits for a human.
- `gitlab_mwps` — GitLab only: after strong approval GAH sets "merge when
  pipeline succeeds" and lets GitLab enforce the CI gate; other providers fall
  back to `auto`.

Reviewer tier (`strong` / `standard` / `weak`) is assigned by GAH config, never
self-declared by the reviewer, and is separate from verdict confidence. A weak
or fallback review always requires a human; no auto-merge on a weak review.

### What always stops for a human

- `human_required == true` (any `HumanRequired` controller decision)
- weak / fallback review verdict, or `HUMAN_REVIEW`
- malformed, missing, or unparseable review output (never merge on it)
- empty backend output (never treated as success)
- ambiguous critical state, or a duplicate-work conflict
- a missing required reviewer capability (hard preflight failure, no silent
  downgrade)
- `merge_policy = stop_for_human`

When unattended trust is in doubt, the conservative operator move is
`gah hold set` on the specific work_id (or `merge_policy = stop_for_human` on the
profile) rather than editing state or stopping all work.

## 7. Rust source-size ratchet guard

GAH enforces a hard ceiling for large Rust files in the `source_structure`
integration test. The baseline lives in
`config/rust-source-size-baseline.toml` and sets:

- `threshold`: files with `<= 1500` lines are unrestricted.
- `files`: tracked `.rs` files over threshold and their current ceilings.

The guard scans tracked Rust source and test files and fails only when:

- A baseline-listed file grows beyond its recorded ceiling.
- A tracked file exceeds the threshold but is missing from the baseline.

During extraction, remove or lower legacy entries:

- If a file is split and both halves remain over threshold, add new baseline
  entries and raise ceilings as needed.
- If a file is split and the original drops below threshold, remove its old
  entry.
- If a file is moved, remove the stale old path entry and add/update the new
  path entry.

Stale baseline entries for deleted/moved paths are reported explicitly by the
test without blocking the run, so they can be cleaned up in the extraction PR.
