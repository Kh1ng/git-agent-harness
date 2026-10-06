# Linux AppImage onboarding validation (#1326)

## Release gate

`.github/workflows/release-linux.yml` runs `scripts/test-linux-appimage.py` on
exactly the AppImage it is about to publish. Failure or unsupported execution
blocks publication. `linux-appimage-gui-smoke` workflow artifacts retain the
SHA-256, extraction/runtime logs, and window screenshots, including failures.
This is native WebKitGTK validation, separate from browser fixture tests.

The runner extracts the AppImage without FUSE, launches its AppRun, and isolates
HOME and all XDG directories. It removes inherited credentials and custom CLI
paths. There is no GAH server, config, checkout, or CLI login in that fresh home.
It never activates setup or installs anything. Success requires:

- A showing web document in the launched process's accessibility tree.
- Settings and setup headings, plus the dynamic “GAH is not installed on this
  computer yet.” response from real native `setup_check` IPC.
- An enabled, sensitive “Set up standalone” button.
- A screenshot of the app window with “Set up standalone” recognized by OCR,
  after scrolling to the setup action. Accessibility alone cannot prove paint.

Exit codes: 0 passed, 1 failed artifact/runtime check, 2 unsupported prerequisites.
A screenshot/OCR failure is a failure, never a pass inferred from process survival.
Only the app window is captured; unrelated desktop windows are excluded.

## Reproduce against a downloaded release

On an x86_64 Ubuntu Linux machine with WebKitGTK 4.1 and the usual Tauri runtime
dependencies, install these test tools:

```sh
sudo apt-get install xvfb xauth dbus-x11 at-spi2-core python3-pyatspi \
  imagemagick xdotool tesseract-ocr
chmod +x /absolute/path/GAH.AppImage
# The output directory must be empty or absent.
dbus-run-session -- xvfb-run -a -s '-screen 0 1440x1000x24' \
  env LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  /usr/bin/python3 scripts/test-linux-appimage.py /absolute/path/GAH.AppImage \
  --environment headless --evidence /tmp/gah-linux-headless-proof
```

Repeat without the two rendering overrides, using a different evidence directory.
Record the release URL/tag and verify the artifact hash before comparing results.
System Python is required so Ubuntu's pyatspi package is visible.

## Interactive fresh desktop check (required separately)

Use a fresh Ubuntu 24.04 x86_64 desktop VM or physical desktop, with a human
present and an X11 session (or a working XWayland accessibility path). Do not
label Xvfb, Docker, WSL, or an unattended virtual display interactive. Run the
same downloaded artifact, without rendering overrides:

```sh
GAH_INTERACTIVE_DESKTOP_CONFIRMED=1 \
  /usr/bin/python3 scripts/test-linux-appimage.py /absolute/path/GAH.AppImage \
  --environment interactive --evidence /tmp/gah-linux-desktop-proof
```

Review `initial.png` and `onboarding.png` visually. Confirm onboarding text is
readable, the setup control is reachable, and no prior server/config/login is
needed. Record Ubuntu version, desktop/session, GPU/driver, artifact URL/hash,
and the human's result beside the proof. The environment flag is an attestation,
not automatic detection of an interactive desktop. An automated pass alone does
not satisfy interactive acceptance. Attach both proof directories to the ticket
or release validation record after checking for sensitive information.

Pure Wayland without XWayland, missing accessibility bridge, unavailable session
D-Bus, and this worker's WSL environment without WebKitGTK/Xvfb are unsupported
by this X11 runner. A supported desktop whose document stays blank is a failure.
OCR failures require screenshot review; do not disable the gate to publish.

## Investigation status and diagnostic boundaries

The ticket reports blank content in Ubuntu 24.04 under Xvfb, both inside Docker
and on a native host. Software rendering and disabling DMABUF did not resolve
those observations. Both are headless; interactive behavior remains unknown.

Source inspection found:

- `apps/desktop/index.html` includes static Settings and setup content; Vite
  builds it into `apps/desktop/dist`, referenced by Tauri `frontendDist`.
- `apps/desktop/main.rs` initially requests `WebviewUrl::App("index.html")`.
  Its navigation policy permits the local `tauri://localhost/index.html` URL,
  and native control permissions are restricted to the local document.
- Startup checks use native IPC; no initial frontend fetch from a running GAH
  server is needed. The release workflow builds Vite before bundling.

These facts do **not** establish that published v0.1.3 contains correct assets,
that its custom protocol successfully serves them, or that WebKit paints them.
No speculative renderer or security workaround is applied.

This worker cannot complete the artifact diagnosis: GitHub DNS resolution fails,
the GitHub CLI launcher fails in WSL, WebKitGTK 4.1 and Xvfb are absent, and no
interactive desktop is available. No artifact screenshots or passing native run
are claimed. Interactive validation and the root cause are still open.

For the original published artifact and a new build, compare hashes and retain
all smoke outputs. If no web document appears, inspect runtime logs for WebKit
process/sandbox errors, confirm the binary contains bundled Settings assets,
and inspect the custom-protocol response and navigation using a diagnostic
WebKit build/inspector. If the document appears but IPC text does not, inspect
module loading, asset MIME types, console errors, and local capability checks.
If accessibility passes but OCR fails, inspect the screenshot and WebKit paint,
GPU and compositor behavior. Preserve each rendering configuration as a distinct
run. Do not infer a container-only cause until the interactive check passes.

## Local test of the gate contract

```sh
python3 -m unittest discover -s scripts/tests -p test_linux_appimage.py
```

These synthetic contract tests demonstrate rejection of menus, blank documents,
missing IPC text, hidden documents, and disabled actions. They are not native GUI
proof and cannot close #1326.
