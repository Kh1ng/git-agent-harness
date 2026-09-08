# iPhone control testing

## Status on September 8, 2026

The Mac has Xcode 26.6. The connected iPhone 17 Pro Max runs iOS 26.6.1 and has Developer Mode enabled. The earlier Linux-only mobile tooling assessment does not apply to this Mac.

The iPhone simulator build succeeds. Three native XCTest checks validate server-address boundaries, secret-free saved URLs, persistent WebKit cookies across backgrounding and app relaunch, and failed-connection recovery. A failed switch hides the previous dashboard, and Retry preserves an unused pairing code. These tests use a local fixture, not a real provider.

The web changes cover phone touch targets, safe areas, session controls, and URL-based project/chat restoration. Browser emulation and simulator results do not prove physical keyboard behavior, camera scanning, cellular reconnection, or node control on an iPhone.

The owner requested an installable iPhone app. The SwiftUI shell adds a persistent app login, native QR scanner, connection recovery, and reviewed app links while reusing the central web dashboard. It does not fix WebSocket/network behavior by itself. Physical Safari and native network tests remain necessary before closing issue #936. Android packaging remains separate.

## Prepare

1. Connect the iPhone to the Mac and unlock it. Accept Trust This Computer if requested.
2. Sign into Xcode under Settings > Accounts. Select your development team for the GAH target.
3. Enable Developer Mode under iPhone Settings > Privacy & Security if needed.
4. Build and run `apps/ios/GAH.xcodeproj` on the iPhone.
5. Turn on Tailscale. Use central `http://100.118.97.79`.

## Acceptance run

Record a pass or the exact failure for each step. Do not count an untested step as a pass.

1. In GAH, open Connection. Scan a new pairing QR code from the owner dashboard, or paste its full pairing link. Confirm the central address and server identity. Name this device.
2. Close and reopen GAH. Confirm it remains paired. Open Nodes and check the Windows worker named 12VHFAILURE.
3. Open Chat. Select the GAH project and the existing Windows worker acceptance conversation. Confirm earlier messages load and the selected execution node is Windows.
4. Open a new test chat on Windows using Claude. Send: `Read-only mobile check: print GAH_IPHONE_OK and the current working directory. Do not edit files or run builds.` Confirm the reply appears on both phone and desktop. Record the node, conversation ID, and result.
5. Start another harmless read-only turn. Confirm streaming works and Stop remains reachable with the keyboard open. Stop once, and verify central reports the turn has stopped.
6. Type an unsent draft. Rotate the phone both ways, lock and unlock it, and switch apps. Confirm the draft and selected chat remain.
7. While a conversation is open, turn Wi-Fi off and use cellular with Tailscale enabled. Confirm the disconnected state is honest, the connection recovers, and history has no duplicated or missing completed replies. Repeat from cellular to Wi-Fi.
8. Force-quit and reopen GAH. Confirm the same project/chat and completed history return. Unsent draft recovery after process termination is not promised.
9. Open Git, Work, Telemetry, and Settings. Confirm scrolling works, controls remain reachable, and error text fits in portrait and landscape. Inspect Git only; do not create commits for this check.
10. Use a second available node for a separate read-only chat. Confirm node labels distinguish the execution locations. Do not treat existing node switching as committed-file transfer; that handoff feature is still pending.
11. Increase iPhone text size and enable VoiceOver. Check the native Connection form and dashboard navigation. Confirm no required control is hidden behind the keyboard or home indicator.
12. Revoke this phone in the owner dashboard. Confirm HTTP and WebSocket access stop. Pair again with a new one-use code.

Repeat steps 3, 6, 7, and 9 in Safari to distinguish web defects from native-shell defects. Safari requires its own pairing session. Native camera scanning and Wi-Fi/cellular changes require the physical phone.
