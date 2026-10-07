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

### Node memory admission

`gah loop` reserves 4096 MiB for each implementation, fix, retry, or escalation
worker and keeps a free-memory floor of `max(2048 MiB, total memory / 6)`.
The critical-memory and memory PSI checks always apply. Operators can set
these global defaults in Settings → Machine capacity or in TOML:

```toml
[defaults.node_capacity]
worker_memory_mib = 4096
memory_floor_mib = 0
worker_cpu_cores = 2
cpu_ceiling_percent = 90
```

`worker_cpu_cores` (1 to 64) is the CPU reserved for each implementation, fix,
retry, or escalation worker. `cpu_ceiling_percent` (10 to 400) is how much of
the node's logical CPUs running workers may reserve in total; above 100
overcommits. The first worker is always admitted. If `worker_cpu_cores` is
larger than the ceiling allows, the node runs one such worker at a time.

`memory_floor_mib = 0` (or an omitted value) keeps the adaptive floor. An
explicit floor must be at least 512 MiB; the worker reservation must also be
at least 512 MiB. Lowering either value raises the risk of running out of
memory. The loop reloads these settings each iteration and announces them
when they change; `gah status` reports the memory values. Review and PM
reservations are fixed fractions and are not configurable.

Validation is layered so a bad value can never lock you out of the config:

- Saving (`gah config set`, Settings page) rejects values below 512 MiB, and
  rejects values this node can never admit -- a worker reservation plus the
  memory floor larger than total node memory defers every implementation
  worker forever. The node check is skipped when total memory cannot be
  read (e.g. the config is written for another machine).
- Loading never fails on invalid values: they are ignored with a warning and
  the defaults apply until the config is fixed, so every repair path
  (`gah config set`, `gah status`, Settings) keeps working.
- The loop logs a warning at startup when the settings can never be admitted
  on this node, instead of leaving the fact buried in deferral logs.

### Release channel and in-app updates (issue #1416)

A green merge to `main` publishes a prerelease **edge** channel build
(`.github/workflows/release-edge.yml`): the `gah` and `gah-mcp-server`
binaries (Linux x86_64, macOS universal), the server bundle
(`gah-server-bundle.tar.gz`: the prebuilt `apps/server` and `apps/web` dist
outputs plus
the OpenCode agent configs), and `edge-manifest.json` — the versioned
manifest with a SHA-256 per artifact. When the `TAURI_SIGNING_PRIVATE_KEY`
secret is configured, the same release carries a signed Tauri updater feed
(`latest.json`) for the desktop app.

Install from the channel instead of rebuilding from source — the checkout
stays the deployment root, only the artifact source changes (no git pull,
cargo, or npm build):

```bash
gah update --from-release            # edge channel, manifest auto-discovered from origin
gah update --from-release --release-manifest /path/or/https://.../edge-manifest.json
```

Every surface shows "Update available" against the same feed: the web
dashboard's top banner and Settings page (which offer *Update and restart*),
the desktop app's Settings row (signed updater, restarts into the new
bundle), and the Fleet page — where each worker reports its version, nodes
behind the coordinator are flagged, *Update node* / *Update all nodes*
push the release to workers, and per-node auto-update opts a node in to
updating whenever central sees it behind. A worker that is mid-dispatch
finishes its run before the update restarts anything. Workers older than
the coordinator's minimum supported version
(`packages/contracts/src/coordinator-protocol.json`) are flagged
`unsupported` instead of failing silently. Source rebuilds remain the
explicit developer mode (`gah update` without `--from-release`, or the
Settings page's *Rebuild from source*).

### Deterministic CLI/control-plane update

Do not assume a `cargo build --release` updates the `gah` command on PATH. A
host can have a stale Cargo-installed binary at `$CARGO_HOME/bin/gah` while
`target/release/gah` is current. Use the built-in updater for the CLI and Node
control plane:

```bash
gah update --pull --repo /path/to/git-agent-harness --restart-server
```

`--pull` fetches and fast-forwards before installation. The command prints an
installation plan and asks for confirmation; pass `--yes` for unattended
updates. Omit `--pull` when reinstalling the current checkout, such as after
changing the node role.
Without `--pull`, it builds the current branch and working tree, including
uncommitted changes.

For unattended first installs, run `GAH_INSTALL_CONFIRMED=1 scripts/install.sh`
with the desired role and configuration environment variables. This accepts
the installer confirmation before it writes configuration or installs services.
For unattended updates, use `gah update --pull --yes --repo /path/to/git-agent-harness`.
Updates refresh existing GAH OpenCode agent files and quota-refresh units even
when `--agent` is omitted; use `--agent` to install additional integrations.

With `--pull`, it refuses a dirty or non-default-branch checkout and pulls
with `--ff-only`. It replaces the actual Cargo-installed CLI with
`cargo install --path . --bin gah --force --locked`,
installs the lockfile-pinned Node dependencies, builds `apps/server`, and
installs/reloads the `gah-loop@.service` user-unit template. On a central
node it also reinstalls the system-level `gah-server.service` unit from the
tracked template (issue #894, so the installed unit can't drift from
`packaging/systemd/`), builds the web dashboard, and optionally restarts
`gah-server.service`. It does not build or deploy desktop, TUI, mobile, or
other client packages.

**Who serves the dashboard.** `gah-server` serves the built web app itself
(`apps/web/dist` in the checkout), so a fresh install needs no separate web
server and no root to deploy the dashboard (issue #1327). `GAH_WEB_ROOT` in
`/etc/gah/server.env` overrides the directory; set it to an empty value when
another web server serves the dashboard and `gah-server` should serve only the
API.

**Hosts with their own web server.** `gah update` also copies the build into a
web root for Caddy or similar (issue #896), using `sudo`:

- `GAH_WEB_DEPLOY_ROOT` unset: copy to `/var/www/gah` if that directory already
  exists, as on a host set up before #1327. Otherwise nothing is copied.
- `GAH_WEB_DEPLOY_ROOT=/some/root`: copy there. The update prints the root.
- `GAH_WEB_DEPLOY_ROOT=` (empty): never copy.

An existing Caddy install therefore keeps working unchanged. To move it to
`gah-server`, remove the dashboard site from Caddy, delete `/var/www/gah`, and
restart `gah-server.service`.

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
gah update --pull --repo /path/to/git-agent-harness --restart-server
```

The updater never starts or restarts a recurring `gah loop`; with
`--restart-server` it also refuses to restart the service while one is active.

### Fixed: `gah update` web step used to wedge on the central node (issue #1010)

The workspace-root `npm run build:web` script carries a `prebuild:web`
lifecycle hook (`npm run test:server`, the full `apps/server` battery) that
npm runs before `vite build`. On the central node that battery once wedged
inside `apps/server/src/managerChatSessions.integration.test.ts`, so the
update's web-build step never completed: observed 39 min at 0% CPU with no
runner-level timeout, in a bounded reproduction where subtest 4 ("a session
preview auto-detects the dev-server port and proxies it") failed at exactly
its 30 s timeout. The same battery passes on macOS. Tracked as issue #994.

`gah update`'s `deploy_web_ui` step (`src/update.rs`) now runs `npm run
--workspace=apps/web build` directly instead of the root `build:web` script,
so the production updater never triggers `prebuild:web` and can't wedge on
that suite. CI still runs `npm run build:web` (see `.github/workflows/CI.yml`
and `frontend.yml`), so the integration battery is still exercised as a merge
gate -- it's just no longer on the production deploy path.

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
  host" below for `HOST`/port configuration). Remote REST and WebSocket access
  requires the coordinator bearer token or a paired-device credential. Remote
  transport also requires TLS unless the operator explicitly sets
  `GAH_ALLOW_INSECURE_HTTP=1`. The server logs its effective bind address and
  this transport requirement at startup when it is bound non-loopback.
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
  alert command in a `systemctl --user edit gah-watchdog.service` drop-in.
  The packaged check writes to the journal and requires no alert transport:
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
`packaging/systemd/gah-server.service`. It contains placeholders, not host
values. `gah update --role central` renders it for the account that runs the
update: its user, checkout, config path, the `node` found on `PATH`, and a
toolchain `PATH` that dispatched work needs. Then it installs the result as a
system service. The user units (`gah-loop@`, `gah-prune`,
`gah-quota-refresh`) are rendered the same way. Do not copy a template
verbatim. Then enable the service:

```bash
gah update --pull --role central
sudo systemctl enable --now gah-server
```

Run `gah update` again after you move the checkout or change the Node install.
Keep local customization in `systemctl edit` drop-ins, because every update
replaces the base unit.

#### Server bind host (issue #643)

`apps/server` (the process this unit starts) defaults `HOST` to `0.0.0.0` and
`PORT` to `3773` when run directly. A fresh Linux central install writes a
safer persistent host: its tailnet IPv4 when available, otherwise
`127.0.0.1`. Two ways to override, in increasing order of durability:

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
address is not loopback, it also logs a prominent warning. Remote API access
requires `COORDINATOR_TOKEN` or a paired-device credential. Plain HTTP is
rejected unless `GAH_ALLOW_INSECURE_HTTP=1` explicitly permits it; that flag
does not disable authentication. Prefer loopback plus an HTTPS proxy.

#### Tailscale names and HTTPS (issue #943)

Enable MagicDNS and HTTPS on the tailnet. Then configure the central Linux
node and each CLI-capable client:

```bash
sudo tailscale set --hostname=hermesagent
sudo tailscale set --accept-dns=true
```

On macOS or iOS, also confirm **Use Tailscale DNS settings** in the Tailscale
app. Test the system resolver with `ping hermesagent`, a browser, or
`tailscale dns query hermesagent`; `host` and `nslookup` can bypass the macOS
system resolver.

The preferred HTTPS setup keeps GAH on loopback and uses Tailscale Serve:

```bash
GAH_SERVER_HOST=127.0.0.1 scripts/install.sh
sudo tailscale serve --bg --https=443 http://127.0.0.1:3773
tailscale serve status
```

Use `https://hermesagent.<tailnet-name>.ts.net` for
`registry_central_url`, pairing, and the second-node setup command. HTTPS
certificates do not cover the bare `hermesagent` name. Enabling Tailscale
HTTPS publishes the machine and tailnet DNS names in Certificate Transparency
logs, although tailnet access rules still restrict the service.

For the direct HTTP MVP, keep the installer's detected `HOST=100.x.y.z`, add
`GAH_ALLOW_INSECURE_HTTP=1` to `/etc/gah/server.env`, and restart
`gah-server`. Use `http://hermesagent:3773` or the tailnet IP. This mode still
requires the coordinator bearer token or a paired-device credential.

An existing Caddy deployment can instead serve the full `*.ts.net` name with
a certificate from `tailscaled`. A bare-name `tls internal` setup requires
installing Caddy's root CA on every client, including explicit full trust on
iOS, so it is not the default. Do not use on-demand issuance for one fixed
name. See [the source-backed comparison](TAILSCALE_HTTPS_RESEARCH_2026-09-12.md).

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
gah update --pull --repo /path/to/git-agent-harness
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
gah update --pull --repo /path/to/git-agent-harness
systemctl --user restart gah-loop@gah gah-loop@sportsball
journalctl --user -u gah-loop@gah -u gah-loop@sportsball -n 100 --no-pager
```

### Scaling workers past the baseline

`max_parallel_workers` and `max_concurrent_per_model` are the baseline. A
profile's `[profiles.<name>.worker_scaling]` section lets the loop grow past
both, and the dashboard exposes it under Settings, Factory, Worker scaling.

Automatic scaling (`enabled = true`) gives a capped model `extra_per_model`
more concurrent runs (default 1) while every fresh quota window of its
subscription, the five-hour one included, has at least
`min_remaining_percent` left (default 50). A model with no fresh quota
reading is never scaled, and the highest-priority candidate is scaled first.
The total stops at `max_workers`, which defaults to twice the baseline.

A manual boost adds workers outright, for one model or for every capped
model, until an optional expiry:

```bash
gah profile set gah --worker-scaling on --worker-scaling-max-workers 6
gah profile set gah --boost-workers 2 --boost-model codex/gpt-5 --boost-hours 3
gah profile set gah --clear worker_boost
```

A boost is explicit, so `max_workers` does not limit it. Neither source
bypasses the pressure gate above: memory and CPU still decide whether an
extra worker starts. The loop applies changes on its next iteration and logs
the worker count when it changes; `gah status --json` reports the result and
the reason for each grant or refusal as `worker_limits`.

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
`colocated` uses systemd on Linux and launchd on macOS. `remote` works on
both. Worker settings go in `~/.config/gah/gah-loop.env`. The Linux and macOS
worker services read this file.

**Remote** — point this host at a gateway already running elsewhere (e.g. a
central node):

```bash
GAH_GATEWAY_MODE=remote \
GAH_GATEWAY_URL=http://gateway-host:8420 \
GAH_GATEWAY_API_KEY=<key> \
scripts/install.sh
```

`GAH_GATEWAY_URL` must use the **central/gateway node's tailnet address (or
MagicDNS name)**, not a LAN IP. A LAN IP only works while the worker stays on
that LAN; roaming makes recall/capture unreachable and hard-blocks manager
chat and dispatch under issue #878's policy. It must also never be derived
from `tailscale ip -4` on the worker: that is the worker's own address, not
the gateway's. To discover a device's own tailnet address (the central node's,
for pasting into `registry_central_url`, pairing, or the setup command), run
`gah tailscale-ip` (#943): it prefers `tailscale ip -4` and falls back to
scanning local interfaces for an address inside `[defaults].tailscale_cidr`
(default `100.64.0.0/10`), failing closed with an actionable error when the
device is not on a tailnet. `--json` emits `{"tailscale_ip": "...", "cidr":
"..."}` for scripting. When the URL is unset, both installers reuse the host from
`GAH_CONFIG` or `~/.config/gah/config.toml`'s `registry_central_url` and the
default gateway port `8420`. A truly new machine has no way to identify which
tailnet peer is the gateway, so use the Settings **Reveal setup command**
(which supplies the central's address explicitly) or set `GAH_GATEWAY_URL`.

Before writing anything, the script calls `POST /recall` against that URL
(not `GET /health` — `/health` needs no auth, so it can't catch a wrong
key; `/recall` is read-only and auth-required on every configured gateway).
A failure there aborts the whole install with a clear error instead of
silently completing with an unreachable gateway — `memoryGatewayClient.ts`
hard-blocks every manager-chat turn on a failed recall/capture, so an
unreachable gateway found only at first real use is a much worse failure
mode than one caught at install time. The installer then writes the gateway
values to the server environment file. Linux uses `/etc/gah/server.env`.
macOS uses `~/.config/gah/server.env`.

Two things make this easier to actually do:
- **Getting the setup command**: `GET /api/settings/gateway` reports only
  the central node's gateway location and configured status. The Settings
  page's "Add a Node" section calls the authenticated
  `POST /api/settings/gateway/bootstrap-command` only after an operator
  clicks **Reveal setup command**, then offers the returned command for
  copying. Credential bytes are absent from ordinary Settings JSON and the
  initial page DOM.
- **A machine with nothing installed yet**: `scripts/bootstrap.sh` asks
  before installing Rust, clones this repo, builds `gah`, and runs
  `gah setup`, which checks and offers every other prerequisite before it
  runs `scripts/install.sh`. The same `GAH_GATEWAY_*` env vars preselect the
  memory choice, so the one-liner still works on a brand-new machine (see
  [Getting started](GETTING_STARTED.md)):
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
scripts/install.sh
```

This seeds `tdai-gateway.local.yaml` from the checkout's tracked
`tdai-gateway.standalone.yaml` template if one doesn't already exist
(embedding off, keyword recall only). No model key is required; see
[Model provider for the colocated gateway](#model-provider-for-the-colocated-gateway-issue-1319)
to select Ollama or an OpenAI-compatible API. The installer keeps existing
model credentials and gateway authentication, generates a
`TDAI_GATEWAY_API_KEY` only if none exists, stores credentials
in `~/.config/gah/tdai-gateway.env` (`chmod 600`), installs
the platform service, and enables it. Linux installs the tracked systemd
unit. macOS generates a launchd agent from the same settings. The installer
waits for `GET /health`. A broken gateway stops the install with an error.

Bound to loopback by default; widen with `gah network-expose` (above) if
another node needs to reach it, matching the guidance in the checked-in
unit file.

Re-running `scripts/install.sh` with different `GAH_GATEWAY_*` values updates
only the gateway keys. The installer does not change an existing `HOST`
value.

### Model provider for the colocated gateway (issue #1319)

Shared memory needs no model provider and no API key. With
`GAH_GATEWAY_PROVIDER` unset, the installer leaves an existing
`tdai-gateway.local.yaml` alone, or seeds the template: keyword (BM25)
recall, no embedding, no generation calls.

The gateway has three separate settings, which setup used to treat as one:

| Setting | Variable | Needed |
| --- | --- | --- |
| Gateway access authentication | `GAH_GATEWAY_API_KEY`, stored as `TDAI_GATEWAY_API_KEY` | Always; generated when not given |
| Embedding model (vector recall) | `GAH_GATEWAY_EMBEDDING_MODEL`, `GAH_GATEWAY_EMBEDDING_DIMENSIONS`, `GAH_GATEWAY_EMBEDDING_API_KEY` | Only with a provider; the key only for `openai` |
| Generation model (memory extraction) | `GAH_GATEWAY_LLM_MODEL`, `GAH_GATEWAY_LLM_API_KEY` | Optional with every provider |

`GAH_GATEWAY_PROVIDER` selects the backend for both models, and
`GAH_GATEWAY_ENDPOINT` overrides its address:

- `ollama`: Ollama, or any local OpenAI-compatible server that needs no key
  (LM Studio, a LiteLLM proxy, vLLM). No credential is asked for or stored.
- `openai`: any authenticated OpenAI-compatible API. The embedding key is
  required; the generation key is optional, and without it the gateway makes
  no generation calls.

`gah setup`, Settings > Memory > Colocated Gateway Provider, and Add a Node
(for a central or standalone node) all collect these same values and pass
them to the installer. `gah setup --yes` selects a provider only when
`GAH_GATEWAY_PROVIDER` is set.

**Gateway contract.** Read from
[`Kh1ng/TencentDB-Agent-Memory`](https://github.com/Kh1ng/TencentDB-Agent-Memory)
`main` at `a0f993ba1eeda16243a8267ba9d1929074b806f9`. That revision has no
Ollama-specific code. `MemoryCore/src/config.ts` treats every
`memory.embedding.provider` other than `none`, `local`, and `qclaw` as a
remote OpenAI-compatible service, and disables it, with only a log line,
unless `apiKey`, `baseUrl`, `model`, and `dimensions` are all set.
`src/core/store/embedding.ts` posts to `${baseUrl}/embeddings`.
`src/gateway/config.ts` uses the generation model only with a non-empty
`llm.apiKey`; a non-empty `TDAI_LLM_API_KEY` in the gateway environment
overrides the file, and an empty one is ignored. A string of the form
`"${VAR}"` expands from the gateway environment, which the service loads
from `~/.config/gah/tdai-gateway.env`. `GET /health` needs no
authentication and reports `stores.embeddingService`.

Because that gateway calls a backend only with a non-empty key, the `ollama`
provider writes the literal `ollama` as the key. It is not a credential:
Ollama ignores it, and it is the value Ollama's own OpenAI-compatibility
guide uses. `scripts/gateway-provider.mjs`, which both installers run,
writes:

| Field | `ollama` | `openai` |
| --- | --- | --- |
| `llm.baseUrl`, `memory.embedding.baseUrl` | endpoint, default `http://127.0.0.1:11434/v1` | endpoint, default `https://api.openai.com/v1` |
| `llm.model` | default `llama3` | default `gpt-4o` |
| `memory.embedding.model` | default `nomic-embed-text` | default `text-embedding-3-small` |
| `memory.embedding.dimensions` | `GAH_GATEWAY_EMBEDDING_DIMENSIONS`, or the known size of the model | the same |
| `memory.embedding.sendDimensions` | `false` (Ollama rejects the field) | `true` |
| `llm.apiKey` | `ollama`, or `${TDAI_LLM_API_KEY}` when a generation key is given | `${TDAI_LLM_API_KEY}` |
| `memory.embedding.apiKey` | `ollama`, or `${TDAI_EMBEDDING_API_KEY}` when an embedding key is given | `${TDAI_EMBEDDING_API_KEY}` |

Rules the installers keep:

- A given key is stored only in `tdai-gateway.env`, never in the YAML.
- Without a new key, a key already in the YAML or the env file is kept while
  its endpoint is unchanged. When the endpoint changes, the stored key is
  cleared, so it is never sent to a backend it was not issued for.
- `openai` with no embedding key, given or stored, stops before anything is
  written.
- An embedding model of unknown size stops the install until
  `GAH_GATEWAY_EMBEDDING_DIMENSIONS` is set. The installer does not guess.
- Linux restarts a gateway that was already running, so it picks up the new
  provider. The install then fails unless `/health` reports
  `embeddingService: true`. That shows the configuration is complete; it
  does not show that the model is pulled or the endpoint reachable.

**Validation.** `scripts/validate-gateway-provider.sh <MemoryCore>` seeds
the template, applies the installers' provider step, starts the real
gateway on a scratch port with scratch data, and calls it. Run on
2026-10-07 on Linux (WSL2, Node 22.22.1) against a live Ollama 0.40.0 with
`nomic-embed-text` and `qwen2.5:0.5b` pulled, and no API key anywhere:

```console
$ VALIDATION_ENDPOINT=http://127.0.0.1:11434/v1 VALIDATION_LLM_MODEL=qwen2.5:0.5b \
    scripts/validate-gateway-provider.sh ~/TencentDB-Agent-Memory/MemoryCore
== MemoryCore revision: a0f993ba1eeda16243a8267ba9d1929074b806f9
== backend: http://127.0.0.1:11434/v1
== installer-written provider configuration
{
  "llm": {
    "baseUrl": "http://127.0.0.1:11434/v1",
    "model": "qwen2.5:0.5b",
    "apiKey": "ollama"
  },
  "embedding": {
    "provider": "ollama",
    "baseUrl": "http://127.0.0.1:11434/v1",
    "model": "nomic-embed-text",
    "dimensions": 768,
    "sendDimensions": false,
    "apiKey": "ollama"
  }
}
== GET /health
{"status":"ok","stores":{"vectorStore":true,"embeddingService":true}}
== POST /capture
{"l0_recorded":2,"scheduler_notified":true}
== POST /recall
{"code":0,"message":"ok","memory_count":0}
== POST /search/conversations
{"total":2,"found_captured_turn":true}
== POST /session/end
{"flushed":true}
== gateway embedding log lines
      1 Background embedding complete: 2/2 vectors updated
      2 Using remote embedding (provider=ollama, model=nomic-embed-text)
      1 [hybrid-embedding] Embedding OK, dims=768
```

Ollama's own request log for that run:

```text
      1  200  POST "/v1/chat/completions"
      3  200  POST "/v1/embeddings"
```

What this shows: the installer-written configuration starts the gateway
with embedding enabled, captured turns are embedded by Ollama, a query with
no keyword in common with the captured turn finds it through the vector
index, and ending the session sends one generation request to Ollama.

What it does not show:

- `/recall` returned `memory_count: 0`. Recall reads extracted memories,
  and a two-turn session produced none in the time the script waits. On a
  CPU-only machine, a `/recall` that overlaps a generation request can also
  exceed the gateway's five-second recall timeout.
- The `openai` provider was not run against a hosted API; no key was used
  for this work. Its configuration is covered by `tests/installer_scripts.rs`.
- `install-linux.sh` and `install-macos.sh` were not run end to end on a
  host. Their provider and credential steps run in
  `tests/installer_scripts.rs`, and the provider step ran live as above.

Without `VALIDATION_ENDPOINT` the script uses `scripts/ollama-api-stub.mjs`,
a stand-in that records request shapes, for machines with no Ollama.

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
Remote and reverse-proxied requests still require `COORDINATOR_TOKEN`.
Direct loopback CLI requests and same-origin loopback browser requests may omit
the token. Cross-origin requests and non-loopback Host names do not receive this
exemption. Reverse proxies must send `Forwarded` or `X-Forwarded-*` headers.
Without the opt-in, access is rejected. The central also rejects any node that
advertises the central node's own endpoint, which would make its liveness
poller poll itself and recurse. Re-running registration updates the existing
node's validated endpoint, transport, secret reference, and profile declarations.

### macOS worker transport

The macOS installer saves `GAH_NODE_ADVERTISED_URL` and the transport mode in the worker LaunchAgent.
Later updates use the saved values. Supply new values only when you want to change the transport.

If central can reach the Mac tailnet address, use the default `trusted_lan` transport.
You can set an HTTPS `GAH_NODE_ADVERTISED_URL` to use Tailscale Serve.
For a plain-HTTP central URL, central must set `GAH_ALLOW_INSECURE_HTTP=1`.

If inbound tailnet TCP does not work, use the managed reverse SSH tunnel:

```bash
GAH_NODE_ROLE=worker \
GAH_CENTRAL_URL=http://192.168.5.15:3773 \
COORDINATOR_TOKEN=<central-token> \
GAH_NODE_SSH_TARGET=khing@192.168.5.15 \
GAH_NODE_SSH_REMOTE_PORT=48774 \
bash scripts/install-macos.sh
```

The SSH key must work with `BatchMode=yes` before you run the installer.
The SSH server on central must permit remote TCP forwarding.
The tunnel maps central `127.0.0.1:48774` to Mac `127.0.0.1:3774`.
The installer registers `http://127.0.0.1:48774` with `loopback` transport.

The installer then calls central's `/api/registry/nodes/:nodeId/health` endpoint.
Installation stops if central cannot reach the advertised URL.
Read `~/.local/state/gah/worker-tunnel.log` if the tunnel does not start.

> **Updating an existing registration requires the coordinator token.**
> Creating a new node is how a worker self-registers, so it stays open to
> loopback/authenticated requests. But a re-registration that matches an
> existing `node_id` repoints where the central polls (`advertised_url`) and
> how it authenticates (`secret_ref`), so the route requires a valid
> `COORDINATOR_TOKEN` even for direct loopback requests. Reverse-proxied
> requests always require the token at the shared authentication boundary.

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

The alert is a `node_offline` activity event. It reaches every delivery
method in [Notification delivery methods](#notification-delivery-methods):
push, APNs, the Telegram/Discord channel, and `GAH_NOTIFY_COMMAND`.

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
database. Within one server process, correctness comes from
acquire/renew/release being synchronous functions with no `await` between
the read-check and the write. Across processes, saves replace the whole
file atomically (temp file + rename, owner-only permissions), so
independent writers are explicitly last-write-wins -- a concurrent
writer's lease can be lost, but the file is never left half-written or
corrupt. A store that fails to load (corrupt or wrong-shape JSON) makes
every claims route return a 500 with a server-side log; it is never
silently reset to empty.

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
  profile `claude_path`. A saved subscription token (`claude setup-token`,
  credential kind `claude_subscription_token`) can be bound to an instance
  instead of a browser login; see "Claude subscription token" below.
- **agy / agy-main / agy-second** — `agy` and the `agy-main` wrapper share the
  default `HOME` and therefore one authenticated account/quota pool;
  `agy-second` is isolated by `agy_second_home` as a distinct account.
- **vibe**, **opencode**, **openhands** — their own respective CLI auth.

### Runner permissions

By default, `gah` injects necessary flags so implementation dispatches (`improve`, `experiment`), which run in a GAH worktree, can make progress. Read-only dispatches (`research`, `audit`, `estimate`, `pm`) run in the profile's real checkout and, like review dispatches, receive none of these flags: they keep the backend CLI's own default permissions plus whatever the profile's `codex_args`/`claude_args` set.

- **codex** — Implementation dispatches run with `--sandbox workspace-write --add-dir <CARGO_TARGET_DIR>`: the worktree plus that dispatch's own build target directory, and nothing above it. A profile `codex_args` that chooses its own sandbox (`--sandbox`/`-s`, `--full-auto`, `--dangerously-bypass-approvals-and-sandbox`) replaces the default sandbox mode.
- **claude** — Implementation dispatches run with `--permission-mode acceptEdits` and an `--allowedTools` list of the edit tools (`Edit,Write,MultiEdit,NotebookEdit`) plus a fixed set of shell commands: `git`, `cargo`, `npm`, `npx`, `node`, `pnpm`, `yarn`, `make`, `python`, `python3`, `pytest`, `go`, and the read-only `ls`, `cat`, `head`, `tail`, `wc`, `grep`, `rg`, `find`, plus `mkdir`. Each is passed as `Bash(<command>:*)`. Unrestricted `Bash` is not the default, because Claude Code has no sandbox: it would let an unattended job run any command as the node user, network included. The list is a reduction, not a sandbox — a build script or a test can still run arbitrary code. A project that needs another command (its own validation command, for example) sets `claude_args = ["--allowedTools", "Edit,Write,MultiEdit,NotebookEdit,Bash(git:*),Bash(<command>:*)"]`, or plain `Bash` to accept unrestricted shell. A profile `claude_args` that sets `--permission-mode` (or `--dangerously-skip-permissions`) or `--allowedTools` replaces the matching default.
- **Other runners** — Depend on their own CLI defaults.

If the backend CLI still refuses an implementation run's writes and the attempt
changes nothing, GAH appends a `GAH: backend writes refused (configuration error): …`
line to `backend-output.log` naming the setting to fix, records the attempt as
`environment_error`, and ends the dispatch without spending the remaining
retries.

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

### Notification delivery methods

Events enter the central server's activity feed. One filter,
`notifiableActivity` in `packages/contracts`, decides which events can wake a
person: a chat reply, a chat failure, a chat permission request, an operator
action, a failed dispatch, a ready review, an offline node, and a provider
login that expired or came back. Every delivery method below uses that filter.

| Method | Receives | Enable |
| --- | --- | --- |
| Dashboard alert | Every feed event (in-page); notifiable events as system notifications | Activity page → **Enable system alerts** |
| Web Push | Notifiable events | Same toggle, on an HTTPS dashboard |
| APNs and Live Activity | Notifiable events; Live Activity for each running chat turn | `GAH_APNS_*` on the central server |
| Telegram or Discord channel | Notifiable chat and node events from the feed; dispatch events directly from the Rust CLI | `[defaults] notification_channel` |
| `GAH_NOTIFY_COMMAND` | Notifiable events, as one line on stdin | Set it in the central server's environment |
| Per-profile `notify_command` | Rust dispatch and controller events only | `notify_command` on a profile |

The channel is split by origin. The Rust CLI sends dispatch and controller
events itself, because a worker's event log never reaches the central feed.
The feed sends the events that only the server sees, through
`gah notify-send --title --message --url`. Each event goes to the channel once.

Configure the channel in the GAH config. Credentials stay in the environment
of both the CLI and the central server:

```toml
[defaults]
notification_channel = "telegram"   # or "discord", or "none"
telegram_chat_id = "12345"
```

```bash
# /etc/gah/server.env and the CLI environment
TELEGRAM_BOT_TOKEN=123456:replace-me     # telegram
DISCORD_WEBHOOK_URL=https://discord...   # discord
GAH_NOTIFY_COMMAND=/home/you/bin/notify  # optional shell hook
```

`GAH_NODE_LIVENESS_NOTIFY_COMMAND` still works as an alias for
`GAH_NOTIFY_COMMAND`. The server prints one deprecation line at startup. The
hook now receives every notifiable event, not only node outages. A failing
delivery method is logged and never blocks the others.

### In-app activity and system alerts

The dashboard's **Activity** page receives dispatch completion/failure, review
ready, node offline/back, quota constraint, gateway failure, and operator-action
events through the existing authenticated WebSocket. The server stores a
bounded, de-duplicated feed in `config/activity.jsonl` (override with
`GAH_ACTIVITY_PATH`). A reconnect sends all entries after the client's last
cursor. The durable feed itself does not use a client poller.

The current page always shows a new in-app alert. Select **Enable system
alerts** on the Activity page to add platform delivery. On an HTTPS dashboard,
supported browsers register Web Push so alerts can arrive after the page
closes. The server keeps its VAPID key pair in `config/push/vapid.json` and
subscriptions in `config/push/subscriptions.json`; both files use mode `0600`.
To rotate the VAPID key, delete both files, restart the central server, and
subscribe each device again. Plain HTTP cannot register background push.

The macOS tray app uses a bounded bridge that accepts alerts only from its
configured central origin. The iPhone shell uses APNs when the central server
has `GAH_APNS_KEY_PATH`, `GAH_APNS_KEY_ID`, and `GAH_APNS_TEAM_ID`; otherwise
it retains foreground local notifications. The APNs `.p8` key must remain
outside the repository. `GAH_APNS_ENVIRONMENT` defaults to `sandbox`; set it to
`production` for distribution builds. Device tokens are stored with mode
`0600` in `config/push/apns-devices.json`.

### Notification history

The Activity page opens on **Notifications** when it is opened from a push,
a link, or with unread notifications. It lists every notifiable event from
the last 30 days, newest first, whatever the routine-event cap has evicted.
Opening one notification marks only that one read; **Mark all read** is
explicit. Each item shows one chip per delivery target, for example
"iPhone ✓ · Telegram ✗ (HTTP 401)", so a failed delivery is visible without
reading server logs. Every pushed URL carries `event=<id>`: a push for an
event with no chat scrolls to that event and highlights it.

### Named provider connections

In the desktop app, open **Settings → Provider connections** on the computer
that will run the account. Add an API key with a provider and an account label.
Use separate names for personal, work, and client accounts. **Replace key**
updates one connection. **Remove** removes only the selected connection.

Each connection keeps its secret on that computer. The connection list contains
metadata only. Central receives usage readings from workers without receiving
their keys. Replacing a key clears its previous account reading until a new
check succeeds.

Choose **Check usage** to request a reading for the selected connection.
A saved key does not prove usage access. Providers without a supported usage
endpoint show an unknown allowance. Two keys share an allowance only when the
provider confirms the same billing account.

To run an account, bind its connection to a compatible local instance.
Alternatively, choose **Create local instance** in the connection list.
Instances keep separate runner state. Mistral dashboard sessions provide usage
access and cannot serve as inference keys. AGY accounts use separate CLI logins.
Existing approval requirements for paid credentials still apply.

The CLI supports the same storage through `gah credentials list --json`,
`gah credentials save`, and `gah credentials remove --id NAME`.
The save command reads the secret from stdin. Its `--id`, `--provider`,
`--kind`, and `--account-label` arguments contain metadata only.
`gah quota refresh --credential NAME` checks only that connection.

### Claude subscription token

GAH runs Claude under isolated per-attempt state, so the interactive OAuth
login in `~/.claude` is invisible to dispatched runs. `claude setup-token`
prints one long-lived token for the subscription; save it once per account
and bind it to a Claude instance (issue #1352). The runner receives it only
as `CLAUDE_CODE_OAUTH_TOKEN`, never as an API key:

```sh
claude setup-token            # prints the token; do not paste it into shell history
read -rs TOKEN; printf '%s' "$TOKEN" | \
  gah credentials save --id claude-work --provider claude \
    --kind claude_subscription_token --account-label work
```

The token counts as subscription quota: candidates on a bound instance are
`included_in_quota` and never require paid-route approval. An
`external_credential_scopes` entry for `ANTHROPIC_API_KEY` does not cover the
token; list `CLAUDE_CODE_OAUTH_TOKEN` in a scope if runs with it should need
work-scoped approval. `gah auth-health`
reports each instance independently; a saved token reads as unknown there
because a login check cannot verify it. `gah quota refresh --credential
claude-work` (also run by auto-refresh) asks the subscription usage endpoint
for the 5-hour and weekly windows. That endpoint needs the `user:profile`
scope, and `claude setup-token` tokens are reported to be inference-only; when
the endpoint rejects the scope, the check records "quota unavailable" for the
credential and the instance's windows stay unknown. That is not a token fault.
Save one credential per account and give each instance its own `state_root`,
so concurrent instances never rewrite shared login state. The token is
stored owner-only and never appears in argv, logs, ledger, or telemetry.

### Mistral dashboard usage

In the desktop app, open **Quota → Connect Mistral**, then choose
**Connect Mistral** in local Settings. Sign in in the Mistral window. GAH verifies
usage access and saves the session privately on this device. The device needs
an installed GAH collector to refresh usage.

For another Mistral account, choose **Connect another Mistral account** in Provider
connections. Give the account a separate name and sign in in its isolated
window. This preserves the existing default session. **Reconnect** updates only
the selected account.

For manual setup on macOS or Linux, save the **Cookie header value** from a
signed-in Mistral dashboard request in `~/.config/gah/mistral-dashboard.cookie`.
This is a session credential, not an API key. Keep it on the account-owning node
with mode `600`. `MISTRAL_DASHBOARD_COOKIE_FILE` can select another private file;
the desktop connection flow uses the default path.

Run `gah quota refresh --backend mistral-dashboard` to collect read-only usage
and the current login's Vibe monthly allowance. Automatic checks run at most
every 30 minutes and skip an empty default file. Reconnect in Settings when the
session expires. Account readings stay separate from Vibe execution instances
and task usage; missing allowance or price data remains unknown.

#### Headless sign-in on central

A node without a desktop session, such as central, can sign in to Mistral
itself. Save the console email and password as a `mistral_login` connection.
Type them on that node; they are read from stdin and never appear in argv.
The JSON is built from the environment so quotes and backslashes in the
password survive:

```sh
read -r MISTRAL_EMAIL; read -rs MISTRAL_PASSWORD; export MISTRAL_EMAIL MISTRAL_PASSWORD
python3 -c 'import json, os; print(json.dumps({"email": os.environ["MISTRAL_EMAIL"], "password": os.environ["MISTRAL_PASSWORD"]}))' |
  gah credentials save --id mistral-console --provider mistral \
    --kind mistral_login --account-label "Mistral console"
unset MISTRAL_EMAIL MISTRAL_PASSWORD
```

The connection is stored in the owner-only credential directory and is never
passed to a runner. GAH keeps the resulting dashboard session in the same
private record. It signs in again only when Mistral rejects that session. A
wrong password or a second-factor prompt is reported as `auth_required`, and
GAH stops signing in with that login until it is saved again, so a bad
password is not retried on every refresh. GAH
stores only the email and password, so an account with an authenticator app
needs a reconnect through the desktop window instead.

To let routing use the allowance, bind the Vibe quota pool to the connection:

```toml
[defaults.routing.quota_sources]
vibe-monthly = "mistral-console"
```

Vibe candidates in that pool then see the account's monthly allowance, become
exhausted at 0% remaining, and gain reset pressure. Unbound pools never inherit
another connection's readings.

### Provider login health

Every node runs `gah auth-health` at server start and every 30 minutes. It
checks claude, codex, hermes, each opencode provider (a saved credential that
lists no models has unknown login validity), `gh`, `glab`, and `MISTRAL_API_KEY` /
`NOUS_API_KEY` when set. It also reports backends that a dispatch attempt
marked unavailable for an authentication failure. The command prints fixed,
secret-free details, never provider output. A worker reports the result in
its `/api/status`, so central sees it in the observation it already polls.

Central shows each broken login as a red row under **Nodes → Logins** and
records `auth_expired` when a login that worked stops working, or when a
check reports it expired. A chat turn that fails to authenticate (401/403,
"not logged in", an expired token, an invalid API key) records it at once.
The next successful turn, or a check that turns ok after a new login,
records `auth_restored`. A CLI that was never logged in is shown but not
pushed.

### Repairing a login from another device

A broken login row on **Nodes → Logins**, and an `auth_expired`
notification on the Activity page, has **Fix login**. The login runs on the
machine that owns the credential; the device sees only a link, a one-time
code, fixed prompt text, and the result. A pending repair is stopped after
10 minutes.

| Login | Flow on the owning machine |
| --- | --- |
| codex | `codex login --device-auth`; the device shows its link and code |
| claude | `claude auth login --claudeai`; the device opens the link, then pastes the code back |
| opencode · github-copilot, gh | GitHub's device flow; the token goes to opencode's `auth.json` or to `gh auth login --with-token` |
| other opencode providers | a pasted API key, saved to opencode's `auth.json` |
| Mistral, Nous | a pasted API key, saved to `~/.config/gah/provider-keys.env` (`GAH_PROVIDER_KEYS_PATH`, mode `0600`) and loaded by the server at start |
| hermes, agy, glab | shown as needing a terminal on that machine |

A paired device may start a repair; it is the only credential change open
to paired devices. Only the device that started a repair, holding the key
returned when it started, can read its code or submit a key. A key never
reaches central's storage or logs. A successful repair re-runs the login
check, which records `auth_restored`. Processes the server started before
a key repair (such as a running `gah loop`) see the key after they restart.

### Telegram manager bridge

The central server accepts authenticated Telegram webhook updates at
`POST /api/manager-chat/bridge/telegram`. Set these environment variables on
the central server:

```bash
TELEGRAM_BOT_TOKEN=123456:replace-me
GAH_TELEGRAM_WEBHOOK_SECRET=replace-with-a-random-secret
```

Register the HTTPS webhook with Telegram. Set its `secret_token` to the exact
`GAH_TELEGRAM_WEBHOOK_SECRET` value. Telegram sends that value in the
`X-Telegram-Bot-Api-Secret-Token` header. Use 1–256 letters, digits,
underscores, or hyphens.

Pair one exact Telegram user and chat from the central host. This route needs
owner access and the standard mutation idempotency key:

```bash
curl -X POST http://127.0.0.1:3773/api/manager-chat/bridge/operators \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: telegram-pair-0001' \
  -d '{"externalUserId":"12345","chatId":"12345","role":"owner","profiles":["gah"]}'
```

Use role `chat` when an identity can converse but cannot approve an action.
An owner-paired identity receives exact one-time allow or reject buttons.
Persistent or broad approvals remain available only in the dashboard.

Plain text targets the only paired profile. For multiple profiles, send
`/chat <profile> <message>`. Other remote slash commands are rejected.
The bridge accepts one UTF-8 text, Markdown, or JSON document up to 64 KiB.
It rejects other attachments. It redacts common token forms before context.

Revoke an identity with its returned operator ID:

```bash
curl -X POST http://127.0.0.1:3773/api/manager-chat/bridge/operators/revoke \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: telegram-revoke-0001' \
  -d '{"operatorId":"replace-with-operator-id"}'
```

The bridge keeps receipts and a redacted audit log below the manager-chat
state directory. A retried Telegram update reuses its stored reply. It never
starts the manager turn or answers an action twice.

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

### Agent wake (opt-in autonomy)

A Telegram ping still needs a human to act. GAH can instead queue an
instruction on the profile's durable manager chat when a review requests fixes,
a dispatch fails, or a backend stalls. Resuming the originating worker session
is parked in #1235. Set two things:

- `defaults.current_manager` — the manager that receives the wake. One of
  `claude`, `codex`, `hermes`.
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

Wakes are fire-and-forget but **always logged**: each queued instruction is
written to a timestamped file under the wake log dir
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
| `config_error`       | The runner itself rejected the configured model before the agent ran | Stops work as human-required; never retried or rerouted. Fix the candidate's model in config and confirm with `gah doctor --validate`. |
| `agent_no_progress`  | Failure byte-identical across attempts                        | The agent's edits aren't affecting the error — usually env/config, not the model. Investigate before re-dispatching. |
| `agent_failure`      | Real, changing validation failures                            | Genuine agent-capability miss. This is the only class where capability escalation to a stronger backend is appropriate (`--escalate`, or the loop's Escalate action). |
| `validation_failure` | Validation never passed after all retries                     | Inspect the session diff/logs; consider `--escalate` or a manual fix. |
| `human_blocked`      | Explicitly requires a human                                   | Human gate. Automation stops here by design. |
| `unknown`            | Unclassified                                                  | Stops by default. Inspect session logs before overriding. |

Three consecutive counted attempts with the same `harness_error` or `environment_error` setup signature produce a derived `repeated_setup_failure` human gate. Digits and absolute paths are normalized when comparing errors. Correct the setup problem, then run `gah clear-attempts` to reset the history and release the gate; control records and capacity deferrals do not count as attempts.

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

## Optional factory automation

The factory module is separate from the local application. The desktop's
**Set up this computer** section appears during onboarding and in **Settings →
This computer**. Select **Enable factory automation** before installation, or
use **Save factory module** afterward. The dashboard, chats, agent execution,
repository workflows, and local worker API do not depend on this selection.

Fresh standalone installs default to off. Existing configuration files without
`defaults.factory_enabled` retain the previous enabled behavior. Updates preserve
explicit true/false values. Fresh networked central/worker installations retain
legacy behavior. `GAH_FACTORY_ENABLED=true|false` overrides installer selection;
`gah setup --factory-enabled true|false` forwards that choice. No configuration
file means the read-only setup check reports the module off.

`gah config set --factory-enabled false` disables and stops all installed or
loaded systemd `gah-loop@<profile>.service` instances and the factory watchdog
service/timer. It leaves the loop template available for later enablement.
`gah loop` rejects starts while disabled; an unmanaged loop also exits when its
next iteration reloads the disabled setting. Managed loops stop immediately.
The dashboard checks module policy before enabling a loop unit. Enabling the module does not start loops or restore
previous boot enablement: start each desired project loop explicitly. This avoids
unexpected ticket dispatch on a later enable. Service-control errors are reported,
not treated as successful transitions. The disabled policy is saved first, so
new dispatch is prevented even if service cleanup fails; retry disabling to
complete cleanup.

Shared services remain active: `gah-server`/`gah-worker` serve application and
execution APIs; `gah-prune` performs storage and chat maintenance;
`gah-quota-refresh` collects account telemetry used by both chats and dispatch;
the optional memory gateway serves both workflows. Only dispatch loops and their
watchdog are factory services. macOS uses application/worker LaunchAgents rather
than the Linux factory units; these shared agents remain available.

Read-only setup JSON retains `ready` for compatibility and exposes
`application_ready`, `factory_enabled`, and `factory_ready` separately.
Factory readiness means the module is enabled and setup prerequisites are ready;
project/backend dispatch readiness still comes from the existing doctor checks.

### Local job file contracts

A local Markdown file passed with `gah dispatch --enforce-job-file --mode fix --target path/to/job.md`
can define `Allowed files` and `Verification commands` sections in both `--mode fix`
and `--mode improve`. Without `--enforce-job-file`, nothing is enforced and job
commands are not run or added to the prompt. The flag requires a local `.md` file
with at least one contract section; otherwise dispatch fails before the agent runs. Each section
must contain top-level Markdown bullets (`-`, `*`, or `+`). Indented bullets
are errors; fenced code blocks and explanatory sentences are ignored. A fence
closes only on a bare marker (no info string) at least as long as the one that
opened it, and an unclosed fence is an error. Headings are case-insensitive and
may end with a colon. A section ends at the next heading of the same or a higher
level; a deeper sub-heading inside a contract section, or a second section with
the same name, is an error. `#123` is prose, not a heading. Dispatch takes the first
backtick-delimited value in each bullet, or the trimmed bullet text before
` (` (a note). Empty items, task-list checkboxes (`- [ ] cmd`), bullets that are
only a note (`- (note)`) and sections without bullets are errors.
The agent sees the first 16 KiB of the job file; anything beyond that is still
enforced.
Allowed files are repository-relative paths or globs: `*` matches within a
segment, and a trailing `/` or `/**` allows all files beneath a directory.
Absolute paths and `..` are rejected. After profile validation, dispatch checks
changed paths, then runs the job commands; failures enter the repair retry loop.
Scope and job commands are checked again before publishing, including with
`--allow-draft-fail`. Missing sections disable their respective rules.

These contracts apply only when an operator hands the file to `gah dispatch`
directly. They never apply to a provider issue, and never to a ticket the loop
selected on its own, even if that ticket file has the same headings: there the
sections stay hints. With `--mr` or `--existing-branch`, the scope check also
counts changes already on the branch.

A profile env file cannot set `GAH_ENFORCE_JOB_FILE`; only the `--enforce-job-file` flag turns enforcement on.
