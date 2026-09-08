# GAH for iPhone

This app loads the central GAH dashboard in a persistent WKWebView. It has no worker, agent CLI, repository checkout, or local server. Node selection and work control use the central dashboard.

Open `GAH.xcodeproj` in Xcode. Select the GAH target, choose your development team under Signing & Capabilities, and select your connected iPhone. Enable Developer Mode on the phone, then build and run. The deployment target is iOS 17.

## Connect

1. Turn on Tailscale on the iPhone.
2. Open GAH, then Connection. Enter `http://100.118.97.79` or your central address.
3. In an owner dashboard, generate a pairing code under Pair a device.
4. In the iPhone app, scan the QR code or paste its full pairing link into Connection.
5. Review the server address, connect, and confirm the server in the dashboard. Give this device a name.

The native scanner requires camera permission. If scanning is unavailable, paste the pairing link. A QR code proposes a connection; it does not silently grant access. Safari and GAH have separate cookie stores. Pair inside GAH to keep its login after closing the app.

HTTPS certificates use the system trust policy. HTTP is supported for the existing Tailscale deployment through a WebKit-only App Transport Security exception. No native JavaScript bridge is exposed. Main-document navigation stays on the chosen server. User-selected external HTTP(S) links open in the system browser.

Only the server address and dashboard routing fields are saved in preferences. Pairing fragments and other query parameters are excluded. WebKit stores the device cookie. The owner can revoke that device in GAH.

## Project chat links

The dashboard accepts `?page=chat&profile=PROFILE&chat=SESSION_ID`. The native app can open `gah://open?url=ENCODED_DASHBOARD_URL`. Percent-encode the entire dashboard URL. Incoming links always open Connection for review. Ordinary HTTP pairing QR codes also work through the in-app scanner.

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
