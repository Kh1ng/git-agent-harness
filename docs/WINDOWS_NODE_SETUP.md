# Windows desktop and WSL worker

The native Tauri desktop manages a central GAH node. The headless worker runs inside WSL2.
The desktop lists Windows and WSL tools separately. A listed executable does not prove authentication or dispatch compatibility.

This implementation requires a Windows test before release. The published desktop 0.1.0 has a hidden startup window.
The new installer requires desktop 0.1.1 or later.

## Install from the central dashboard

1. Open Settings → General → Add a Node.
2. Enter the central node's LAN or VPN address.
3. Select the desktop, the worker, or both.
4. For direct LAN access, enter the central access token.
5. Reveal the Windows install command.
6. For worker installation, open PowerShell as administrator with your normal Windows account.
7. Paste the command.

The desktop installer runs silently. Open **GAH Worker** from the Start menu.
The connection screen remains available when the central node cannot load.

Worker setup requires x64 Windows and an initialized, non-root Ubuntu user in WSL2.
If WSL needs installation or a restart, the command stops with instructions. Complete those steps, then run the command again.

The central server needs these settings:

- `COORDINATOR_TOKEN` for authenticated access.
- `GH_TOKEN`, `GITHUB_TOKEN`, or a working `gh auth login` for private release downloads.
- `GAH_ALLOW_INSECURE_HTTP=1` for the current worker enrollment transport on a trusted LAN or VPN.
- Its Git checkout, including these committed installer changes.

The command contains the central access token. Use it only on a trusted worker computer.
The worker does not receive the central server's GitHub credential.

The worker uses port 3774. Windows forwards that port to WSL and restricts its firewall rule to the central node.
Use a stable Windows LAN or VPN address. If that address changes, run setup again.

The `GAH WSL Worker` task starts at Windows logon. It refreshes WSL forwarding and keeps the distribution active.
The `gah-worker.service` user service owns the worker process. Closing the desktop leaves this service running.
This installation does not start before Windows logon.

Microsoft documents the relevant [WSL networking](https://learn.microsoft.com/en-us/windows/wsl/networking) and [systemd behavior](https://learn.microsoft.com/en-us/windows/wsl/systemd).

## Finish worker readiness

Installation registers the node without repository profiles. It does not advertise the node as ready for a profile.

1. Open Ubuntu as the same WSL user.
2. Install and authenticate the backend that you want to use.
3. For GitHub repositories, install and authenticate `gh`.
4. For GitLab repositories, install and authenticate `glab`.
5. Clone the repository into the WSL filesystem.
6. Add its profile with `gah profile add`.
7. Register its profile name with the command below.
8. Check profile readiness before dispatching work.

The private worker environment adds the installed GAH binary to the shell's path:

```bash
source ~/.local/share/gah/worker/worker.env
gah profile add --help
~/.local/share/gah/worker/register.sh PROFILE_NAME
gah doctor --profile PROFILE_NAME
```

Use the same profile name as the central node. To register several profiles, supply comma-separated names.
A Claude-only node is valid. Other agent CLIs are optional.

Native Windows CLI dispatch is not implemented by this change. Windows executable discovery is informational.
The current worker executes agents inside WSL. Native execution needs separate process, filesystem, and cancellation tests.

## Test without publishing a release

The Desktop workflow can build a matching Windows installer and Linux worker bundle from one revision.
Its Linux job runs only for a manual workflow dispatch. It uploads Actions artifacts and does not publish a release.

After the tested branch is available on GitHub, dispatch it and record the successful run ID and commit:

```sh
gh workflow run desktop.yml --ref BRANCH -R Kh1ng/git-agent-harness
gh run list --workflow desktop.yml --event workflow_dispatch --limit 5 -R Kh1ng/git-agent-harness
gh run watch RUN_ID --exit-status -R Kh1ng/git-agent-harness
gh run view RUN_ID --json headSha -R Kh1ng/git-agent-harness
```

Deploy that revision on central before the worker acceptance test. The local bundle does not update central.
On Windows, use authenticated `gh` downloads from the same successful run. In PowerShell:

```powershell
$run = 'RUN_ID'
$root = Join-Path $env:TEMP "gah-test-$run"
gh run download $run -R Kh1ng/git-agent-harness -n gah-worker-windows-nsis -D "$root\desktop"
gh run download $run -R Kh1ng/git-agent-harness -n gah-worker-linux-test -D "$root\worker"
$bundle = Join-Path $root 'bundle'
New-Item -ItemType Directory -Path $bundle -Force | Out-Null
Copy-Item "$root\desktop\*" $bundle
Copy-Item "$root\worker\*" $bundle -Force
```

Open PowerShell as administrator with your normal Windows account. Set the central URL and read its token without echoing it:

```powershell
$central = 'http://CENTRAL_LAN_IP:3773'
$secret = Read-Host 'Central access token' -AsSecureString
$credential = New-Object System.Management.Automation.PSCredential('gah', $secret)
$token = $credential.GetNetworkCredential().Password
& "$bundle\install-windows.ps1" -TestArtifactDirectory $bundle -CentralUrl $central -CoordinatorToken $token -Role both
Remove-Variable token, credential, secret
```

Set `$bundle` again in the elevated shell if it did not inherit your earlier variables.
Use `-Role worker` for headless-only testing. Desktop-only testing needs only the desktop artifact and accepts `-Role desktop`.

The installer verifies both revision manifests, all file hashes, and its own script before it installs anything.
Mixed revisions, missing files, changed files, and a script from another bundle fail validation.
Checksums establish bundle consistency; they are not publisher signatures. Download through authenticated GitHub access.

This option replaces release downloads with verified local files. The same installer still configures WSL, forwarding, the firewall, and the logon task.
It registers the worker with central. Complete the readiness and acceptance steps below.
This test does not prove the online release-download path. Run the normal Add a Node command after a matching release is approved.

## Windows acceptance test

Use a disposable repository for dispatch tests. Record the installer filename, Windows version, WSL version, and selected distribution.

1. Build or download the NSIS installer and matching worker test bundle from the accepted revision.
2. Install it on Windows without an existing GAH desktop configuration.
3. Open the app from the Start menu.
4. Check that a visible connection window opens without a console window.
5. Enter the central address and open the dashboard.
6. Enter an unreachable address and check that the connection screen remains available.
7. Install the worker through Add a Node.
8. Check its registry identity and declared profiles on the central node.
9. Authenticate Claude inside WSL and configure one repository profile.
10. Dispatch one bounded task to that node.
11. Check logs, terminal output, cancellation, and resulting repository changes.
12. Close the desktop and check that the worker remains reachable.
13. Restart Windows and log on.
14. Check worker reachability and the refreshed WSL forwarding address.
15. Run setup again and check that the node identity remains unchanged.

Also test missing WSL, WSL1, an uninitialized Linux user, missing systemd, rejected credentials, and unavailable release assets.
Check that each failure stops setup without claiming readiness.

## Local checks

```bash
npm run build:contracts
npm run build:shared
npm run --workspace=apps/server typecheck
npm run --workspace=apps/web typecheck
npm run --workspace=apps/desktop typecheck
npm exec tsx -- --test apps/server/src/nodeSetup.test.ts apps/server/src/fleetDispatch.test.ts
python3 scripts/test-wsl-worker-config.py
pwsh -NoProfile -File scripts/test-windows-installer.ps1
CARGO_BUILD_JOBS=1 cargo test --manifest-path apps/desktop/Cargo.toml --bin gah-desktop -- --test-threads=1
CARGO_BUILD_JOBS=1 cargo clippy --manifest-path apps/desktop/Cargo.toml --bin gah-desktop -- -D warnings
```

## Evidence from 2026-09-07

The macOS checks passed: desktop Rust test, desktop Clippy, three workspace typechecks, and both frontend builds.
Twelve focused server tests passed. These cover installer selection, command validation, and authenticated fleet dispatch and reconciliation.
The PowerShell parser checked the installer and its embedded task script. The WSL configuration test checked quoting, credential permissions, and stable identity.
A Chromium smoke test checked the desktop form, separate tool environments, visible errors, and horizontal overflow with a mocked native bridge.

Actual Windows launch, WSL installation, Windows logon, LAN forwarding, and a real backend dispatch remain untested.
No release was published. Native iOS/Android builds and QR pairing remain separate work.
