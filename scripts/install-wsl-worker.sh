#!/usr/bin/env bash
# Called by install-windows.ps1 inside the selected WSL distribution.
set -euo pipefail
stage="${1:?Missing installer staging directory}"
[ "$(id -u)" != 0 ] || { echo 'Use a non-root WSL user.' >&2; exit 1; }
[ "$(uname -m)" = x86_64 ] || { echo 'This release requires x86_64 WSL.' >&2; exit 1; }
if ! systemctl --user show-environment >/dev/null 2>&1; then
  echo 'Enable systemd in WSL (/etc/wsl.conf: [boot] systemd=true), run wsl --shutdown from Windows, reopen the distribution, then rerun setup.' >&2
  exit 1
fi
# An already provisioned distro must not need a sudo password to install the worker.
missing_packages=()
for package in ca-certificates curl git xz-utils build-essential; do
  if [ "$(dpkg-query -W -f='${db:Status-Status}' "$package" 2>/dev/null || true)" != 'installed' ]; then
    missing_packages+=("$package")
  fi
done
if [ "${#missing_packages[@]}" -gt 0 ]; then
  sudo apt-get update
  sudo apt-get install -y "${missing_packages[@]}"
fi
install_dir="$HOME/.local/share/gah/worker"
mkdir -p "$install_dir"
chmod 700 "$install_dir"
# Each build has its own directory so a failed upgrade leaves the running service intact.
release_dir="$(mktemp -d "$install_dir/release.XXXXXXXX")"
tar -xzf "$stage/source.tar.gz" -C "$release_dir"
mkdir -p "$release_dir/bin"
install -m 755 "$stage/gah" "$release_dir/bin/gah"
# role-cli-check:start -- also exercised without installing a service.
if ! "$release_dir/bin/gah" config set --help | grep -q -- '--node-role' ||
   ! "$release_dir/bin/gah" status --help | grep -q -- '--role' ||
   ! "$release_dir/bin/gah" installer --help >/dev/null 2>&1; then
  echo 'The downloaded GAH CLI is too old for this worker installer. Publish/install a matching CLI release with config --node-role, status --role, and installer support. The existing worker service and settings have not changed.' >&2
  exit 1
fi
# role-cli-check:end
# WSL inherits Windows PATH entries; select Node and npm from the same Linux installation.
node="$(node -p 'process.platform === "linux" && Number(process.versions.node.split(".")[0]) >= 20 ? process.execPath : ""' 2>/dev/null || true)"
if [ -z "$node" ] || [ ! -x "$(dirname "$node")/npm" ]; then
  node_dir="$install_dir/node"
  mkdir -p "$node_dir"
  curl -fsSL https://nodejs.org/dist/latest-v22.x/SHASUMS256.txt -o "$release_dir/SHASUMS256.txt"
  node_archive="$(awk '$2 ~ /^node-v22\.[0-9]+\.[0-9]+-linux-x64.tar.xz$/ { print $2; exit }' "$release_dir/SHASUMS256.txt")"
  [ -n "$node_archive" ] || { echo 'Cannot locate a Node 22 Linux release.' >&2; exit 1; }
  curl -fsSL "https://nodejs.org/dist/latest-v22.x/$node_archive" -o "$release_dir/$node_archive"
  (cd "$release_dir"; awk -v archive="$node_archive" '$2 == archive' SHASUMS256.txt | sha256sum --check -)
  tar -xJf "$release_dir/$node_archive" -C "$node_dir" --strip-components=1
  node="$node_dir/bin/node"
fi
export PATH="$(dirname "$node"):$PATH"
cd "$release_dir"
npm ci --workspace=apps/server --workspace=packages/contracts --workspace=packages/shared --include-workspace-root --no-audit --no-fund
npm run build:contracts
npm run build:shared
npm run --workspace=apps/server build

# Store secrets in the WSL user's private directory, never in a task's command line.
"$release_dir/bin/gah" installer wsl-worker --settings "$stage/settings.json" --root "$install_dir" --release "$release_dir" --node "$node"
# Persist role beside profiles. Do not pin it in worker.env: re-flagging config survives restart.
central_url="$("$release_dir/bin/gah" installer json --file "$stage/settings.json" --pointer /central_url)"
"$release_dir/bin/gah" config set --node-role worker --registry-central-url "$central_url" --config "$HOME/.config/gah/config.toml"
systemctl --user daemon-reload
systemctl --user enable gah-worker.service
systemctl --user restart gah-worker.service
systemctl --user is-active --quiet gah-worker.service
printf '\nWorker service installed. Backend logins and repository profiles still need setup.\n'
