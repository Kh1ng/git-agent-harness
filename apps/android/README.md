# Android controller

This app is a small native host for the shared GAH web control surface. It stores the central server address and WebView session on the device. It does not contain a worker, backend, or local control-plane server.

Build and test:

```sh
gradle -p apps/android testDebugUnitTest assembleDebug
```

Install the debug APK on a connected device or emulator:

```sh
adb install -r apps/android/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n com.kh1ng.gah/.MainActivity
```

Enter the HTTPS tailnet name for the central node. HTTP is also accepted for a private tailnet address. Navigation within that origin stays in the app; external HTTP(S) links open in the system browser.
