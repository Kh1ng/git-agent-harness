# Native Linux onboarding acceptance — #1320

Status: **blocked_on_human**. Acceptance criterion 7 requires human execution of this checklist and cannot be satisfied by automated worker tests.
The browser test's mocked host and screenshots cannot establish native Linux
authentication, permission prompts, service installation or dashboard completion.

Use a disposable fresh Linux desktop VM with a new user, a graphical session and
a running PolicyKit authentication agent. Do not use an existing GAH installation
or replace the native Tauri bridge, repository CLI, coding agent or installer with
mocks. Use test accounts and a test memory gateway. Record the distribution,
desktop/session type, tested source SHA, desktop artifact checksum and CLI version
in the evidence handoff. If onboarding downloads a different source revision,
record that revision too; do not attribute its behavior to the desktop SHA.

Take a VM snapshot before onboarding so the memory variants can each start fresh.
Launch the real desktop app from the desktop environment. Perform setup through
Settings; observe whether any step unexpectedly requires an interactive terminal.

| Step | Action | Required observation / capture |
| --- | --- | --- |
| Missing packages | With the selected repository CLI absent, select GitHub, then GitLab. Try the repository login action. | Installation guidance appears and login is blocked before credentials are sent. Install the selected CLI using its official guide and use **Check again**. Capture the missing-package state. |
| Explicit choices | Choose **Local standalone**, a coding agent, a repository provider, **Skip shared memory**, and the desired optional factory setting. | Choices are visible before installation. Record the chosen values without account or repository identifiers. |
| Repository failure | Submit an intentionally invalid test token. | The real CLI rejects it; Settings shows a recoverable failure. Capture the error with the token field and identifying data omitted. Retry with a valid test token and check the real CLI authentication state. |
| Coding-agent login | Use **Sign in to coding agent** and complete the actual browser/device login. | Login completes and the setup prerequisite check reflects it. Record the outcome; do not capture a device code, account page or credential. |
| Permission failure | Start **Install selected configuration**, then cancel or deny the actual PolicyKit prompt. | Setup reports failure and allows retry. Capture the sanitized Settings failure and progress. Record that a real PolicyKit prompt was observed. |
| Retry | Retry installation with the same choices and approve the native permission prompt. | The selected choices persist, real installation completes and Settings reports completion. Capture the retry/progress and completion states. |
| Local dashboard | Open the resulting dashboard from the app. | The loopback dashboard loads and responds without Tailscale or another GAH node. Capture the sanitized loaded dashboard and record the loopback URL and service health. A completion label alone is insufficient. |
| Shared memory | Restore the fresh snapshot and repeat with **Use an existing memory gateway** and a test gateway. | The selected gateway is used successfully. Record a sanitized gateway health/request result; never include its API key. Keep the skip and connected outcomes separate. |
| Optional factory | If selected, configure a test factory profile through Settings. | Profile configuration succeeds. Do not claim that this validates factory dispatch or service lifecycle, tracked separately in #1317. |

If a step fails, record the observed failure and stop claiming acceptance for that
step. After a fix, repeat failure, retry and local completion on the tested
revision. Do not mark missing observations as passed.

Before handing captures to the review owner, inspect every image/video frame and
remove tokens, keys, account identifiers, repository identifiers and login codes.
Prefer cropping/redaction of a copy; never commit unsanitized originals. Include
only sanitized captures in the evidence package, with relative links in this
record:

```text
Run date:
Distribution / desktop / session type:
Fresh VM and new-user baseline:
Desktop source SHA / artifact SHA-256:
Installed CLI version / source SHA:
Real PolicyKit prompt observed:
Missing-package result / capture:
Rejected repository login result / capture:
Coding-agent login result (no credentials or codes):
Denied installation result / capture:
Retry with preserved choices result / capture:
Successful loopback dashboard result / capture:
Shared memory skipped result:
Shared memory connected result / sanitized gateway evidence:
Optional factory result, or skipped:
Unexpected terminal interactions:
Outstanding failures:
Sanitization reviewer:
```

The lifecycle owner must attach the sanitized native failure, retry and successful
local completion captures and this completed record to the PR before merging.
Worker execution of this checklist does not authorize creating or updating a PR.
