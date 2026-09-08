# GAH control surfaces

The owner confirmed these requirements on 2026-09-07.

| Surface | Manage the central node | Execute repository work |
| --- | --- | --- |
| Browser | Yes | Through registered workers |
| Windows Tauri app | Yes | Optional WSL worker on the same computer |
| iOS app | Yes | No |
| Android app | Yes | No |

Mobile apps reuse the central dashboard and authenticated connections. They do not install GAH workers, repositories, Rust, Node, or agent CLIs.
Windows requires a native desktop experience. The worker can use WSL for Linux tools.
Native Windows execution remains a separate compatibility requirement.

## QR pairing acceptance criteria

QR pairing is planned. This branch does not implement it.

1. On an authenticated control surface, select **Pair a device**.
2. Generate a short-lived, single-use pairing code.
3. Show a QR code containing the central address and the pairing code.
4. Scan the code on iOS or Android.
5. Show the central identity and requested access before the device confirms pairing.
6. Exchange the code for a separate credential for that device.
7. Store the credential in the platform credential store or an authenticated, persistent web session.
8. Show paired devices and permit individual revocation.

The QR code must not contain the central administrator token, a GitHub credential, or a worker installation command.
A copied QR code must stop working after redemption or expiry. Restarting the central server must invalidate pending codes.
HTTP APIs and WebSocket dispatch must enforce the paired device credential and its revocation.
The existing central WebSocket authentication gap must be resolved before pairing can claim access control.

Use the central address from the pairing payload. Do not embed a private LAN or Tailscale address in an app build.
Provide manual address entry when QR scanning is unavailable.

## Test boundaries

Check iOS and Android separately. Browser viewport tests do not prove native builds or device behavior.
Exercise camera permission refusal, expired codes, replayed codes, wrong servers, revoked credentials, and unavailable networks.
Check keyboard layout, chat streaming, background/resume, and Wi-Fi/cellular reconnection.

The existing iOS handoff starts with Safari validation. Keep that work independent from Windows worker installation.
Add Android coverage and a shared pairing contract before duplicating connection logic in separate mobile shells.
