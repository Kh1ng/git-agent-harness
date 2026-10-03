# Getting started

GAH runs coding agents (Claude Code, Codex, opencode) against your GitHub or
GitLab repositories. You decide what a machine is for, and setup installs only
what that choice needs. A command-line-only machine never gets a server; a
worker never gets the dashboard; shared memory is off unless you turn it on.

## Install

### Paste one line (Linux, macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh | bash
```

While the repository is private, use a GitHub token with read access:

```bash
curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" \
  https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/bootstrap.sh \
  | GITHUB_TOKEN="$GITHUB_TOKEN" bash
```

The script needs git and curl. It asks before installing Rust, clones the
repository to `~/git-agent-harness`, builds `gah` (a few minutes the first
time), and hands the terminal to `gah setup`.

`gah setup` then:

1. Asks what this machine is for: the central node with the dashboard, a
   worker for a central node you already run, or the command line only.
2. Asks for a repository checkout to add as your first project, and which
   agent you use.
3. Lists every prerequisite with a ✓ or ✗, and offers to install each missing
   one. A command that needs your administrator password says so before it
   runs. Nothing is installed without a yes.
4. Stops before building if anything required is still missing, and prints
   the exact command for each item.
5. Builds GAH and installs its service (systemd on Linux, launchd on macOS),
   adds the project, and prints what was set up, what was skipped, and how
   to add it later.

Run `gah setup` again at any time. It checks everything and only does what is
missing, so it also resumes a setup that stopped.

### Build from source

```bash
git clone https://github.com/Kh1ng/git-agent-harness.git
cd git-agent-harness
cargo build --release --bin gah
target/release/gah setup
```

To see what a machine has and lacks without changing anything:

```bash
target/release/gah setup --check            # for people
target/release/gah setup --check --json     # for scripts and installers
```

`scripts/install.sh` is the service installer that `gah setup` runs. Its
settings are environment variables, documented in
[OPERATIONS.md](OPERATIONS.md); run it directly only if you already know
which ones you need.

### The desktop app (macOS)

The GAH app from the releases page opens on **Set up this computer**: the
same checklist `gah setup --check` prints. **Install GAH in Terminal** (or
**Finish setup in Terminal** once GAH is installed) runs the paste line or
`gah setup` in Terminal, where it can ask questions and request your
password. **Check again** refreshes the list afterwards. With a GitHub
login in `gh`, the app's paste line also works while the repository is
private.

### macOS release signing

Release builds require these repository secrets:

- `APPLE_CERTIFICATE`: base64-encoded Developer ID Application certificate exported as a `.p12` file.
- `APPLE_CERTIFICATE_PASSWORD`: password for that export.
- `APPLE_SIGNING_IDENTITY`: complete `Developer ID Application: ...` identity.
- `APPLE_ID`: Apple account used for notarization.
- `APPLE_PASSWORD`: app-specific password for that account.
- `APPLE_TEAM_ID`: Apple Developer team identifier.

The release workflow verifies the app signature and notarization ticket. It also notarizes and staples the DMG before publishing.
Missing credentials or failed verification stop the release. Unsigned development artifacts from the Desktop workflow remain available for testing.
See [Tauri's signing instructions](https://v2.tauri.app/distribute/sign/macos/) for certificate export and notarization credentials.

### Windows

Install the desktop app from the releases page. In **Set up this computer**, select the terminal setup button.
The app opens a PowerShell console and offers to enable WSL2 and install Ubuntu.
Approve the Windows elevation prompt. If requested, restart Windows and sign in. The setup console reopens automatically.
Create your Linux user when prompted, then type `exit`. GAH setup continues inside WSL as that user.
If setup fails, select the terminal setup button again. Existing WSL1 distributions require conversion to WSL2 before setup.

For a worker connected to another central node, use the [Windows tester guide](WINDOWS_TESTER_GUIDE.md).
That installer also configures the Windows forwarding port and worker startup task.

## What each choice needs

`gah setup` checks exactly this list. The table is generated from the same
code, so it cannot fall out of date.

<!-- requirements:start -->
| Requirement | Needed for | Why |
| --- | --- | --- |
| git | Everything | Checks out your repositories and the work branches agents create. |
| Rust toolchain (cargo) | Everything | Builds gah itself, and rebuilds it on every `gah update`. |
| Node.js 20 or newer | Everything | Installs your agent CLI through npm, and runs the dashboard server and memory gateway. |
| Your coding agent (Claude Code, Codex, or opencode) | Everything | The coding agent GAH runs for you. |
| Your coding agent's login | Everything | The agent needs its own account to do any work. |
| `gh` (GitHub) or `glab` (GitLab) | Everything | Reads issues and opens pull requests on your repositories. |
| `gh` or `glab` login | Everything | Lets GAH read issues and push branches as you. |
| curl | Dashboard, Worker | The installer uses it to check that services came up. |
| systemd (Linux) or launchd (macOS) | Dashboard, Worker | Keeps the GAH server running and restarts it after a reboot. |
| Tailscale | Dashboard, Worker (recommended) | Reach the dashboard from your phone and other machines over HTTPS, privately. |
| openssl | Shared memory | Generates the memory gateway's access key. |
<!-- requirements:end -->

### Shared memory

Shared memory lets chats and dispatched work recall a project's earlier
context. It costs more than anything else on this list: running it on the
central node needs a checkout of the MemoryCore gateway, its npm packages,
and an OpenAI-compatible API key for the gateway's own model calls. Skip it
at first; add it later with:

```bash
gah setup --memory colocated     # run the gateway on this machine
gah setup --memory remote        # use a gateway running elsewhere
```

Workers never run their own gateway. They reach the central node's memory
through its relay.

## After setup

- **Central node:** open the dashboard on port 3773 of this machine. Pair
  your phone from Settings → Pair a device. Add another machine from
  Settings → Add a Node, which prints the command to paste there.
- **Worker:** it appears under Nodes on the central dashboard. Its projects
  are added from the dashboard's Chat page.
- **Command line only:** run `gah doctor`, then `gah dispatch --profile <project>`.

Add more projects with `gah init` (see the [README](../README.md#onboarding))
or, on a central node, from Chat.

## Unattended installs

`--yes` accepts every default and every offer without prompting. Choices come
from flags; secrets come only from the environment, never from flags, so
they stay out of shell history and process lists.

| Flag | Values |
| --- | --- |
| `--role` | `central`, `worker`, `cli-only` |
| `--agent` | `claude`, `codex`, `opencode` |
| `--provider` | `github`, `gitlab` |
| `--project` | path to a repository checkout |
| `--memory` | `off`, `colocated`, `remote` |
| `--central-url` | the central node's address (worker) |
| `--gateway-url` | a remote memory gateway's address |
| `--memorycore` | a MemoryCore checkout (colocated memory) |

| Environment variable | Used for |
| --- | --- |
| `COORDINATOR_TOKEN` | a worker's access token for its central node |
| `GAH_GATEWAY_API_KEY` | a remote memory gateway's key |
| `GAH_GATEWAY_LLM_API_KEY` | the colocated gateway's model key |

With the paste line, set `GAH_YES=1` and the `GAH_NODE_ROLE`,
`GAH_CENTRAL_URL`, and `GAH_GATEWAY_*` variables on the `bash` side of the
pipe; the script turns them into these flags.

## Troubleshooting

- **"Still missing, so nothing was built yet."** Provide the items listed
  under it, then run the command setup prints.
- **An install command failed.** Setup prints `✗` next to it and moves on.
  Run that command yourself to see its full error.
- **The repository is private and `gah update` cannot pull.** After setup,
  run `gh auth setup-git` once so git uses your `gh` login.
