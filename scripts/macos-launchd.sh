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

case "$action" in
  status)
    run_launchctl print "$domain/$label" >/dev/null 2>&1
    exit $?
    ;;
  start)
    run_launchctl kickstart -k "$domain/$label"
    exit
    ;;
  stop)
    run_launchctl kill SIGTERM "$domain/$label" >/dev/null 2>&1 || true
    exit
    ;;
  install) ;;
  *) echo "ERROR: invalid action '$action'" >&2; exit 1 ;;
esac

repo="${3:?Install requires the GAH repository path}"
profile="${4:-}"
repo="$(cd "$repo" && pwd -P)"
[ -f "$repo/Cargo.toml" ] || { echo "ERROR: $repo is not a GAH checkout" >&2; exit 1; }
if [ "$role" = worker ] && [ -z "$profile" ]; then
  echo 'No profile is configured; recording the checkout without installing a worker service.'
fi

mkdir -p "$agents_dir" "$HOME/.local/state/gah" "$HOME/.cache/gah/tmp" "$HOME/.config/gah"
node_path="${GAH_NODE_PATH:-$(command -v node || true)}"
gah_path="${GAH_CLI_PATH:-$(command -v gah || true)}"
if [ "$role" = central ] && [ -z "$node_path" ]; then echo 'ERROR: node is required for central mode' >&2; exit 1; fi
if [ "$role" = worker ] && [ -n "$profile" ] && [ -z "$gah_path" ]; then echo 'ERROR: gah is required for worker mode' >&2; exit 1; fi

export GAH_LAUNCHD_ACTION="$action" GAH_LAUNCHD_ROLE="$role" GAH_LAUNCHD_REPO="$repo"
export GAH_LAUNCHD_PROFILE="$profile" GAH_LAUNCHD_PLIST="$plist" GAH_LAUNCHD_LABEL="$label"
export GAH_LAUNCHD_NODE="$node_path" GAH_LAUNCHD_GAH="$gah_path"
export GAH_LAUNCHD_PORT="${GAH_DESKTOP_SERVER_PORT:-3774}"
export GAH_LAUNCHD_NPX="${GAH_NPX_PATH:-$(command -v npx || true)}"
python3 - <<'PY'
import json, os, pathlib, plistlib, tempfile

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
for value in [str(repo), profile]:
    if any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise SystemExit('ERROR: launchd values must not contain control characters')

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

if role == 'worker' and not profile:
    plist_path.unlink(missing_ok=True)
    raise SystemExit(0)

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
    source = 'set -a; [ ! -f "$HOME/.config/gah/gah-loop.env" ] || . "$HOME/.config/gah/gah-loop.env"; set +a; exec "$0" loop --profile "$1"'
    common.update(
        ProgramArguments=['/bin/bash', '-lc', source, os.environ['GAH_LAUNCHD_GAH'], profile],
        EnvironmentVariables={
            'HOME': str(home), 'PATH': path, 'GAH_CONFIG': str(home / '.config/gah/config.toml'),
            'XDG_STATE_HOME': str(home / '.local/state'), 'TMPDIR': str(home / '.cache/gah/tmp'),
        },
        RunAtLoad=False,
        KeepAlive=False,
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
run_launchctl bootout "$domain/$other" >/dev/null 2>&1 || true
rm -f "$agents_dir/$other.plist"
if [ -f "$plist" ]; then
  run_launchctl bootout "$domain/$label" >/dev/null 2>&1 || true
  run_launchctl bootstrap "$domain" "$plist"
fi
gateway_plist="$agents_dir/$gateway_label.plist"
if [ "$role" = worker ]; then
  run_launchctl bootout "$domain/$gateway_label" >/dev/null 2>&1 || true
elif [ -f "$gateway_plist" ]; then
  run_launchctl bootout "$domain/$gateway_label" >/dev/null 2>&1 || true
  run_launchctl bootstrap "$domain" "$gateway_plist"
fi
