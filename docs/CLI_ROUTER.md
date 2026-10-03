# CLI subscription router

GAH uses [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) as a local subscription gateway. OpenCode runs coding jobs and ACP chat through it. The existing AGY, Claude, Codex, and Vibe runners remain available.

The Quota page shows router accounts, available models, remaining quota, and reset times. It also controls account pause, routing strategy, and session affinity. The central server refreshes router quota at startup and every 15 minutes, even with no dashboard open. The refresh button also offers an immediate check. Each observation includes its check time. A failed refresh shows an error instead of a zero balance.

## Install on a central node

Install Python 3.11 or later and OpenCode first. Run this command as the user that runs the GAH server:

```sh
python3 scripts/setup-cli-router.py --service --register-profile gah
```

If OpenCode is outside `PATH`, add `--opencode /absolute/path/to/opencode`. The installer pins CLIProxyAPI v8.0.10 and verifies the release checksum. Linux uses a user systemd service. macOS uses a LaunchAgent.

For existing AGY CLI logins, repeat `--import-agy-home` for each isolated home:

```sh
python3 scripts/setup-cli-router.py --service --register-profile gah \
  --import-agy-home "$HOME" \
  --import-agy-home "$HOME/.local/share/gah/agy-instances/agy-second"
```

The importer verifies each refresh credential with the upstream OAuth client. It copies credentials into the private router directory. It preserves the original AGY logins. Existing router credentials are retained on repeated installation.

The default directory is `~/.config/gah`. It contains private connection keys, OAuth credentials, and runner state. Keep this directory outside source control. Use `--directory` to choose another location. If that differs from the default, set `GAH_CLI_ROUTER_SETTINGS_PATH` for the GAH server.

The proxy listens on `127.0.0.1:8317`. GAH accesses its management API locally. For additional accounts, use the upstream OAuth console through an SSH tunnel:

```sh
ssh -L 8317:127.0.0.1:8317 user@central-node
```

Open `http://127.0.0.1:8317/management.html`. The management key is in the private `cli-router.json` file on the node. After login, refresh the inventory:

```sh
python3 scripts/setup-cli-router.py --refresh-models
```

## Use the router

Restart the GAH server after registering the `cli-router` instance. Select **CLI router** in the chat runner selector. Select a model from the router inventory. Model IDs use `gah-router/<upstream-model-id>`.

For a direct CLI check on the central node:

```sh
~/.config/gah/gah-cli-router-opencode run \
  --model gah-router/gemini-3.1-pro-low 'Reply with ROUTER_OK.'
```

A different router origin requires both new keys. Blank key fields preserve saved keys only for the existing origin.

The wrapper discovers current models and reads current connection keys at process start. Keys never appear in command arguments. OpenCode state uses a separate home. Automatic conversation sharing is disabled.

The installer adds one explicit backend instance. It preserves existing defaults and candidate priorities. For factory jobs, add the instance and model to the relevant profile candidates through GAH configuration. The proxy balances accounts that support that model. Cross-provider model fallback still uses GAH candidates.

The MCP tool `gah_cli_router` returns the same read-only snapshot. Mutations require an owner session and use GAH audit, rate limits, and idempotency controls. Worker nodes do not expose the router management API.

## Operation and privacy

Nous balances come from `https://portal.nousresearch.com/api/oauth/account`
with an explicit `NOUS_API_KEY`, or the current native Hermes Nous sign-in when
that key is absent. Run `gah quota refresh --backend nous` for a live check.
Automatic refresh supports both sources. Native checks use Hermes's Portal
resolver to renew expiring OAuth credentials without clearing inference
cooldowns. `HERMES_HOME` selects the auth store; a missing custom store never
falls back to the default account. Explicit keys keep precedence, including
when rejected. Do not copy a short-lived Hermes OAuth token into `NOUS_API_KEY`.
GAH uses the existing Hermes virtual environment under
`~/.hermes/hermes-agent/venv` or `/usr/local/lib/hermes-agent/venv`. GAH
records subscription credit percentages in the `nous-portal-api` pool. Purchased
credits and rollover funds are not a monthly quota denominator. If the API does
not supply a valid monthly cap, the percentage remains unknown.

Local collectors can pass one account observation as JSON on stdin to
`gah quota record`. This command requires an explicit backend instance, a check
time, and a source label. It validates percentages and timestamps before it
appends to the shared store. It does not accept credentials or provider payloads.

Linux service commands:

```sh
systemctl --user status gah-cli-router
systemctl --user restart gah-cli-router
journalctl --user -u gah-cli-router --no-pager -n 30
```

The default policy is round robin with one-hour session affinity. The Quota page can select fill first or weighted round robin. Numeric account weights require the upstream console. Earliest-reset selection from the video belongs to its custom dashboard. This integration uses upstream policies.

Telemetry and account training controls are separate. Local telemetry switches do not establish an account training opt-out. Account controls require an authenticated provider session. GAH never stores an invented opt-out header or claims that telemetry controls disable training.

## Checks

```sh
python3 -m unittest discover -s scripts/tests
npm run typecheck
npm run test:server
npm run --workspace=apps/web test:component -- CliRouterPanel.spec.tsx
```

[Video evidence and timestamps](analysis/cli-router-2026-10-02/README.md) explain the upstream choice and the custom dashboard differences.

## Work scheduling and cache reuse

GAH chooses the job, runner, and model. The proxy chooses an eligible account for that model. A quota limit should move work to another eligible account or GAH candidate. A healthy session should retain its account and stable prompt prefix so provider caches can be reused. Reset pressure should choose where new work starts without interrupting productive cached sessions.

GAH's weekly pacing compares remaining quota with the target remaining balance. Unused quota becomes more urgent as reset approaches. Explicit candidate priorities still take precedence. This weekly calculation does not model monthly or five-hour windows.

Router observations are sanitized and appended through `gah quota record` to the durable quota store. Bind router account IDs to native backends with `accountBackends` in the private router settings to apply those readings to the matching native account. Unbound accounts retain opaque identities and cannot imply capacity for a native account. AGY observations distinguish Google-native and external-model pools.

Scheduling uses fresh weekly or monthly subscription balances for reset pressure. Five-hour throttles affect eligibility without creating urgency to spend. Failed account-wide checks invalidate earlier window readings. Named credential removal or rotation also retires its earlier capacity. Each launched dispatch attempt records its selected subscription capacity and reset pressure alongside the actual route. Provider-reported cache reads and writes stay in the existing usage record.
