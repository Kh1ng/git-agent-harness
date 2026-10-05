# Optional factory automation (#1317)

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

## Sanitized verification

Isolated test coverage (no installed services, provider writes, or credentials):

- `cargo test --lib factory::`: fresh/no config is off, legacy config remains on,
  explicit values round-trip, and only factory units are classified for stopping.
- `cargo test --test gah_cli setup::factory_toggle`: off → on → off persists the
  selection, stops loop instances/watchdog twice, rejects disabled CLI loop starts,
  and leaves shared services untouched. Enabling never starts services.
- `node scripts/test-desktop-settings.mjs`: GUI default off and repeated enable/
  disable interactions show independent application and factory readiness.
- Server `gahCli.test.ts` verifies disabled dashboard loop starts and enabled
  start/rollback compatibility.

Manual installed-host proof is not included, so acceptance criterion 5 remains
open: no installed-host run has been performed for this change. Before review
approval, on a throwaway standalone host, install with the checkbox off, use
dashboard/chat/repository workflows, enable and explicitly start a configured
loop, then disable and verify its stopped/disabled state alongside active shared
services. Repeat an upgrade with a legacy config and an explicit off config.
Record sanitized GUI screenshots and `systemctl --user` output without
environment values or tokens. No PR was created or updated by this worker;
attach this evidence during the GAH-owned review lifecycle.

### Execution results in the worker sandbox

Passed:

- `cargo fmt --check`
- `cargo test --lib setup::` (21 tests)
- `cargo test --lib factory::` (2 tests)
- `cargo test --lib controller::runtime::` (45 tests)
- `cargo test --test factory_module` (1 test; a running loop's enabled startup
  policy is overridden by the disabled policy on disk before dispatch)
- `cargo test --test gah_cli` (208 tests, including 5 setup tests)
- `env PATH=/home/ramrod/.cargo/bin:/usr/local/bin:/usr/bin:/bin cargo test --test source_structure`
  (31 tests; the inherited WSL PATH contains spaces that break existing shell fixtures)
- `cargo clippy --all-targets --all-features -- -D warnings`
- `npm run typecheck` and `npm run --workspace=apps/desktop typecheck`
- `npm run build:server`, `npm run --workspace=apps/web build`, and
  `npm exec --workspace=apps/desktop -- vite build`
- `node --import tsx --test-name-pattern='factory module disabled' apps/server/src/gahCli.test.ts`
- `bash -n scripts/bootstrap.sh scripts/configure-node-role.sh`
- `git diff --check`

Full-suite success remains unverified. `cargo test` and
`XDG_STATE_HOME=/tmp/gah-1317-state cargo test --no-fail-fast` were run. The latter
library run passed 1871 of 1873 tests; the remaining two route tests passed
individually when `GAH_LEDGER_PATH` pointed to isolated writable temporary files.
The missing-config CLI diagnostic and source-size guard failures discovered by
the full run were fixed; complete CLI and source-structure reruns passed.
Memory/HTTP integration tests cannot bind sockets in this sandbox (`EPERM`).

`npm run test:server` cannot complete successfully here: localhost binding and
process probes are denied. The focused CLI wrapper file passes 20 of 22 tests;
its existing stop/enablement checks encounter `spawnSync systemctl EPERM`.
`npm run --workspace=apps/server test:mock` and `npm test --workspace=apps/mcp-server`
are blocked by the tsx IPC socket restriction. `node scripts/test-desktop-settings.mjs`
and `cargo test --manifest-path apps/desktop/Cargo.toml` were executed successfully
before the review repair below.

### Review repair (unloaded factory checkbox)

**Set up standalone** now sends `factoryEnabled` only after the checkbox was
loaded from the host's setup report or changed by the user; otherwise the
argument is omitted and `configure-node-role.sh` keeps an existing selection.
`scripts/test-desktop-settings.mjs` asserts both cases, but that updated script
is unverified: in the repair sandbox Chromium cannot start (`libnspr4.so` is
missing) and the desktop crate cannot build (`gobject-2.0` is missing). Passed
there: `npm run typecheck`, `npm run --workspace=apps/desktop typecheck`,
`npm exec --workspace=apps/desktop -- vite build`, and `git diff --check`.

### Review repair (fresh macOS standalone default)

macOS has no standalone role, so the desktop's **Set up standalone** installs
the central role and an untouched checkbox sends no factory choice. That left a
fresh macOS standalone install with the module on. The macOS desktop now adds
`GAH_STANDALONE=1` to the setup command, and `configure-node-role.sh` applies
the fresh-install default-off for that marker as well as for the `standalone`
role. An existing configuration file and an explicit `GAH_FACTORY_ENABLED`
still take precedence; a plain networked central install is unchanged.
`cargo test --test gah_cli setup::` covers the central-role cases.

Acceptance criterion 5 is still open after this repair: the worker cannot
install services on a throwaway host or post to the PR, so the installed-host
procedure above has not been run and no screenshots or `systemctl --user`
output exist yet.
