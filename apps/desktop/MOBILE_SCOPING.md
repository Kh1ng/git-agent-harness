# Mobile Targets Scoping (Android / iOS)

Scoping analysis for adding Android/iOS Tauri mobile targets to `apps/desktop`.
Related issue: #144 (parent #139). This is a scoping/PM document only — no
mobile support is implemented here.

## Conclusion

**Mobile targets are NOT buildable in this environment. Closed without further
action (see #144).**

## Environment inventory (Debian GNU/Linux 13, x86_64)

| Requirement                         | Present? | Notes                                            |
| ----------------------------------- | -------- | ------------------------------------------------ |
| Android SDK (`sdkmanager`, `adb`)   | No       | Not installed; `ANDROID_HOME`/`ANDROID_SDK_ROOT` unset |
| Android NDK                         | No       | Required for native (Rust) mobile builds         |
| JDK / `javac`                       | No       | Gradle (Android build step) requires a JDK       |
| `cargo ndk`                         | No       | Not installed                                    |
| `tauri` CLI (`tauri android init`)  | No       | Not installed via cargo or npx                   |
| Rust mobile targets                 | No       | Only `x86_64-unknown-linux-gnu` installed        |
| Signing (`keytool`)                 | No       | No keystore / signing tooling                    |
| Xcode / `xcodebuild` / `swift`      | No       | **iOS requires macOS + Xcode**, unavailable on Linux |
| `apps/web` production build         | N/A      | Not built locally; webview reuse unverified       |

## Rationale

- **iOS** cannot be targeted from Linux at all — Xcode and the iOS SDK are
  macOS-only. There is no path to validate an iOS build in this environment.
- **Android** would require the Android SDK + NDK, a JDK, `cargo-ndk`, and the
  Tauri CLI (`tauri android init`), none of which are present. Installing and
  provisioning these is out of scope for this ticket and cannot be validated
  without the signing/SDK toolchain.

## Follow-up (only if/when tooling is available)

If mobile tooling is provisioned later, the work should decompose into:

1. **#TBD-Android**: `tauri android init`, install Android SDK/NDK + JDK, add
   `aarch64-linux-android`/`armv7-linux-androideabi` Rust targets, configure
   signing, and verify a debug APK build.
2. **#TBD-iOS**: provision a macOS runner with Xcode, `tauri ios init`, and
   verify a simulator build.
3. **#TBD-WebView**: confirm the existing `apps/web` build runs unmodified
   inside the mobile webview, and enumerate any capabilities (notifications,
   storage, deep links, permissions) needing mobile-specific handling.

Until that tooling exists, neither target should be started.
