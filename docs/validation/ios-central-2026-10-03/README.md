# iOS central navigation evidence

Date: 2026-10-03. Dashboard build: `0965dcb5`, version 0.1.3.

The current native shell passed an XCTest navigation check on the iPhone 17 Pro simulator.
It opened Nodes, Chat, Git, Telemetry, and Settings against the real central server through an SSH tunnel.
The tunnel used `khing@hermesagent` and forwarded local port 18373 to central port 3773.
This loopback connection used the server's existing local-owner access. It does not prove remote device pairing or direct Tailscale access on the phone.

The test also passed background/resume and cold-launch restoration of Settings.
These screenshots are simulator evidence:

- [Nodes](nodes.png)
- [Chat](chat.png)
- [Git](git.png)
- [Telemetry](telemetry.png)
- [Settings](settings.png)

```text
ControllerTests.testPhysicalCentralNavigationAndBackgroundRecovery passed
Executed 1 test, with 0 failures
TEST EXECUTE SUCCEEDED
```

The physical iPhone 17 Pro Max build and install succeeded with the owner's development team.
The app installed as `com.kh1ng.gah.controller`, version 0.1.0, bundle version 2, and launched at the HTTPS central origin.
The physical XCTest runner timed out while enabling UI automation, before executing test steps.
Phone automation was stopped. No physical navigation, camera, cellular handoff, keyboard, or Live Activity result is claimed here.

The central dashboard address verified during this run is `https://hermesagent.tail82695.ts.net:8443`.
The default HTTPS port currently serves another application. Use the updated phone checklist for the remaining acceptance steps.
