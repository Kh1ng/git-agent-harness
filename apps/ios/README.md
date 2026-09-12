# GAH for iPhone

This app loads the central GAH dashboard in a persistent WKWebView. It has no worker, agent CLI, repository checkout, or local server. Node selection and work control use the central dashboard.

Open `GAH.xcodeproj` in Xcode. Select the GAH target, choose your development team under Signing & Capabilities, and select your connected iPhone. Enable Developer Mode on the phone, then build and run. The deployment target is iOS 17.

## Connect

1. Turn on Tailscale on the iPhone.
2. For first launch, tap **Set up connection** and enter `http://100.118.97.79` or your central address.
3. In an owner dashboard, open **Settings > Connection & pairing** and generate a pairing QR code.
4. In the iPhone dashboard, open **Settings > Connection & pairing > Scan pairing QR code**.
5. Scan the code. If the server address changes, review it before selecting **Open server**.
6. Confirm the server identity in the dashboard and name this device.

Pairing controls live in dashboard Settings. The app has no separate globe button. If central cannot load, **Connection settings** opens a local recovery form. This form accepts a central address or pairing link and includes the scanner.

The native scanner requires camera permission. If scanning is unavailable, paste the full pairing link in Settings. A QR code proposes a connection. The dashboard still requires confirmation. Safari and GAH have separate cookie stores. Pair inside GAH to retain its login after closing the app.

The dashboard Activity page can request local notification permission. While
GAH is running, completed work, failures, reviews, and node-health events use
the same durable WebSocket feed as the web and desktop apps. Event IDs become
iOS notification identifiers, so reconnect replay does not display a duplicate.
This is local foreground delivery, not background APNs.

This Settings scanner requires the updated iPhone app. Older installed apps do not receive native changes from a dashboard refresh.

HTTPS certificates use the system trust policy. HTTP uses a WebKit-only App Transport Security exception for the existing Tailscale deployment. The native bridge accepts pairing scans and bounded activity notifications only from the configured server's main frame. Pairing scans also require the main Settings page. It rejects iframe requests and provides no credential or general command access. Main-document navigation stays on the chosen server. Pairing links to another server require address confirmation. Other user-selected external HTTP(S) links open in the system browser.

Only the server address and dashboard routing fields are saved in preferences. Pairing fragments and other query parameters are excluded. WebKit stores the device cookie. The owner can revoke that device in GAH.

## Project chat links

The dashboard accepts `?page=chat&profile=PROFILE&chat=SESSION_ID`. The native app can open `gah://open?url=ENCODED_DASHBOARD_URL`. Percent-encode the entire dashboard URL. Incoming app links open the local Settings form for address review. Ordinary HTTP pairing QR codes also work through the in-app scanner.

The dashboard keeps its page and selected conversation in the URL. Backgrounding does not reload the web view or discard its draft. After process termination, the saved page and conversation reopen and history comes from central. An unsent draft is not guaranteed after iOS terminates the web process.

## Build and test

Simulator build, without signing or Rust compilation:

```sh
xcodebuild -project apps/ios/GAH.xcodeproj -scheme GAH \
  -configuration Debug -sdk iphonesimulator \
  -derivedDataPath /tmp/gah-ios-derived CODE_SIGNING_ALLOWED=NO build
```

For the native tests, start the local fixture in one terminal:

```sh
python3 apps/ios/GAHTests/fixture.py
```

Choose an available iPhone simulator with `xcrun simctl list devices available`, then run:

```sh
xcodebuild -project apps/ios/GAH.xcodeproj -scheme GAH \
  -destination 'platform=iOS Simulator,id=YOUR_SIMULATOR_ID' \
  -derivedDataPath /tmp/gah-ios-derived CODE_SIGNING_ALLOWED=NO test
```

The fixture binds only to `127.0.0.1:18773`. It tests cookies and navigation without contacting any coding provider. Stop it after testing. The simulator app is under `/tmp/gah-ios-derived/Build/Products/Debug-iphonesimulator/GAH.app`. A simulator build cannot be installed on an iPhone.

Use [the phone testing checklist](../../docs/IOS_TESTING.md) for camera, keyboard, VoiceOver, network changes, and real node control.
