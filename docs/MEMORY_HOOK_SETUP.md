# Shared agent memory hooks

`gah setup memory-hooks` installs the shared TDAI memory hook for selected agent tools. Run it on each worker where those tools run. On Windows, run this command inside WSL. The installer supports macOS and Linux and requires Python 3.10 or newer.

Choose the tools to configure:

```sh
gah setup memory-hooks --tool claude,codex
# Include Hermes when it is installed:
gah setup memory-hooks --tool claude,codex,hermes
```

The command installs `~/.local/bin/gah-memory-hook` and merges hook entries into these files:

| Tool | Configuration | Events |
| --- | --- | --- |
| Claude Code | `~/.claude/settings.json` | SessionStart, Stop |
| Codex | `~/.codex/hooks.json` | SessionStart, Stop |
| Hermes | `~/.hermes/config.yaml` | on_session_start, on_session_end |

Only selected tools are changed. Other hooks and settings remain in place. Hermes YAML comments and quotes are preserved with its existing `ruamel.yaml` library. The installer selects the standard Hermes virtual environment when available. For a custom installation, pass `--python /path/to/venv/bin/python`.

In Codex, review and trust the hooks through `/hooks`. In Hermes, review the new hooks interactively before headless use. Setup preserves existing approval settings, including `hooks_auto_accept`.

## Connect a gateway

If no gateway is configured, the installed hooks remain inactive. Setup does not install or start a gateway service. After deploying a gateway, run:

```sh
gah setup memory-hooks --tool claude,codex --gateway-url http://127.0.0.1:8420
```

The URL is stored in `~/.config/gah/memory-hooks.json`. `TDAI_GATEWAY_URL` overrides it at runtime. Use a URL reachable from the worker's environment; WSL loopback refers to WSL. Use HTTPS when the connection requires transport encryption.

Keep credentials out of the command line. Hooks read `TDAI_GATEWAY_API_KEY` from the agent's environment or a `TDAI_GATEWAY_API_KEY=...` line in `~/.config/gah/tdai-gateway.env`. Protect that file with mode `0600`. Existing installations with this file retain their default gateway URL, `http://127.0.0.1:8420`.

Setup never calls the gateway or reads agent transcripts. At runtime, the bundled hook retains the reference implementation's recall, capture, and flush behavior. A gateway failure logs a diagnostic and skips memory for that event. Requests time out after eight seconds; errors do not fail the agent session.

## Repeat, recover, and verify

Re-running the same command does not duplicate GAH hooks or create backups for unchanged files. Before changing an existing file, setup saves a private sibling backup named `<filename>.gah-backup-*` and prints its path. Invalid selected configurations fail before any configuration is replaced. A write failure rolls back files written by that invocation, provided another process has not changed them.

To undo setup, close the affected agent tools and restore the printed backups. On a fresh installation, remove only the GAH hook entries and the installed script. Do not delete a configuration file that now contains other settings. Setup refuses to replace symlinked files; update the managed source of a symlink instead.

For an isolated installation check, use a temporary directory:

```sh
gah setup memory-hooks --tool claude,codex --home-dir /tmp/gah-memory-check
```

`--home-dir` selects the installation destination. Runtime gateway settings still belong to the account running the agent; this option does not switch accounts. `GAH_MEMORY_HOOK_CONFIG` can select an alternate gateway settings file.

The automated checks cover safe merges, repeat runs, malformed configuration, rollback, quoted paths, and missing or unavailable gateways. They use temporary files and mocked HTTP. They do not verify a new live session with every provider version; native hook contracts and transcript formats can change.

This feature packages the working central-node reference recorded in [#858](https://github.com/Kh1ng/git-agent-harness/issues/858). Historical session backfill remains outside its scope.
