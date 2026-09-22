#!/usr/bin/env bash
# Own the macOS control-plane/worker lifecycle from one deterministic place.
set -euo pipefail

action="${1:?Usage: macos-launchd.sh install|start|stop|status central|worker [repo] [profile]}"
role="${2:?Missing node role}"
case "$role" in central|worker) ;; *) echo "ERROR: invalid node role '$role'" >&2; exit 1 ;; esac

server_label=dev.git-agent-harness.server
worker_label=dev.git-agent-harness.worker
gateway_label=dev.git-agent-harness.memory-gateway
label="$server_label"
[ "$role" = worker ] && label="$worker_label"
domain="gui/${GAH_LAUNCHD_UID:-$(id -u)}"
agents_dir="${GAH_LAUNCH_AGENTS_DIR:-$HOME/Library/LaunchAgents}"
plist="$agents_dir/$label.plist"

run_launchctl() {
  if [ "${GAH_LAUNCHD_DRY_RUN:-}" != 1 ]; then launchctl "$@"; fi
}

bootstrap_agent() {
  if [ "${GAH_LAUNCHD_DRY_RUN:-}" = 1 ]; then return; fi
  for attempt in 1 2 3; do
    launchctl bootstrap "$domain" "$1" && return
    [ "$attempt" = 3 ] || sleep 1
  done
  return 1
}

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :EnvironmentVariables:$1" "$plist"
}

register_worker() {
  [ "${GAH_LAUNCHD_DRY_RUN:-}" = 1 ] && return
  gah_path="$(plist_value GAH_BINARY)"
  identity_path="$(plist_value GAH_COORDINATOR_IDENTITY_PATH)"
  host="$(plist_value HOST)"
  port="$(plist_value PORT)"
  transport_mode="$(plist_value GAH_REGISTRY_TRANSPORT_MODE)"
  for _ in $(seq 1 20); do
    if /usr/bin/curl -fsS -m 2 "http://$host:$port/health" >/dev/null 2>&1; then
      set -a
      # shellcheck disable=SC1090
      [ ! -f "$HOME/.config/gah/gah-loop.env" ] || . "$HOME/.config/gah/gah-loop.env"
      set +a
      GAH_COORDINATOR_IDENTITY_PATH="$identity_path" "$gah_path" node register --transport-mode "$transport_mode"
      return
    fi
    sleep 1
  done
  echo "ERROR: the macOS worker did not become healthy at http://$host:$port" >&2
  return 1
}

configure_worker_transport() {
  [ "${GAH_LAUNCHD_DRY_RUN:-}" = 1 ] && return
  [ "$(plist_value GAH_TAILSCALE_SERVE)" = 1 ] || return
  tailscale_path="$(plist_value GAH_TAILSCALE_CLI)"
  port="$(plist_value PORT)"
  "$tailscale_path" serve --bg --yes --https="$port" "http://127.0.0.1:$port" >/dev/null
}

disable_worker_transport() {
  [ "${GAH_LAUNCHD_DRY_RUN:-}" = 1 ] && return
  worker_plist="$agents_dir/$worker_label.plist"
  [ -f "$worker_plist" ] || return
  [ "$(/usr/libexec/PlistBuddy -c 'Print :EnvironmentVariables:GAH_TAILSCALE_SERVE' "$worker_plist" 2>/dev/null || true)" = 1 ] || return
  tailscale_path="$(/usr/libexec/PlistBuddy -c 'Print :EnvironmentVariables:GAH_TAILSCALE_CLI' "$worker_plist")"
  port="$(/usr/libexec/PlistBuddy -c 'Print :EnvironmentVariables:PORT' "$worker_plist")"
  "$tailscale_path" serve --https="$port" off >/dev/null 2>&1 || true
}

case "$action" in
  status)
    run_launchctl print "$domain/$label" >/dev/null 2>&1
    exit $?
    ;;
  start)
    [ "$role" != worker ] || configure_worker_transport
    run_launchctl kickstart -k "$domain/$label"
    [ "$role" != worker ] || register_worker
    exit
    ;;
  stop)
    [ "$role" != worker ] || disable_worker_transport
    run_launchctl bootout "$domain/$label" >/dev/null 2>&1 || true
    exit
    ;;
  install) ;;
  *) echo "ERROR: invalid action '$action'" >&2; exit 1 ;;
esac

repo="${3:?Install requires the GAH repository path}"
profile="${4:-}"
repo="$(cd "$repo" && pwd -P)"
[ -f "$repo/Cargo.toml" ] || { echo "ERROR: $repo is not a GAH checkout" >&2; exit 1; }

mkdir -p "$agents_dir" "$HOME/.local/state/gah" "$HOME/.cache/gah/tmp" "$HOME/.config/gah"
node_path="${GAH_NODE_PATH:-$(command -v node || true)}"
gah_path="${GAH_CLI_PATH:-$(command -v gah || true)}"
if [ -z "$node_path" ]; then echo "ERROR: node is required for $role mode" >&2; exit 1; fi
if [ "$role" = worker ] && [ -z "$gah_path" ]; then echo 'ERROR: gah is required for worker mode' >&2; exit 1; fi

port="${GAH_DESKTOP_SERVER_PORT:-3774}"
advertised_url="${GAH_NODE_ADVERTISED_URL:-}"
tailscale_path="${GAH_TAILSCALE_PATH:-$(command -v tailscale || true)}"
if [ -z "$tailscale_path" ] && [ -x /Applications/Tailscale.app/Contents/MacOS/Tailscale ]; then
  tailscale_path=/Applications/Tailscale.app/Contents/MacOS/Tailscale
fi
if [ "$role" = worker ] && [ -z "$advertised_url" ]; then
  [ -n "$tailscale_path" ] || { echo 'ERROR: Tailscale is required for a macOS worker.' >&2; exit 1; }
  tailscale_dns="$($tailscale_path status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')"
  [ -n "$tailscale_dns" ] || { echo 'ERROR: cannot find this Mac tailnet DNS name.' >&2; exit 1; }
  advertised_url="https://$tailscale_dns:$port"
fi

export GAH_LAUNCHD_ACTION="$action" GAH_LAUNCHD_ROLE="$role" GAH_LAUNCHD_REPO="$repo"
export GAH_LAUNCHD_PROFILE="$profile" GAH_LAUNCHD_PLIST="$plist" GAH_LAUNCHD_LABEL="$label"
export GAH_LAUNCHD_NODE="$node_path" GAH_LAUNCHD_GAH="$gah_path"
export GAH_LAUNCHD_PORT="$port" GAH_LAUNCHD_ADVERTISED_URL="$advertised_url"
export GAH_LAUNCHD_TAILSCALE="$tailscale_path"
export GAH_LAUNCHD_NPX="${GAH_NPX_PATH:-$(command -v npx || true)}"
python3 - <<'PY'
import ipaddress, json, os, pathlib, plistlib, socket, tempfile, urllib.parse, uuid

def replace_file(path, data, mode):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as output:
            output.write(data)
        os.chmod(temporary, mode)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary): os.unlink(temporary)

home = pathlib.Path.home()
repo = pathlib.Path(os.environ['GAH_LAUNCHD_REPO'])
role = os.environ['GAH_LAUNCHD_ROLE']
profile = os.environ['GAH_LAUNCHD_PROFILE']
label = os.environ['GAH_LAUNCHD_LABEL']
plist_path = pathlib.Path(os.environ['GAH_LAUNCHD_PLIST'])
port = os.environ['GAH_LAUNCHD_PORT']
if not port.isdigit() or not 1024 <= int(port) <= 65535:
    raise SystemExit('ERROR: GAH_DESKTOP_SERVER_PORT must be between 1024 and 65535')
for value in [str(repo), profile, os.environ['GAH_LAUNCHD_ADVERTISED_URL']]:
    if any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise SystemExit('ERROR: launchd values must not contain control characters')

worker_identity = home / '.local/share/gah/worker/identity.json'
worker_host = ''
worker_transport = ''
tailscale_serve = False
if role == 'worker':
    advertised_url = os.environ['GAH_LAUNCHD_ADVERTISED_URL']
    parsed = urllib.parse.urlsplit(advertised_url)
    if parsed.scheme not in ('http', 'https') or parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path not in ('', '/'):
        raise SystemExit('ERROR: GAH_NODE_ADVERTISED_URL must be an HTTP(S) origin without credentials or a path')
    if parsed.port != int(port):
        raise SystemExit('ERROR: GAH_NODE_ADVERTISED_URL must use GAH_DESKTOP_SERVER_PORT')
    if parsed.scheme == 'https':
        if not os.environ['GAH_LAUNCHD_TAILSCALE']:
            raise SystemExit('ERROR: Tailscale is required for an HTTPS macOS worker')
        worker_host = '127.0.0.1'
        worker_transport = 'authenticated_remote'
        tailscale_serve = True
    else:
        try:
            worker_host = str(ipaddress.IPv4Address(parsed.hostname or ''))
        except ipaddress.AddressValueError:
            raise SystemExit('ERROR: an HTTP GAH_NODE_ADVERTISED_URL must use this Mac IPv4 address')
        worker_transport = 'trusted_lan'
    legacy_identity = repo / 'config/coordinator-identity.json'
    identity_source = worker_identity if worker_identity.exists() else legacy_identity
    try:
        identity = json.loads(identity_source.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        identity = {}
    identity.update(
        node_id=identity.get('node_id') or str(uuid.uuid4()),
        display_name=(identity.get('display_name') if identity.get('display_name') != 'GAH Coordinator' else None) or socket.gethostname(),
        advertised_url=advertised_url,
    )
    replace_file(worker_identity, (json.dumps(identity, indent=2) + '\n').encode(), 0o600)

settings_path = home / '.config/gah/desktop.json'
try:
    settings = json.loads(settings_path.read_text())
except (FileNotFoundError, json.JSONDecodeError):
    settings = {}
gateway_repo = os.environ.get('GAH_GATEWAY_MEMORYCORE_PATH') or settings.get('gateway_repository_path', '')
settings.update(repository_path=str(repo), server_port=int(port))
if os.environ.get('GAH_GATEWAY_MEMORYCORE_PATH'):
    settings['gateway_repository_path'] = str(pathlib.Path(gateway_repo).resolve())
replace_file(settings_path, (json.dumps(settings, indent=2) + '\n').encode(), 0o600)

path = os.environ.get('PATH', '/usr/local/bin:/usr/bin:/bin')
common = {
    'Label': label,
    'WorkingDirectory': str(repo),
    'ProcessType': 'Background',
    'StandardOutPath': str(home / f'.local/state/gah/{role}.log'),
    'StandardErrorPath': str(home / f'.local/state/gah/{role}.log'),
}
if role == 'central':
    server = repo / 'apps/server/dist/bin.js'
    web = repo / 'apps/web/dist/index.html'
    if not server.is_file() or not web.is_file():
        raise SystemExit('ERROR: central build is missing; run gah update --role central first')
    source = 'set -a; [ ! -f "$HOME/.config/gah/server.env" ] || . "$HOME/.config/gah/server.env"; [ ! -f "$HOME/.config/gah/tdai-gateway.env" ] || . "$HOME/.config/gah/tdai-gateway.env"; set +a; exec "$0" "$1"'
    common.update(
        ProgramArguments=['/bin/bash', '-lc', source, os.environ['GAH_LAUNCHD_NODE'], str(server)],
        EnvironmentVariables={
            'HOME': str(home), 'PATH': path, 'NODE_ENV': 'production', 'HOST': '127.0.0.1',
            'PORT': port, 'GAH_CONFIG_PATH': str(home / '.config/gah/config.toml'),
            'GAH_WEB_ROOT': str(repo / 'apps/web/dist'), 'GAH_ENABLE_ADMIN_UPDATE': '1',
        },
        RunAtLoad=True,
        KeepAlive=True,
        ThrottleInterval=5,
    )
else:
    server = repo / 'apps/server/dist/bin.js'
    if not server.is_file():
        raise SystemExit('ERROR: worker build is missing; run gah update --role worker first')
    source = 'set -a; [ ! -f "$HOME/.config/gah/gah-loop.env" ] || . "$HOME/.config/gah/gah-loop.env"; set +a; export GAH_COORDINATOR_IDENTITY_PATH="$2"; exec "$0" "$1"'
    common.update(
        ProgramArguments=['/bin/bash', '-lc', source, os.environ['GAH_LAUNCHD_NODE'], str(server), str(worker_identity)],
        EnvironmentVariables={
            'HOME': str(home), 'PATH': path, 'NODE_ENV': 'production', 'HOST': worker_host,
            'PORT': port, 'GAH_CONFIG': str(home / '.config/gah/config.toml'),
            'GAH_CONFIG_PATH': str(home / '.config/gah/config.toml'),
            'GAH_BINARY': os.environ['GAH_LAUNCHD_GAH'],
            'GAH_COORDINATOR_IDENTITY_PATH': str(worker_identity),
            'GAH_ALLOW_INSECURE_HTTP': '0' if tailscale_serve else '1',
            'GAH_REGISTRY_TRANSPORT_MODE': worker_transport,
            'GAH_TAILSCALE_CLI': os.environ['GAH_LAUNCHD_TAILSCALE'],
            'GAH_TAILSCALE_SERVE': '1' if tailscale_serve else '0',
            'XDG_STATE_HOME': str(home / '.local/state'),
            'TMPDIR': str(home / '.cache/gah/tmp'),
        },
        RunAtLoad=False,
        KeepAlive=True,
        ThrottleInterval=5,
    )

replace_file(plist_path, plistlib.dumps(common, sort_keys=False), 0o644)

gateway_plist = plist_path.parent / 'dev.git-agent-harness.memory-gateway.plist'
if role == 'central' and gateway_repo:
    gateway_repo = pathlib.Path(gateway_repo).resolve()
    gateway_server = gateway_repo / 'src/gateway/server.ts'
    gateway_config = gateway_repo / 'tdai-gateway.local.yaml'
    gateway_env = home / '.config/gah/tdai-gateway.env'
    npx = os.environ['GAH_LAUNCHD_NPX']
    if not gateway_server.is_file() or not gateway_config.is_file() or not gateway_env.is_file() or not npx:
        raise SystemExit('ERROR: the saved TDAI gateway is incomplete; rerun install-macos.sh with GAH_GATEWAY_MODE=colocated')
    gateway = {
        'Label': 'dev.git-agent-harness.memory-gateway',
        'ProgramArguments': ['/bin/bash', '-lc', 'set -a; . "$HOME/.config/gah/tdai-gateway.env"; set +a; exec "$0" tsx src/gateway/server.ts', npx],
        'WorkingDirectory': str(gateway_repo),
        'EnvironmentVariables': {'HOME': str(home), 'PATH': path, 'TDAI_GATEWAY_CONFIG': str(gateway_config)},
        'RunAtLoad': True, 'KeepAlive': True, 'ThrottleInterval': 5, 'ProcessType': 'Background',
        'StandardOutPath': str(home / '.local/state/gah/memory-gateway.log'),
        'StandardErrorPath': str(home / '.local/state/gah/memory-gateway.log'),
    }
    replace_file(gateway_plist, plistlib.dumps(gateway, sort_keys=False), 0o644)
elif role == 'worker':
    gateway_plist.unlink(missing_ok=True)
PY

other="$worker_label"
[ "$role" = worker ] && other="$server_label"
[ "$role" != central ] || disable_worker_transport
run_launchctl bootout "$domain/$other" >/dev/null 2>&1 || true
rm -f "$agents_dir/$other.plist"
if [ -f "$plist" ]; then
  run_launchctl bootout "$domain/$label" >/dev/null 2>&1 || true
  bootstrap_agent "$plist"
fi
gateway_plist="$agents_dir/$gateway_label.plist"
if [ "$role" = worker ]; then
  run_launchctl bootout "$domain/$gateway_label" >/dev/null 2>&1 || true
elif [ -f "$gateway_plist" ]; then
  run_launchctl bootout "$domain/$gateway_label" >/dev/null 2>&1 || true
  bootstrap_agent "$gateway_plist"
fi
