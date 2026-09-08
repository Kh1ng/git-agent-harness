# Windows, worker chat, GitLab, and device pairing test script

This is an operator checklist. Record PASS, FAIL, or BLOCKED for each section.
Use disposable repositories for agent edits, pushes, and draft pull requests.
Do not include tokens, pairing links, cookies, or private source files in test reports.

## 1. Record the build and prepare the machines

| Item | Value |
| --- | --- |
| Desktop Actions run | PENDING: replace with the approved test run |
| Source revision | PENDING: replace with the full commit SHA |
| Central revision | Must match the source revision |
| Windows installer | Record the exact `*_x64-setup.exe` filename |
| Windows and WSL versions | Record `winver`, `wsl --version`, and `wsl --list --verbose` |
| Central address | Record the LAN or VPN origin, including the port |
| Worker identity | Record the node ID after installation |
| Test repositories | Record the GitHub, GitLab.com, and custom GitLab URLs |
| Phone/browser | Record the OS version and browser version |

1. Replace the two PENDING values before installation.
2. Start central from the recorded source revision.
3. Configure `COORDINATOR_TOKEN` on central.
4. For trusted LAN worker enrollment, configure `GAH_ALLOW_INSECURE_HTTP=1` on central.
5. For named dashboard origins, include the exact origin in `GAH_WS_ALLOWED_ORIGINS`.
6. Prepare x64 Windows and WSL2 Ubuntu with a non-root default user.
7. Open Ubuntu once to complete its user setup.
8. Prepare a second worker or a matching central checkout for the two-node test.

The worker requires systemd in WSL. Installation can require a Windows restart and a second run.
A successful tool discovery does not prove authentication or repository access.
Native Windows agent execution is not implemented. This script exercises agents inside WSL.
Native iOS and Android artifacts remain unverified. Phone steps below use Safari and Chrome as control surfaces.

## 2. Check Add a Node and install the matching test bundle

1. Open the central dashboard with owner access.
2. Open **Settings**, then **General**, then **Add a Node**.
3. Enter **Central LAN or VPN address**. Use an address Windows can reach.
4. Select **Desktop app + WSL worker** under **Install**.
5. Select **Reveal Windows install command**.
6. Select **Copy command**, or copy the selected text manually.
7. Check that the command targets the chosen central address and installation role.

Expected: the command is available, errors are visible, and failed clipboard access has a manual fallback.
The command contains the central token. It belongs only on the trusted worker computer.

The normal command downloads published release assets. It cannot prove this branch before a matching release exists.
Until release approval, use the matching unpublished Actions bundle below. Record the online download path as BLOCKED.
Do not replace it with an older GitHub release or the CLI `.exe`.

In a normal Windows PowerShell session, download both artifacts from the recorded successful run:

```powershell
$run = 'REPLACE_WITH_APPROVED_RUN_ID'
$root = Join-Path $env:TEMP "gah-test-$run"
gh run download $run -R Kh1ng/git-agent-harness -n gah-worker-windows-nsis -D "$root\desktop"
gh run download $run -R Kh1ng/git-agent-harness -n gah-worker-linux-test -D "$root\worker"
$bundle = Join-Path $root 'bundle'
New-Item -ItemType Directory -Path $bundle -Force | Out-Null
Copy-Item "$root\desktop\*" $bundle
Copy-Item "$root\worker\*" $bundle -Force
```

Open PowerShell as administrator with the same Windows account. Set `$bundle` to the directory from the previous session.

```powershell
$bundle = 'REPLACE_WITH_FULL_BUNDLE_DIRECTORY'
$central = 'http://REPLACE_WITH_CENTRAL_IP:3773'
$secret = Read-Host 'Central access token' -AsSecureString
$credential = New-Object System.Management.Automation.PSCredential('gah', $secret)
$token = $credential.GetNetworkCredential().Password
try {
    & "$bundle\install-windows.ps1" -TestArtifactDirectory $bundle -CentralUrl $central -CoordinatorToken $token -Role both
} finally {
    Remove-Variable token, credential, secret
}
```

Expected: the installer verifies matching revisions and hashes before installation.
A missing file, changed file, mixed revision, or installer from another bundle must stop installation.
For headless-only coverage, repeat on a separate test machine with `-Role worker`.

## 3. Open the native desktop and check worker persistence

1. Open **GAH Worker** from the Windows Start menu.
2. Check that a visible connection window opens without a blank terminal.
3. Open the central dashboard through the connection screen.
4. Authenticate the dashboard with the central token or device pairing from section 7.
5. Enter an unreachable central address in the connection screen.
6. Check that the app shows a recoverable error.
7. Restore the correct address.
8. Open **Nodes** on central.
9. Select the Windows worker and select **Check health**.
10. Record its node ID and advertised address.
11. Close the Tauri app.
12. Select **Check health** again from another browser.
13. Restart Windows and log on with the same account.
14. Check worker health again.
15. Repeat installation with the same bundle and central address.
16. Check that the worker retains its node ID.

Expected: closing the GUI does not stop the worker. The worker returns after Windows logon.
Registration alone must not claim that every backend or repository is ready.
The logon task is `GAH WSL Worker`. The Linux user service is `gah-worker.service`.

## 4. Prepare credentials and import onto the worker

1. Open Ubuntu as the default WSL user.
2. Install and authenticate the selected agent CLI there. Claude alone is sufficient.
3. For GitHub, install and authenticate `gh` there.
4. For GitLab, install and authenticate `glab` for the exact repository host there.
5. Configure repository clone and push credentials in the worker service environment.
6. Open **Chat**, then **Import from Git**.
7. Select the Windows worker under **Import on**.
8. Enter the disposable repository URL under **Git repository URL**.
9. Complete the provider fields from section 6.
10. Select **Import repository**.
11. Check that the project appears under the selected worker.
12. Check the checkout path on that worker.
13. Check that central did not create a checkout for this remote import.
14. Open **Nodes** and select the worker.
15. Select its **Worker profile**, then **Check readiness**.

Expected: import creates the checkout and profile on the selected worker.
Readiness identifies missing authentication or tools. Missing optional backends do not imply that the selected backend is unavailable.
Git pushes and provider API access use separate credentials. `glab auth status` alone does not prove push access.

For a local diagnostic, run this inside Ubuntu:

```bash
source ~/.local/share/gah/worker/worker.env
gah doctor --profile REPLACE_WITH_PROFILE_NAME
systemctl --user status gah-worker.service --no-pager
```

For a manually configured profile, register it with `~/.local/share/gah/worker/register.sh PROFILE_NAME`.
The dashboard import performs its own profile registration.

## 5. Run one conversation on two nodes

1. Import the same disposable repository on a second worker, or configure its matching profile on central.
2. Use the same profile name, provider, repository, and host on both nodes.
3. Check readiness on each node for the selected backend.
4. In **Chat**, select the worker project and select **New chat**.
5. Select **Blank chat**, the project, the backend, and **Run on node**.
6. Name the chat `Worker acceptance` and select **Start chat**.
7. Send: `Report your current working directory and Git branch. Do not change files.`
8. Check that the reply identifies the selected node and its checkout.
9. Send: `Create gah-worker-test.txt containing first node. Do not commit or push.`
10. Check that the file exists only in that node's chat checkout.
11. After the turn finishes, select the second node under **Run on node**.
12. Ask for the working directory, branch, and presence of `gah-worker-test.txt`.
13. Check that the node uses its own checkout and retains the central conversation history.
14. Return to the first node and check that its file remains.
15. Start a bounded task that produces several tool calls.
16. During execution, check that the node selector is disabled.
17. Select **Stop** and check that tool execution stops.
18. Check that a new turn cannot overlap the previous worker process.
19. For a backend with interactive permissions, request an operation that requires approval.
20. Reject the request and check that the operation does not run.
21. Repeat with approval and check that the selected worker performs the operation.
22. Disconnect the worker network and select **Check health** on central.
23. Check that the unavailable node cannot receive a new turn.
24. Restore the network and repeat the health and readiness checks.
25. Open chat **Storage** and check that unknown remote allocation displays **Unknown**, not `0 B`.

26. After both nodes are reachable and idle, select **Archive** for the test chat.
27. Check that each node preserves dirty work in its archive patch and retains the Git branch.
28. Check that central does not access worker paths as local directories.

Expected: the app never silently moves a request to another node.
Each node retains its own checkout. Files do not transfer when the conversation moves.
Record permission checks as BLOCKED if the selected backend does not expose an interactive permission request.

## 6. Repeat provider tests

| Provider | Repository URL example | Repository provider | Extra fields |
| --- | --- | --- | --- |
| GitHub | `https://github.com/owner/gah-test` | GitHub.com or GitLab.com | None |
| GitLab.com | `https://gitlab.com/group/subgroup/gah-test` | GitHub.com or GitLab.com | Numeric GitLab project ID |
| Custom GitLab | `https://gitlab.example.com/group/subgroup/gah-test` | GitLab, including custom hosts | Numeric project ID and `https://gitlab.example.com/api/v4` |

Use a real accessible repository in place of each example.
Custom GitLab supports HTTPS root domains and explicit ports. An installation beneath a URL subpath is outside this test scope.
For GitLab push operations, GAH reads `GITLAB_PAT2` before `GITLAB_PAT`.

For each provider:

1. Complete the worker import and chat checks from sections 4 and 5.
2. Import or configure the same test project on central for the Git page checks.
3. Create a test branch with one harmless committed change.
4. Push that branch to the disposable repository.
5. Open **Git** and its pull request tab.
6. Select **New PR**, enter a title and description, select **Draft**, then select **Create PR**.
7. Check that GitLab produces a merge request on the configured host and nested project namespace.
8. Check the source branch, target branch, draft state, and description at the provider.
9. On the central project, open **New chat**, then **From PR**.
10. Select the test PR or MR.
11. Check that its seeded conversation contains the provider description and correct link.
12. On the remote project, check that **From issue** and **From PR** explain the current central-only setup requirement.
13. Repeat an import with an unavailable host or rejected credentials.
14. Check that the UI reports the error without creating a usable project entry.

Expected: custom GitLab operations stay on the configured host. They do not use GitHub or GitLab.com accidentally.
Provider errors must not reveal credentials or private CLI output in the dashboard.

## 7. Pair a phone and revoke it

1. On the owner dashboard, select **Pair a device** below the access-token control.
2. Enter **Central server address** that the phone can reach.
3. Select **Generate pairing code**.
4. Scan the QR with the phone camera and open the link.
5. Compare the server name, address, server ID, and requested access against the owner screen.
6. Enter a **Device name**, such as `iPhone test`.
7. Select **Confirm server and pair**.
8. Check that the URL no longer contains the pairing fragment.
9. Reload the dashboard and check that the device retains access.
10. Open a chat and send a bounded request to the worker.
11. Check streaming, tool activity, scrolling, and the keyboard layout.
12. Background the browser, then return to it.
13. Check that the conversation restores its current state.
14. Switch networks, then reconnect through an address reachable on the new network.
15. On the owner dashboard, select **Refresh devices**.
16. Select **Revoke iPhone test**.
17. Check that the phone loses its live connection and cannot send new work.
18. Check that other paired devices retain access.
19. Open the redeemed pairing link in a fresh browser session.
20. Check that it cannot issue another device session.
21. Generate another code and wait more than five minutes before opening it.
22. Check that the expired code is rejected.
23. Generate another code, restart central, then open that unused link.
24. Check that the pre-restart code is rejected.
25. With camera access unavailable, use **Pair this device** and **Open a pairing link**.
26. Repeat this section in Android Chrome with a separate device name.

Expected: pairing grants trusted dashboard control, not worker execution on the phone itself.
The phone receives a separate browser session. The QR contains no central token or provider credential.
Revocation blocks future requests. It does not cancel work that central already accepted.
HTTPS is preferred. Explicit trusted LAN HTTP shows its transport warning during confirmation.
Native camera integration, app keychains, and installed mobile webviews require separate native app tests.

## 8. Record results and preserve evidence

For each failure, record the section, step, expected result, actual result, build revision, and affected node.
Include the installer filename for Windows launch failures.
Include a redacted screenshot or short recording for UI failures.
Keep dirty chat worktrees until their files and archive patches are checked.

| Section | PASS / FAIL / BLOCKED | Evidence or issue |
| --- | --- | --- |
| Build identity and prerequisites | | |
| Add a Node and matching bundle installation | | |
| Native desktop and worker persistence | | |
| Worker credentials, remote import, and readiness | | |
| Two-node conversation, stop, and permissions | | |
| GitHub | | |
| GitLab.com | | |
| Custom GitLab | | |
| iOS Safari pairing and control | | |
| Android Chrome pairing and control | | |
| Published release download path | BLOCKED until a matching approved release exists | |
| Native iOS/Android builds and native Windows execution | BLOCKED pending separate implementation or device evidence | |

See [Windows node setup](../WINDOWS_NODE_SETUP.md) for installer details.
See [Control surfaces](../CONTROL_SURFACES.md) for pairing access and storage rules.
