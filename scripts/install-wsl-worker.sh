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
sudo apt-get update
sudo apt-get install -y ca-certificates curl git python3 xz-utils build-essential
install_dir="$HOME/.local/share/gah/worker"
mkdir -p "$install_dir"
chmod 700 "$install_dir"
# Each build has its own directory so a failed upgrade leaves the running service intact.
release_dir="$(mktemp -d "$install_dir/release.XXXXXXXX")"
tar -xzf "$stage/source.tar.gz" -C "$release_dir"
mkdir -p "$release_dir/bin"
install -m 755 "$stage/gah" "$release_dir/bin/gah"
"$release_dir/bin/gah" --version
# role-cli-check:start -- also exercised without installing a service.
if ! "$release_dir/bin/gah" config set --help | grep -q -- '--node-role' ||
   ! "$release_dir/bin/gah" status --help | grep -q -- '--role'; then
  echo 'The downloaded GAH CLI is too old for this worker installer. Publish/install a matching CLI release with config --node-role and status --role support. The existing worker service and settings have not changed.' >&2
  exit 1
fi
# role-cli-check:end
if ! command -v node >/dev/null || [ "$(node -p 'process.versions.node.split(".")[0]')" -lt 20 ]; then
  node_dir="$install_dir/node"
  mkdir -p "$node_dir"
  curl -fsSL https://nodejs.org/dist/latest-v22.x/SHASUMS256.txt -o "$release_dir/SHASUMS256.txt"
  node_archive="$(awk '$2 ~ /^node-v22\.[0-9]+\.[0-9]+-linux-x64.tar.xz$/ { print $2; exit }' "$release_dir/SHASUMS256.txt")"
  [ -n "$node_archive" ] || { echo 'Cannot locate a Node 22 Linux release.' >&2; exit 1; }
  curl -fsSL "https://nodejs.org/dist/latest-v22.x/$node_archive" -o "$release_dir/$node_archive"
  (cd "$release_dir"; awk -v archive="$node_archive" '$2 == archive' SHASUMS256.txt | sha256sum --check -)
  tar -xJf "$release_dir/$node_archive" -C "$node_dir" --strip-components=1
  export PATH="$node_dir/bin:$PATH"
fi
cd "$release_dir"
npm ci --workspace=apps/server --workspace=packages/contracts --workspace=packages/shared --include-workspace-root --no-audit --no-fund
npm run build:contracts
npm run build:shared
npm run --workspace=apps/server build

# Store secrets in the WSL user's private directory, never in a task's command line.
python3 - "$stage/settings.json" "$install_dir" "$release_dir" "$(command -v node)" <<'PY'
import json, os, pathlib, shlex, sys, uuid
settings = json.loads(pathlib.Path(sys.argv[1]).read_text())
root, release, node = map(pathlib.Path, sys.argv[2:])
config = pathlib.Path.home() / '.config/gah/config.toml'
config.parent.mkdir(parents=True, exist_ok=True)
if not config.exists():
    config.write_text('[defaults]\n\n[profiles]\n')
identity_path = root / 'identity.json'
identity = json.loads(identity_path.read_text()) if identity_path.exists() else {'node_id': str(uuid.uuid4())}
identity.update(display_name=settings['display_name'], advertised_url=settings['advertised_url'])
identity_path.write_text(json.dumps(identity))
env = {
    'COORDINATOR_TOKEN': settings['token'], 'GAH_ALLOW_INSECURE_HTTP': '1',
    'GAH_COORDINATOR_IDENTITY_PATH': str(identity_path),
    'GAH_CONFIG_PATH': str(config), 'GAH_CONFIG': str(config),
    'GAH_BINARY': str(release / 'bin/gah'), 'HOST': '0.0.0.0', 'PORT': '3774',
}
env_path = root / 'worker.env'
env_path.write_text(''.join(f'export {k}={shlex.quote(v)}\n' for k, v in env.items()) + 'export PATH=' + shlex.quote(str(release / 'bin') + ':' + str(node.parent) + ':') + '"$PATH"\n')
env_path.chmod(0o600)
start = root / 'start.sh'
start.write_text('#!/usr/bin/env bash\nset -euo pipefail\nsource ' + shlex.quote(str(env_path)) + '\ncd ' + shlex.quote(str(release)) + '\nexec ' + shlex.quote(str(node)) + ' apps/server/dist/bin.js\n')
start.chmod(0o700)
register = root / 'register.sh'
register.write_text('#!/usr/bin/env bash\nset -euo pipefail\nsource ' + shlex.quote(str(env_path)) + '\ncd ' + shlex.quote(str(release)) + '''
for attempt in {1..30}; do
  if curl -fsS http://127.0.0.1:3774/health >/dev/null; then break; fi
  sleep 1
done
exec ''' + shlex.quote(str(node)) + ' apps/server/dist/registerNodeCli.js --central-url ' + shlex.quote(settings['central_url']) + ' --self-url http://127.0.0.1:3774 --transport-mode trusted_lan --secret-ref env:COORDINATOR_TOKEN --labels windows,wsl --profiles "${1:-}"\n')
register.chmod(0o700)
# systemd quoting uses double quotes and expands percent specifiers, unlike shell quoting.
unit_path = pathlib.Path.home() / '.config/systemd/user/gah-worker.service'
unit_path.parent.mkdir(parents=True, exist_ok=True)
quoted_start = str(start).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%')
unit_path.write_text('[Unit]\nDescription=GAH headless WSL worker\nAfter=network-online.target\n\n[Service]\nExecStart=/bin/bash --login "' + quoted_start + '"\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n')
PY
# Persist role beside profiles. Do not pin it in worker.env: re-flagging config survives restart.
central_url="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["central_url"])' "$stage/settings.json")"
"$release_dir/bin/gah" config set --node-role worker --registry-central-url "$central_url" --config "$HOME/.config/gah/config.toml"
systemctl --user daemon-reload
systemctl --user enable gah-worker.service
systemctl --user restart gah-worker.service
systemctl --user is-active --quiet gah-worker.service
printf '\nWorker service installed. Backend logins and repository profiles still need setup.\n'
