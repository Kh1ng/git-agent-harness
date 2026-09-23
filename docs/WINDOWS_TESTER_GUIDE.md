# Windows tester guide

This guide installs one approved GAH test build on Windows. The worker runs inside WSL2 Ubuntu.

Use a disposable repository for the first edit test. Do not use a repository with uncommitted work.

## Get the installer

For release 0.1.2 or later, ask the maintainer for the command from **Settings → General → Add a Node**.
The command downloads matching release files and configures the central connection.

For an unpublished revision, ask the maintainer for these items:

Ask the maintainer for these items:

- One test bundle from a successful Desktop workflow run.
- The full source revision for that workflow run.
- The central GAH URL.
- A temporary central access token.

The unpublished bundle must contain these files from the same workflow run:

```text
desktop-artifact.json
worker-artifact.json
install-windows.ps1
GAH_*_x64-setup.exe
gah
source.tar.gz
install-wsl.sh
```

Do not mix files from different workflow runs. The installer rejects mixed revisions and changed files.

The token gives owner access to the central node. Do not put it in a test report or screenshot.
GAH does not have account sign-in. Device pairing does not grant update or administration access.

## Prepare Windows

1. Use an x64 Windows computer.
2. Install Tailscale and join the same tailnet as the central node.
3. Open PowerShell as administrator.
4. Run `wsl --install --distribution Ubuntu` if Ubuntu is not installed.
5. Restart Windows if the WSL installer requests a restart.
6. Open Ubuntu once and create a normal Linux user.
7. Run `wsl --list --verbose` and make sure that Ubuntu uses WSL version 2.

## Install an unpublished build

Extract the test bundle to one local directory. Then open PowerShell as administrator.

```powershell
$bundle = 'C:\path\to\gah-test-bundle'
$central = 'https://central.example.test'
$secret = Read-Host 'Central access token' -AsSecureString
$credential = New-Object System.Management.Automation.PSCredential('gah', $secret)
$token = $credential.GetNetworkCredential().Password
try {
    & "$bundle\install-windows.ps1" -TestArtifactDirectory $bundle -CentralUrl $central -CoordinatorToken $token -Role both
} finally {
    Remove-Variable token, credential, secret
}
```

The command can stop after it installs WSL. If this occurs, finish the Ubuntu setup and run the command again.

For a published release, run the Add a Node command instead of the local-bundle command.

The installer adds two components:

- The GAH desktop application.
- A worker service inside WSL2.

The `GAH WSL Worker` task starts the worker after Windows sign-in. The desktop application does not own the worker process.

## Prepare one project

1. Open Ubuntu as the same Linux user.
2. Install and authenticate one supported agent CLI.
3. Run `gh auth login` for a GitHub test repository.
4. Open GAH from the Windows Start menu.
5. Connect GAH to the central URL.
6. Enter the central token, or pair this computer from an owner session.
7. Open **Chat**, then select **Import from Git**.
8. Select the Windows worker under **Import on**.
9. Import the disposable repository.
10. Open **Nodes** and select the Windows worker.
11. Select the imported profile and run the readiness check.

Use this command in Ubuntu if the worker does not appear:

```bash
source ~/.local/share/gah/worker/worker.env
systemctl --user status gah-worker.service --no-pager
gah doctor --profile PROFILE_NAME
```

Run `gh auth status` and `claude auth status` if either CLI appears signed out.
Run `command -v codex` before you select Codex. No output means that Codex is not installed inside WSL.

## Do the acceptance test

1. Start a new chat for the disposable project.
2. Select the Windows worker under **Run on node**.
3. Ask the agent to report its working directory and Git branch.
4. Make sure that the path is inside WSL, not on the central node.
5. Ask the agent to create one test file without a commit or push.
6. Make sure that the file exists only in the worker checkout.
7. Close the GAH desktop application.
8. Use another browser to make sure that the worker remains healthy.
9. Restart Windows and sign in.
10. Make sure that the worker becomes healthy again.

## Send the test report

Include these items:

- The workflow run ID and source revision.
- The installer file name.
- The output from `winver`, `wsl --version`, and `wsl --list --verbose`.
- PASS or FAIL for installation, worker health, chat, file creation, and restart recovery.
- The exact error text and step number for each failure.

Do not include tokens, cookies, pairing links, or private source files.

## Current limits

- Agent processes run inside WSL2. Native Windows execution is not implemented.
- The worker starts after Windows sign-in. It does not start at the boot screen.
- A paired device can control work. Owner-only actions require the central token.
- Release 0.1.2 and later support the online Add a Node installer.
- Use a matching Actions bundle only for an unpublished revision.
