#!/usr/bin/env bash
# Deterministic first install for a GAH central or worker host on macOS.
#
# macOS has no systemd, so this is a genuinely separate script from
# scripts/install-linux.sh, not that script with the systemd calls patched
# out. Consequences that follow from that:
#
#   - launchd owns the role-appropriate service. Central builds and starts the
#     API/shared web control surface; worker installs a stopped loop agent that
#     the desktop app can start after a profile is configured.
#   - No sudo, no /etc -- everything lives under $HOME.
#   - Coordinator credentials remain in ~/.config/gah/gah-loop.env; the worker
#     LaunchAgent sources that file without copying secrets into its plist.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "ERROR: scripts/install-macos.sh is for macOS only (uname -s reported $(uname -s)). Use scripts/install-linux.sh, or scripts/install.sh to auto-detect." >&2
  exit 1
fi

role="${GAH_NODE_ROLE:-worker}"
case "$role" in central|worker) ;; *) echo "ERROR: unknown GAH_NODE_ROLE='$role' (expected 'central' or 'worker')" >&2; exit 1 ;; esac

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ "$role" = central ]; then
  case "${GAH_GATEWAY_MODE:-}" in
    colocated)
      : "${GAH_GATEWAY_MEMORYCORE_PATH:?GAH_GATEWAY_MODE=colocated requires GAH_GATEWAY_MEMORYCORE_PATH}"
      : "${GAH_GATEWAY_LLM_API_KEY:?GAH_GATEWAY_MODE=colocated requires GAH_GATEWAY_LLM_API_KEY}"
      [ -f "$GAH_GATEWAY_MEMORYCORE_PATH/src/gateway/server.ts" ] || { echo 'ERROR: GAH_GATEWAY_MEMORYCORE_PATH is not a MemoryCore checkout.' >&2; exit 1; }
      gateway_config="$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.local.yaml"
      if [ ! -f "$gateway_config" ]; then
        cp "$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.standalone.yaml" "$gateway_config"
      fi
      gateway_api_key="${GAH_GATEWAY_API_KEY:-$(openssl rand -hex 24)}"
      export GAH_MACOS_GATEWAY_URL=http://127.0.0.1:8420 GAH_MACOS_GATEWAY_KEY="$gateway_api_key"
      python3 - <<'PY'
import os
import tempfile
from pathlib import Path
for value in [os.environ['GAH_MACOS_GATEWAY_KEY'], os.environ['GAH_GATEWAY_LLM_API_KEY']]:
    if not value.strip() or any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise SystemExit('ERROR: gateway credentials must not contain control characters')
path = Path.home() / '.config/gah/tdai-gateway.env'
path.parent.mkdir(parents=True, exist_ok=True)
def quoted(value):
    for character in ['\\', '"', '$', '`']:
        value = value.replace(character, '\\' + character)
    return '"' + value + '"'
fd, temporary = tempfile.mkstemp(dir=path.parent)
try:
    with os.fdopen(fd, 'w') as output:
        output.write('TDAI_GATEWAY_API_KEY=' + quoted(os.environ['GAH_MACOS_GATEWAY_KEY']) + '\n')
        output.write('TDAI_LLM_API_KEY=' + quoted(os.environ['GAH_GATEWAY_LLM_API_KEY']) + '\n')
    os.replace(temporary, path)
finally:
    if os.path.exists(temporary): os.unlink(temporary)
PY
      ;;
    remote)
      : "${GAH_GATEWAY_URL:?GAH_GATEWAY_MODE=remote requires GAH_GATEWAY_URL on macOS}"
      export GAH_MACOS_GATEWAY_URL="$GAH_GATEWAY_URL" GAH_MACOS_GATEWAY_KEY="${GAH_GATEWAY_API_KEY:-}"
      auth_header=()
      [ -z "$GAH_MACOS_GATEWAY_KEY" ] || auth_header=(-H "Authorization: Bearer $GAH_MACOS_GATEWAY_KEY")
      curl -fsS -m 10 "${auth_header[@]}" -H 'Content-Type: application/json' -d '{"query":"gah install reachability check","session_key":"gah:install-check"}' "$GAH_GATEWAY_URL/recall" >/dev/null
      ;;
    "") ;;
    *) echo "ERROR: unknown GAH_GATEWAY_MODE='$GAH_GATEWAY_MODE'" >&2; exit 1 ;;
  esac
  if [ -n "${GAH_MACOS_GATEWAY_URL:-}" ]; then
    python3 - <<'PY'
import os
import tempfile
from pathlib import Path
path = Path.home() / '.config/gah/server.env'
path.parent.mkdir(parents=True, exist_ok=True)
lines = path.read_text().splitlines() if path.exists() else []
values = {'TDAI_GATEWAY_URL': os.environ['GAH_MACOS_GATEWAY_URL']}
if os.environ.get('GAH_MACOS_GATEWAY_KEY'): values['TDAI_GATEWAY_API_KEY'] = os.environ['GAH_MACOS_GATEWAY_KEY']
for key, value in values.items():
    if any(ord(char) < 32 or ord(char) == 127 for char in value): raise SystemExit('ERROR: gateway settings must not contain control characters')
    for character in ['\\', '"', '$', '`']: value = value.replace(character, '\\' + character)
    lines = [line for line in lines if not line.startswith(key + '=')]
    lines.append(key + '="' + value + '"')
fd, temporary = tempfile.mkstemp(dir=path.parent)
try:
    with os.fdopen(fd, 'w') as output: output.write('\n'.join(lines) + '\n')
    os.replace(temporary, path)
finally:
    if os.path.exists(temporary): os.unlink(temporary)
PY
  fi
elif [ -n "${GAH_GATEWAY_MODE:-}" ]; then
  echo 'ERROR: configure the TDAI gateway on a central node, not a worker.' >&2
  exit 1
fi

# --bin gah is required: Cargo.toml declares a second [[bin]]
# (generate-cli-capabilities) with no default-run set, so a bare `cargo run`
# is ambiguous and errors instead of picking one.
bash "$repo_root/scripts/configure-node-role.sh" "$role" cargo run --bin gah --
cargo run --bin gah -- update --repo "$repo_root" --role "$role"

if [ "$role" = central ] && [ "${GAH_GATEWAY_MODE:-}" = colocated ]; then
  gateway_ready=0
  for _ in $(seq 1 15); do
    if curl -fsS -m 2 -H "Authorization: Bearer $GAH_MACOS_GATEWAY_KEY" http://127.0.0.1:8420/health >/dev/null 2>&1; then
      gateway_ready=1
      break
    fi
    sleep 2
  done
  [ "$gateway_ready" = 1 ] || { echo 'ERROR: the macOS memory-gateway LaunchAgent did not become healthy. Read ~/.local/state/gah/memory-gateway.log.' >&2; exit 1; }
fi

if command -v tailscale >/dev/null 2>&1; then
  tailscale set --accept-dns=true
else
  echo 'WARNING: Tailscale is not installed; join the tailnet, then enable Use Tailscale DNS settings.' >&2
fi

gateway_env_file="$HOME/.config/gah/gah-loop.env"

echo "GAH installed. Update with: gah update --repo $repo_root --role $role"
if [ "$role" = worker ] && [ -f "$gateway_env_file" ]; then
  echo "Worker credentials are in $gateway_env_file; launchd loads them when the desktop starts the worker."
fi
