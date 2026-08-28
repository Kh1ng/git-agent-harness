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
installs/reloads the `gah-loop@.service` user-unit template. On a central
node it also reinstalls the system-level `gah-server.service` unit from the
tracked template (issue #894, so the installed unit can't drift from
`packaging/systemd/`), builds the web dashboard and deploys its `dist` to the
web root (issue #896), and optionally restarts `gah-server.service`. It does
not build or deploy desktop, TUI, mobile, or other client packages.

The web deploy root defaults to `/var/www/gah`, a conventional static-site
root. It is **not** something this repo ships or documents as a server
layout — set `GAH_WEB_DEPLOY_ROOT` to wherever the web server on this host
actually serves the dashboard from (the deploy prints the chosen root so a
mismatch is visible). Set it to an empty string to skip web deploy entirely
on hosts that serve the dashboard from elsewhere.

`--restart-server` refuses to run while any `gah loop --profile …` process is
active. The loop has its own systemd user cgroup and must be stopped cleanly
first, then rerun the update. The restart also requires passwordless `sudo`
permission for `systemctl`; configure that deliberately for unattended hosts.

Run `gah update` itself as the operator user, not via `sudo` — it installs the
loop's systemd *user* unit under that user's own `$HOME`/`$XDG_CONFIG_HOME`.
Only the root-owned sub-steps — the system-level `gah-server.service` unit
install (issue #894), the web-root deploy (issue #896), and the
`--restart-server` restart — need root, and `gah update` escalates those
internally via its own `sudo …` calls. Running the whole command under `sudo`
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

### Known issue: `gah update` web step can wedge on the central node

The workspace-root `npm run build:web` runs a `prebuild` hook (`npm run
test:server`, the full `apps/server` battery) before `vite build`. On the
central node that battery wedges inside
`apps/server/src/managerChatSessions.integration.test.ts`, so the update's
web-build step never completes: observed 39 min at 0% CPU with no runner-level
timeout, and a bounded reproduction where subtest 4 ("a session preview
auto-detects the dev-server port and proxies it") fails at exactly its 30 s
timeout. The same battery passes on macOS. The CLI install, `apps/server/dist`
build, and systemd unit reinstall all precede the web step, so those still land
correctly; only the dashboard goes stale. Tracked as issue #994.

Verified workaround (used when the issue → chat work shipped, #991–#993) —
skip the test gate, build and deploy the dashboard directly:

```bash
cd apps/web && npx vite build
sudo rm -rf /var/www/gah/* && sudo cp -r dist/* /var/www/gah/
sudo systemctl restart gah-server   # only if the server build also changed
```

### Deployed state (2026-08-28)

Both the central node and the macOS dev host run the `gah` CLI at `origin/main`
(`4d5ccad6`, the issue → chat fixes, #991–#993). Central `gah-server` is active
and serving the rebuilt `apps/web` `dist` from `/var/www/gah`; the issue → chat
routes (`GET/POST /api/manager-chat/issues…`) are live through the front door
(`https://hermesagent.tail82695.ts.net`). Central `gah update` rolled the CLI
and `apps/server`; the dashboard was deployed with the workaround above because
of the prebuild gate issue.

### systemd units

- **`gah-server`** — runs `apps/server/dist/bin.js`, the REST/WebSocket
  control-plane server backing the desktop/web dashboard (see "Server bind
  host" below for `HOST`/port configuration). Its mutation routes (profile
  writes, config changes, loop start/stop) have no app-level authentication
  yet — that's issue #532. Until it ships, network-layer access control
  (loopback bind, Tailscale, Cloudflare Access) is the only auth boundary;
  keep the bind host loopback or restricted to a private interface unless
  you've deployed one of those in front of it. The server logs its effective
  bind address at startup and emits a warning there when it is bound
  non-loopback.
- **`gah-loop@<profile>`** — the recurring bounded controller. Each iteration
  is one observe → classify → decide → execute-one-action → persist cycle.
  It is a systemd *user* unit, and is the sole parent of that profile's worker
  pool. `KillMode=control-group` ensures an operator stop or parent failure
  kills every concurrent backend child; do not wrap it in a shell supervisor
  or start a detached `gah loop` by hand.
  The dashboard owns boot policy through systemd itself. **Stop** runs
  `systemctl --user disable --now gah-loop@<profile>` so the loop stops and
  stays stopped after reboot. **Start** runs `enable --now`, so it starts now
  and at the next login. A direct `systemctl --user start` also starts a
  disabled unit for the current login without changing its next-boot policy.
  `enable` only auto-starts a unit at boot when the user's systemd manager
  lingers (`loginctl enable-linger <user>`); without linger the unit starts
  at the next interactive login instead.
- **`gah-watchdog`** (`.service` + `.timer`) — an **alert-only** health check
  for `gah-loop@<profile>.service` units (issue #726). Every profile
  configured in `gah`'s config gets checked (`--profile` scopes it to one).
  `gah watchdog-check` only ever runs `systemctl --user show <unit>
  --property=... --value` — a read-only query — and prints one line per
  profile whose loop is stopped or has failed; it never runs `systemctl
  start`/`restart`/`enable`, `gah loop`, or calls any HTTP endpoint. **Only
  the operator or the dashboard may start a loop**
  (`systemctl --user start gah-loop@<profile>`, or the dashboard's equivalent
  button) — nothing else in GAH's packaging is allowed to. This replaces an
  earlier host-local, untracked script that silently restarted a stopped loop
  by calling the dashboard's start endpoint, resuming concurrent work during
  a blocker repair; that incident is exactly what this contract prevents. The
  timer is installed by `gah update`/`scripts/install.sh` alongside the loop
  unit but is never automatically enabled — opt in once you've configured an
  alert command in `gah-watchdog.service`'s `ExecStart`:
  ```bash
  systemctl --user enable --now gah-watchdog.timer
  ```

Direct `gah loop --once` remains useful for a bounded operator smoke test. On
Linux, a recurring foreground loop arms a kernel parent-death signal and exits
through normal SIGTERM cleanup when its launcher disappears; it refuses to
start if already orphaned to PID 1. Graceful cleanup walks the backend's PPID
tree, including tools that used `setsid`. Unattended operation must still use
the systemd unit: its control-group boundary additionally covers SIGKILL, when
no in-process cleanup handler can run.

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

#### Server bind host (issue #643)

`apps/server` (the process this unit starts) defaults `HOST` to `0.0.0.0` and
`PORT` to `3773`. Two ways to override, in increasing order of durability:

- **Direct process override**: set `HOST` in the process environment (for
  example when running `npm start` by hand). An unusable value (not a literal
  IPv4/IPv6 address) fails startup immediately with a clear error instead of
  silently falling back to the default.
- **Persistent systemd override**: `packaging/systemd/gah-server.service`
  reads `/etc/gah/server.env` via `EnvironmentFile=-` (the file is optional;
  the unit still starts if it's absent). Set `HOST=127.0.0.1`, a specific
  interface address, or `0.0.0.0` there instead of editing the installed
  unit. `scripts/install.sh` creates this file on first install (seed it with
  `GAH_SERVER_HOST=127.0.0.1 scripts/install.sh`) and leaves an existing file
  untouched on every later run; `gah update --restart-server` never writes to
  it either, so an operator's override survives every reinstall/update.

The server logs its effective bind address on every startup. When that
address is not loopback, it also logs a prominent warning: the mutation
routes (profile writes, config changes, loop start/stop) have no
authentication yet (issue #532), so anything that can reach a non-loopback
bind can call them. This is not new exposure — `0.0.0.0` has always been the
default — the warning just makes it visible instead of silent. Prefer binding
loopback and fronting the dashboard with Tailscale/Cloudflare Access (or
whatever your network boundary is) until #532 ships.

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

`max_parallel_workers` is a ceiling, not a promise to launch that many
processes blindly. Before every launch and refill, the native loop samples
node-wide available memory, one-minute CPU load, and Linux memory/CPU pressure
stall information. It also reserves projected headroom for workers that have
started but have not reached peak usage yet. Implementation and repair work
reserve more headroom than review or merge work, so a review backlog can keep
using otherwise-stranded capacity without admitting another compiler-heavy
worker near the memory floor. Set the ceiling high enough for the node and let
the pressure gate reduce live concurrency; route capacity and work claims
remain independent limits.

`MemAvailable` includes memory already materialized by running workers, while
lease reservations account for workers that have not reached peak use. The
gate takes the smaller of live available memory and total memory minus active
reservations before charging the new worker. CPU admission similarly uses the
larger of live load and committed CPU rather than adding them. This prevents a
launch burst from repeatedly spending one idle sample without double-counting
workers already visible in the live metrics. Reservations are released as soon
as each worker completes. If live pressure or reservation integrity cannot be
verified, admission fails closed rather than silently falling back to the
configured ceiling.

The pressure-aware admission code runs inside the `gah` binary. Updating a
source checkout alone does not change an already-installed loop service.
After upgrading, rebuild/install and restart the affected user units:

```bash
gah update --repo /path/to/git-agent-harness
systemctl --user restart gah-loop@gah gah-loop@sportsball
journalctl --user -u gah-loop@gah -u gah-loop@sportsball -n 100 --no-pager
```

Set `max_open_managed_mrs` per profile to bound implementation intake. It
defaults to `max_parallel_workers`; at the limit GAH keeps reviewing, fixing,
and merging existing work but does not start another PR-producing dispatch.
Use `gah profile set <name> --max-open-managed-mrs <count>` to change it.

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

#### Memory gateway placement (issue #880)

The TDAI memory gateway (manager-chat's compaction db, `apps/server`'s
`memoryGatewayClient.ts`) has no default install step of its own — a fresh
`gah-server` install works without one (manager chat is a required
dependency at *runtime*, not install time), but nothing configures it for
you. `scripts/install.sh` (and its per-OS `install-linux.sh`/`install-macos.sh`
implementations) grow two opt-in modes, selected via `GAH_GATEWAY_MODE`;
leaving it unset skips this section entirely and behaves exactly as before.
`colocated` needs the systemd unit in `packaging/systemd/` and is Linux-only;
`remote` works on both. On a Rust `gah loop`/dispatch worker (any `--role
worker` install, either OS), the gateway env vars land in
`~/.config/gah/gah-loop.env` instead of `/etc/gah/server.env` — that's the
file `gah-loop@.service` actually reads on Linux, and on macOS (no systemd
at all) nothing auto-loads it, so `source` it into the shell before running
`gah` (the installer prints the exact command).

**Remote** — point this host at a gateway already running elsewhere (e.g. a
central node):

```bash
GAH_GATEWAY_MODE=remote \
GAH_GATEWAY_URL=http://gateway-host:8420 \
GAH_GATEWAY_API_KEY=<key> \
scripts/install.sh
```

Before writing anything, the script calls `POST /recall` against that URL
(not `GET /health` — `/health` needs no auth, so it can't catch a wrong
key; `/recall` is read-only and auth-required on every configured gateway).
A failure there aborts the whole install with a clear error instead of
silently completing with an unreachable gateway — `memoryGatewayClient.ts`
hard-blocks every manager-chat turn on a failed recall/capture, so an
unreachable gateway found only at first real use is a much worse failure
mode than one caught at install time. On success, `TDAI_GATEWAY_URL`/
`TDAI_GATEWAY_API_KEY` are written into `/etc/gah/server.env` (the same
file `GAH_SERVER_HOST` above uses, `EnvironmentFile=-`'d by
`gah-server.service`).

Two things make this easier to actually do:
- **Getting the key**: `GET /api/settings/gateway` (gated by the same
  `authMiddleware` as `/api/registry`/`/api/claims`, not the app's
  unauthenticated default) reports the central node's gateway URL and API
  key. The dashboard's Settings page has a "Compaction DB (Memory Gateway)"
  section with a reveal/copy button for it — no need to SSH in and cat an
  env file on the central node.
- **A machine with nothing installed yet**: `scripts/bootstrap.sh` installs
  missing prerequisites via their own official installers (rustup, nvm),
  clones this repo, and execs `scripts/install.sh` — same `GAH_GATEWAY_*`
  env vars, just usable from a one-liner on a brand-new machine:
  ```bash
  curl -fsSL https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh \
    | GAH_GATEWAY_MODE=remote GAH_GATEWAY_URL=http://gateway-host:8420 \
      GAH_GATEWAY_API_KEY=<key> bash
  ```
  (Env vars go on the `bash` side of the pipe, not the `curl` side.)

**Co-located** — run the gateway on this same host, scripted instead of
the hand-deployment this replaces:

```bash
GAH_GATEWAY_MODE=colocated \
GAH_GATEWAY_MEMORYCORE_PATH=/path/to/TencentDB-Agent-Memory/MemoryCore \
GAH_GATEWAY_LLM_API_KEY=<openai-compatible-key> \
scripts/install.sh
```

This seeds `tdai-gateway.local.yaml` from the checkout's tracked
`tdai-gateway.standalone.yaml` template if one doesn't already exist
(OpenAI-compatible LLM, embedding off / BM25-only recall — edit that file
directly for a different LLM/embedding backend, e.g. a local LiteLLM
proxy), generates a `TDAI_GATEWAY_API_KEY` if none was given, writes both
into `~/.config/gah/tdai-gateway.env` (`chmod 600`), installs
`packaging/systemd/tdai-memory-gateway.service` as a `--user` unit with
`WorkingDirectory`/`ExecStart` substituted for the checkout path and the
`node` found on `PATH`, and enables it. The script waits for `GET /health`
to come up before wiring `TDAI_GATEWAY_URL=http://127.0.0.1:8420` (plus the
generated API key) into `/etc/gah/server.env`, so a broken gateway config
fails the install rather than leaving `gah-server` pointed at nothing.

Bound to loopback by default; widen with `gah network-expose` (above) if
another node needs to reach it, matching the guidance in the checked-in
unit file.

Re-running `scripts/install.sh` with different `GAH_GATEWAY_*` values
updates just those keys in `/etc/gah/server.env` (unlike `GAH_SERVER_HOST`,
which is create-once-then-never-touched) — an operator's own `HOST`
override is untouched either way.

### Network exposure (issue #879)

`gah network-expose` is the one configuration surface for exposing a
gah-managed service beyond loopback, replacing hand-run, per-service `ufw`
sessions with no shared record of intent. Config lives under `[defaults]`:

```toml
[defaults]
# "loopback" (safe default -- nothing exposed) | "lan" | "lan_tailscale"
network_exposure = "lan_tailscale"
lan_cidrs = ["192.168.1.0/24", "192.168.5.0/24"]
# tailscale_cidr defaults to "100.64.0.0/10" (every tailnet's fixed CGNAT
# range) if unset -- only takes effect when network_exposure = "lan_tailscale".
```

Apply it per port:

```bash
gah network-expose --port 8420 --label "memory gateway"
# --level overrides the configured default for one call (the advanced/
# drill-down case -- a specific port needs a narrower or wider scope):
gah network-expose --port 9119 --label "debug endpoint" --level loopback
```

This only applies firewall rules (idempotently -- safe to re-run, never
removes an existing rule; narrowing exposure is a deliberate separate
action) and prints a recommended bind host (`0.0.0.0` for `lan`/
`lan_tailscale`, `127.0.0.1` for `loopback`). It does not rewrite a
service's own config file -- apply the recommended bind host in whatever
config that service actually reads (an `Environment=HOST=...` line in a
systemd unit, a gateway's own YAML, etc.).

Requires passwordless `sudo` for `ufw` (`sudo -n ufw status` must not
prompt) for unattended use; run interactively once if it isn't configured
yet -- the command inherits stdio, so a password prompt still works.

### Node registration and fleet-collision preflight (issue #881)

Self-service node registration (`POST /api/registry/nodes` on the central
node's `apps/server`) and fleet aggregation (`GET /api/registry/fleet`)
already exist. Two things build on top of them here: a repeatable way to
call registration instead of hand-building a curl call, and an advisory
safety check before `gah loop` starts.

**Registering a node**, run from the node being registered, pointed at the
central node's now-reachable URL (`gah network-expose` above covers
exposing whatever port that traffic needs):

```bash
npm run register-node --workspace=apps/server -- \
  --central-url https://central.example.com \
  --transport-mode authenticated_remote \
  --secret-ref env:NODE_TOKEN \
  --labels laptop,dev
```

Identity (`node_id`/`display_name`/`advertised_url`/`version`/
`schema_digest`) is read from this node's own `GET /health` rather than
re-derived -- one identity-generation path (`getCoordinatorIdentity()`),
not two that could drift. `--transport-mode` and `--secret-ref` are policy
choices the registering operator makes, matching `registerNode()`'s
validation (`loopback`/`authenticated_remote`/`trusted_lan`; non-loopback
endpoints require `authenticated_remote` over TLS). `COORDINATOR_TOKEN` in
the environment, if set, is sent as the central node's Bearer auth.
Confirm it worked: `curl <central-url>/api/registry/fleet` should list the
new node.

**Fleet-collision preflight**: `gah loop` refuses/warns before starting if
the central registry already shows another node with active claims/work
for the same profile -- catching the common case of accidentally starting
the same profile on two nodes. Opt-in via `[defaults]`:

```toml
[defaults]
registry_central_url = "https://central.example.com"
# "warn" (default -- print and proceed) | "refuse" (abort startup)
registry_preflight_mode = "refuse"
```

This is advisory, not a guarantee: the registry's node-staleness window is
30 minutes, and there's an inherent TOCTOU race between the check and
actually starting work -- it does not make it safe to run the same profile
from two nodes at once (that needs real claim arbitration, tracked
separately and not yet implemented). Leaving `registry_central_url` unset
skips the check entirely; a failed check (network, parse error) never
blocks startup either, since failing closed on a flaky registry would be
worse than the problem this exists to catch. See `crate::fleet_preflight`
for the implementation.

**Self-registration (issue #944)**: a node must be registered with the
central before it can claim work -- `authorizeClaimRequest` refuses leases
for unregistered nodes. `gah loop` now self-registers best-effort at
startup (advisory: a failure warns loudly but never blocks the loop), and
`gah node register` does it on demand. Both read the node's identity from
`coordinator-identity.json` (the same file `apps/server` reads/writes) and
POST it to the central's `/api/registry/nodes`:

```bash
gah node register \
  --central-url https://central.example.com \
  --transport-mode trusted_lan \
  --secret-ref env:COORDINATOR_TOKEN \
  --profiles gah,sportsball
```

`--transport-mode` defaults to `trusted_lan` (the self-hosted tailnet
default); the loop-start self-registration reads `GAH_REGISTRY_TRANSPORT_MODE`
and `GAH_REGISTRY_SECRET_REF` env vars (defaults `trusted_lan` /
`env:COORDINATOR_TOKEN`). A fresh worker can set `GAH_NODE_ADVERTISED_URL`
to create its stable identity on the first loop start. For a non-loopback
`trusted_lan` endpoint over **plain HTTP**
(e.g. `http://100.118.97.79` on a tailnet), the central must opt in with
`GAH_ALLOW_INSECURE_HTTP=1`; the worker needs the same opt-in for
plain-HTTP status polling. This flag is deliberately named for what it does —
it lifts the TLS requirement for **every** `authMiddleware`-protected route
(the registry, claims, and settings APIs), not just LAN registration — so
treat it as "this host accepts plain HTTP for authenticated API traffic".
Requests still require `COORDINATOR_TOKEN`.
Without the opt-in, access is rejected. The central also rejects any node that
advertises the central node's own endpoint, which would make its liveness
poller poll itself and recurse. Re-running registration updates the existing
node's validated endpoint, transport, secret reference, and profile declarations.

> **Updating an existing registration requires the coordinator token.**
> Creating a new node is how a worker self-registers, so it stays open to
> loopback/authenticated requests. But a re-registration that matches an
> existing `node_id` repoints where the central polls (`advertised_url`) and
> how it authenticates (`secret_ref`), so the route requires a valid
> `COORDINATOR_TOKEN` even for loopback-looking requests — otherwise any
> tailnet peer reaching the central through its reverse proxy (which appears
> loopback to the server) could hijack a node by its ID alone.

### Node liveness scheduler (issue #883)

Before this, nothing polled registered nodes periodically -- `pollNodeObservation`
only ran on-demand (a dashboard fetch, dispatch routing), so a node with
nothing currently dispatching to it could go dark and never get flagged.
`apps/server`'s `bin.ts` now starts `RegistryService.startLivenessScheduler()`
at boot: every 60 seconds it polls every registered node via the same
`getNodeObservations()` the dashboard and `/api/registry/fleet` already
use, and alerts once a node has been `stale`/`unreachable`/`auth_failed`/
`incompatible` for 3 consecutive checks (~3 minutes) -- once per outage,
not on every subsequent bad check, and again if it recovers and later goes
bad a second time.

Alerting reuses the same shell-hook shape as the Rust side's per-profile
`notify_command` (section 4 below) rather than inventing new plumbing: set
`GAH_NODE_LIVENESS_NOTIFY_COMMAND` in `apps/server`'s environment (e.g.
`/etc/gah/server.env`, `EnvironmentFile=-`'d by `gah-server.service`) and
a one-line message is piped to that command's stdin, shell-executed.
Unset skips alerting entirely; a failing or missing command is logged to
stderr and swallowed -- it never crashes the scheduler.

```bash
# /etc/gah/server.env
GAH_NODE_LIVENESS_NOTIFY_COMMAND=/home/you/bin/telegram-notify
```

This is the interim, poll-based liveness model. The eventual model
(worker nodes dial in and hold a persistent WebSocket, reusing the
already-built `client.hello`/`server.welcome` handshake and the
currently-inert `ping`/`server.ping` message as a real heartbeat) is a
bigger lift -- tracked separately, not implemented here.

### Central claim arbitration (issue #882)

Two local-only mechanisms have always guarded against dispatching the same
work_id twice: `src/work_claim.rs`'s per-machine JSON+PID lock (protects
two `gah loop` processes on the *same* host -- untouched by this, it's a
different problem) and `src/dispatch/claims.rs`'s claim-mode ledger entry
(protects against a second *node* picking up the same ticket -- this is
the one that can't actually work across machines, since each node's
`ledger.jsonl` is local to its own disk). This adds a real cross-node
version of the second one.

**Opt-in, reusing #881's config**: set `registry_central_url` (the same
field the fleet-collision preflight uses) and it becomes the sole
cross-node exclusivity gate -- the local claim-mode ledger check is
bypassed (see `check_duplicate_work`'s `central_claims_active` parameter)
and every dispatch instead acquires a lease from the central node before
starting any backend work. Unset: identical to before this existed.

```toml
[defaults]
registry_central_url = "https://central.example.com"
```

**Fail closed.** An unreachable central node or a lease already held by
another node aborts dispatch via `?` before any backend work starts. A
central-node outage stops dispatch fleet-wide rather than risk a silent
double-dispatch -- a deliberate tradeoff, not an oversight.

**A node must declare which profiles it runs at registration time**
(`--profiles` on `register-node`, issue #881) before it can claim work
under them -- the central node checks this on every acquire/renew, not
just at registration:

```bash
npm run register-node --workspace=apps/server -- \
  --central-url https://central.example.com \
  --transport-mode authenticated_remote \
  --secret-ref env:NODE_TOKEN \
  --profiles gah,sportsball
```

**Renewal, not a flat TTL.** A lease lasts 15 minutes; a still-running
dispatch renews it every 5 minutes (`lease/3`, giving two missed renewals
of margin) via `POST /api/claims/renew` over plain HTTPS -- deliberately
not a persistent connection. The target end-state architecture is nodes
dialing *in* to the central node for everything (never the reverse, so a
worker never needs to expose a network surface of its own), and a
worker-initiated HTTP call already satisfies that; a future upgrade to a
long-lived connection (WebSocket with an HTTP fallback) would sit behind
this same acquire/renew/release shape rather than requiring a redesign.
If renewal has been failing long enough that the lease would have lapsed
(3 consecutive failures), a loud warning is logged, but the in-progress
backend process is **not** killed -- exclusivity degrades to advisory past
that point rather than the dispatch being forcibly terminated over what
might be a transient network blip.

**Storage**: `apps/server/src/claimsService.ts` keeps leases in a plain
JSON file (`config/claims-config.json` by default,
`GAH_CLAIMS_CONFIG_PATH` to override) plus an in-memory `Map`, not a real
database -- correctness comes from acquire/renew/release being
synchronous functions with no `await` between the read-check and the
write, which Node's single-threaded event loop already makes atomic, the
same guarantee a `WHERE` clause buys you in SQL.

See `src/central_claims.rs` (Rust client, `gah`'s side) and
`apps/server/src/claimsService.ts` (server, holds the leases) for the
implementation.

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
- **agy / agy-main / agy-second** — `agy` and the `agy-main` wrapper share the
  default `HOME` and therefore one authenticated account/quota pool;
  `agy-second` is isolated by `agy_second_home` as a distinct account.
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

A dispatch that reaches a **terminal harness refusal** — e.g. `backend
descendant cleanup failed; refusing to retry` — records a durable
`human_required` gate with reason code `terminal_harness_failure`. The
controller stops re-dispatching that ticket (including after a reboot) until
an operator inspects it and explicitly releases the gate with
`gah ledger clear-attempts --profile <profile> '<WORK_ID>'`.

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
gah sync   --profile <profile> --json    # explicit current + historical PR/MR reconciliation
gah events --profile <profile> --since 7d # controller event stream
gah ledger work <WORK_ID>                # full history for one work item
```

`gah sync` classifications: `CI_FAILED`, `NEEDS_REVIEW`, `NEEDS_FIX`,
`READY_FOR_HUMAN`, `MERGED`, `STALE`, `UNKNOWN`. It only reports; it does not
auto-merge or auto-dispatch.

Recurring `gah status` and controller observations query at most 100 open pull
requests, never the repository's full history. Reaching that cap is an
incomplete observation and fails closed. Full-history provider queries are
reserved for explicit `gah sync`, ledger reconciliation, and pruning. This
boundary prevents the July 16, 2026 incident where polling up to 1,000
historical GitHub PRs every 30 seconds exhausted the user's shared 5,000-point
GraphQL allowance.

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

- If a file is split and an extracted file still exceeds the threshold, add a
  reviewed entry at that file's exact current line count. Never increase an
  existing ceiling to make growth pass.
- If a file is split and the original drops below threshold, remove its old
  entry.
- If a file is moved, remove the stale old path entry and add/update the new
  path entry.

Stale baseline entries for deleted/moved paths are reported explicitly by the
test without blocking the run, so they can be cleaned up in the extraction PR.
