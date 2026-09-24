#!/usr/bin/env bash
# Real first install on a disposable GitHub-hosted macOS account, never a developer's Mac.
set -euo pipefail
if [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ] || [ "$(uname -s)" != Darwin ]; then
  echo 'ERROR: this test installs GAH and may only run on a GitHub-hosted macOS runner.' >&2
  exit 1
fi
if [ -e "$HOME/.config/gah" ] || [ -e "${CARGO_HOME:-$HOME/.cargo}/bin/gah" ]; then
  echo 'ERROR: expected a fresh runner without an existing GAH installation.' >&2
  exit 1
fi

# update requires a clean default-branch checkout and performs a real fetch/pull.
# A local origin pins those operations to this PR, instead of installing main.
stage="$(mktemp -d "$RUNNER_TEMP/gah-install.XXXXXX")"
git init --bare --initial-branch=main "$stage/origin.git"
git -C "$stage/origin.git" config receive.shallowUpdate true
git push "$stage/origin.git" HEAD:refs/heads/main
git clone "$stage/origin.git" "$stage/checkout"
registration_file="$stage/registration.json"
health_file="$stage/health-checks"
health_mode="$stage/health-mode"
export GAH_CENTRAL_URL=http://127.0.0.1:47819
python3 - "$registration_file" "$health_file" "$health_mode" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/':
            self.send_response(200)
            self.end_headers()
            return
        if not self.path.startswith('/api/registry/nodes/') or not self.path.endswith('/health') or self.headers.get('Authorization') != 'Bearer ci-install-test-token':
            self.send_error(401)
            return
        health = Path(sys.argv[2])
        health.write_text(health.read_text() + '1\n' if health.exists() else '1\n')
        unhealthy = Path(sys.argv[3]).exists()
        body = json.dumps({
            'status': 'unhealthy' if unhealthy else 'healthy',
            'state': 'unreachable' if unhealthy else 'healthy',
            **({'error': {'kind': 'NETWORK', 'message': 'reverse tunnel is unavailable'}} if unhealthy else {})
        }).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if self.path != '/api/registry/nodes' or self.headers.get('Authorization') != 'Bearer ci-install-test-token':
            self.send_error(401)
            return
        Path(sys.argv[1]).write_bytes(self.rfile.read(int(self.headers['Content-Length'])))
        body = json.dumps({'warnings': []}).encode()
        self.send_response(201)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass

ThreadingHTTPServer(('127.0.0.1', 47819), Handler).serve_forever()
PY
central_pid=$!
trap 'launchctl bootout "gui/$(id -u)/dev.git-agent-harness.worker-tunnel" 2>/dev/null || true; kill "$central_pid" 2>/dev/null || true' EXIT
for _ in {1..50}; do /usr/bin/curl -s -o /dev/null "$GAH_CENTRAL_URL" && break; sleep 0.1; done
/usr/bin/curl -s -o /dev/null "$GAH_CENTRAL_URL" || { echo 'ERROR: fake central did not start' >&2; exit 1; }
export CARGO_TARGET_DIR="$GITHUB_WORKSPACE/target"
export REGISTRATION_FILE="$registration_file"
export HEALTH_FILE="$health_file"
export GAH_NODE_ROLE=worker
export COORDINATOR_TOKEN=ci-install-test-token
# The hosted runner has no tailnet. Production workers keep the Tailscale default.
export GAH_DESKTOP_SERVER_PORT=4774
export GAH_NODE_SSH_TARGET=127.0.0.1
export GAH_NODE_SSH_REMOTE_PORT=48774
bash "$stage/checkout/scripts/install-macos.sh"
launchctl print "gui/$(id -u)/dev.git-agent-harness.worker-tunnel" >/dev/null

unset GAH_DESKTOP_SERVER_PORT GAH_NODE_SSH_TARGET GAH_NODE_SSH_REMOTE_PORT
bash "$stage/checkout/scripts/macos-launchd.sh" install worker "$stage/checkout"
launchctl print "gui/$(id -u)/dev.git-agent-harness.worker-tunnel" >/dev/null

"${CARGO_HOME:-$HOME/.cargo}/bin/gah" --help >/dev/null
python3 - <<'PY'
import os
from pathlib import Path
import tomllib

config_root = Path.home() / '.config'
config = tomllib.loads((config_root / 'gah/config.toml').read_text())['defaults']
assert config['node_role'] == 'worker', config
assert config['registry_central_url'] == os.environ['GAH_CENTRAL_URL'], config
credentials = config_root / 'gah/gah-loop.env'
assert credentials.stat().st_mode & 0o777 == 0o600
assert credentials.read_text() == 'COORDINATOR_TOKEN="ci-install-test-token"\n'
for name in ['gah-reviewer.md', 'gah-implementer.md']:
    assert (config_root / 'opencode/agents' / name).is_file(), name
assert not (config_root / 'systemd').exists(), 'macOS must not install systemd units'
assert not Path('/etc/gah/server.env').exists(), 'a worker must not configure a central server'
assert (Path.home() / 'Applications/GAH.app').is_dir(), 'the deterministic update must install the desktop app'
desktop = __import__('json').loads((config_root / 'gah/desktop.json').read_text())
assert desktop['repository_path'].endswith('/checkout'), desktop
assert desktop['server_port'] == 4774, desktop
registration = __import__('json').loads(Path(os.environ['REGISTRATION_FILE']).read_text())
assert registration['advertised_url'] == 'http://127.0.0.1:48774', registration
assert registration['transport_mode'] == 'loopback', registration
assert Path(os.environ['HEALTH_FILE']).read_text().count('1\n') >= 2
print('Real macOS install passed: executable, worker role, relay URL, private credentials, agent configs, and launchd checkout state.')
PY

touch "$health_mode"
if bash "$stage/checkout/scripts/macos-launchd.sh" start worker >"$stage/unreachable.out" 2>"$stage/unreachable.err"; then
  echo 'ERROR: worker start succeeded while central reported the advertised URL unreachable.' >&2
  exit 1
fi
grep -F 'central registered this worker but cannot reach its advertised URL: reverse tunnel is unavailable' "$stage/unreachable.err"
