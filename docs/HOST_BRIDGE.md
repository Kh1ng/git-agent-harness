# Host bridge contract

The dashboard web app runs in four hosts: a plain browser (including the
installable PWA), the Tauri desktop shell on Linux/Windows/macOS, the iOS
WKWebView shell, and the Android WebView shell. `apps/web/src/lib/hostBridge.ts`
is the single module that knows which host is present and how each host
services a capability; UI code must go through it (today via the
`ExternalAnchor` component) instead of hand-rolling `target="_blank"` or
host-specific checks.

## Why `target="_blank"` is banned in the dashboard

- The iOS shell implements no `createWebViewWith` handler, so new-window
  requests are silently discarded — dead links.
- The desktop shell's navigation policy blocks every navigation that is
  not the configured central, so neither a new window nor an in-place
  navigation to a provider URL can succeed on its own.
- The Android shell tolerates new windows only because its WebView folds
  them into the same view; relying on that is fragile.

## Capability: opening an external HTTP(S) URL

`ExternalAnchor` renders a normal `<a>` (never with `target`) and handles
the click per host:

| Host | Detection | Open mechanism |
| --- | --- | --- |
| Desktop (Linux/Windows/macOS) | `__GAH_DESKTOP_EXTERNAL_LINKS__` marker injected by the shell | `open_external_url` Tauri command: validated plain HTTP(S) URL, guarded by the configured-central trust boundary, opened with the OS opener (`open`, `explorer.exe`, `xdg-open`). Falls back to a `noopener` tab on older shells without the command. |
| iOS shell | `window.webkit.messageHandlers.gahController` | **Passthrough**: the default anchor navigation runs, and the shell's navigation delegate opens it via `UIApplication.open`. Programmatic `location.href` assignments are *not* routed this way; only real anchor clicks are. |
| Android shell | `GAH-Android` user-agent marker | **Passthrough**: the shell's `shouldOverrideUrlLoading` opens external origins with `ACTION_VIEW`. |
| Plain browser | none of the above | `window.open(url, '_blank', 'noopener,noreferrer')`. |

Modified clicks (cmd/ctrl/shift/alt, non-primary buttons) always fall
through to the browser default in every host.

### Host implementer requirements

Any new native host must satisfy one of the two contracts above for
external links:

1. **Bridge contract** (desktop-style): expose a command/message that
   validates the URL as plain HTTP(S) without credentials, verifies the
   caller is the trusted dashboard document, and opens the OS browser
   itself. Mark its availability with a `__GAH_DESKTOP_*`-style inert
   global before the dashboard loads.
2. **Passthrough contract** (mobile-style): open the system browser for
   real anchor navigations whose origin is not the configured central,
   and discard nothing the dashboard sends as a same-document anchor.

### Desktop command registration checklist

- `build.rs`: add the command to `AppManifest::new().commands(&[...])`
  so its ACL permission is generated.
- `tauri.conf.json`: grant the permission in a `remote` capability scoped
  to `http://*`/`https://*` windows labeled `dashboard` (remote dashboards
  can invoke it; local pages cannot).
- `main.rs`: validate the URL with `external_url` and the caller with
  `open_project::configured_central` before acting.

## Extending the bridge

A new host capability (clipboard, file save, native share) should:

1. be named as a method or component in `hostBridge.ts`/a component that
   sits on it, with a contract row per host in this document;
2. define behavior for all four hosts in that one place — no host checks
   outside the bridge;
3. prefer passthrough or a single bridge call so callers never branch on
   host identity.
