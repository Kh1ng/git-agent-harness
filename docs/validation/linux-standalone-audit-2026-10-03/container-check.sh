#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq xvfb xauth dbus-x11 imagemagick python3-pyatspi libgtk-3-0 libwebkit2gtk-4.1-0 libayatana-appindicator3-1 x11-utils > /tmp/dependencies.log 2>&1
useradd -m -s /bin/bash app-tester
mkdir -p /tmp/app
cd /tmp/app
/artifacts/release.AppImage --appimage-extract >/tmp/extract.log 2>&1
chown -R app-tester:app-tester /tmp/app
printf 'bundle_runtime_files='; find squashfs-root -type f \( -name gah -o -name node -o -name bin.js \) | wc -l
su - app-tester -c 'set +e; export WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1; for tool in cargo npm node gh glab tailscale; do command -v "$tool" || echo "$tool absent"; done; cd /tmp/app/squashfs-root; timeout 20s xvfb-run -a -s "-screen 0 1600x1100x24" dbus-run-session -- sh -c '\''./AppRun >/tmp/gui-launch.log 2>&1 & app_pid=$!; sleep 12; import -window root /proofs/fresh-linux.png; python3 /artifacts/accessibility.py; kill -0 "$app_pid"; echo "app_process_check_exit=$?"; xwininfo -root -tree | head -25; kill "$app_pid"; wait "$app_pid"'\''; cat /tmp/gui-launch.log'
