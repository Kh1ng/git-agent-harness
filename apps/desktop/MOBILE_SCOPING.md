# Mobile targets

The earlier Linux environment lacked Apple tooling. That assessment is obsolete for the current Mac, which has Xcode 26.6.

The iPhone controller now lives in [`apps/ios`](../ios/README.md). It uses SwiftUI and WKWebView to load the central dashboard, with persistent WebKit login, a configurable central address, QR scanning, and reviewed project/chat links. It contains no worker runtime or Rust build dependency.

A successful simulator build does not prove physical installation or mobile network recovery. See [iPhone testing](../../docs/IOS_TESTING.md) for recorded evidence and the manual acceptance run. Issue #936 remains open until its physical-device checks pass.

The Android controller lives in [`apps/android`](../android/README.md). It is a native WebView host for the same central dashboard and stores the selected central address on the device. It contains no worker runtime. CI builds a debug APK; release signing remains a distribution concern.
