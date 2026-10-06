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

# Confirm the entire installer before persisting role, credentials, or gateway
# settings. The update below reuses this approval rather than prompting again.
if [ -z "${GAH_INSTALL_CONFIRMED:-}" ]; then
  echo "Install GAH for role '$role' from $repo_root; configure node settings, credentials, and role services."
  echo "Gateway mode: ${GAH_GATEWAY_MODE:-none}; agent assets: ${GAH_INSTALL_AGENT:-none}."
  printf 'Apply these changes? [y/N] '
  answer=""
  read -r answer || true
  case "$answer" in
    y|Y|yes) ;;
    *) echo 'Installation cancelled before configuration or installation' >&2; exit 1 ;;
  esac
fi

# --bin gah is required: Cargo.toml declares a second [[bin]]
# (generate-cli-capabilities) with no default-run set, so a bare `cargo run`
# is ambiguous and errors instead of picking one. The first call builds gah.
gah_cli=(cargo run --locked -q --bin gah --)

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
      # Values travel on stdin; the file is written with mode 0600.
      gateway_env="$HOME/.config/gah/tdai-gateway.env"
      printf '%s' "$GAH_MACOS_GATEWAY_KEY" | "${gah_cli[@]}" installer env-set --file "$gateway_env" TDAI_GATEWAY_API_KEY
      printf '%s' "$GAH_GATEWAY_LLM_API_KEY" | "${gah_cli[@]}" installer env-set --file "$gateway_env" TDAI_LLM_API_KEY
      ;;
    remote)
      : "${GAH_GATEWAY_URL:?GAH_GATEWAY_MODE=remote requires GAH_GATEWAY_URL on macOS}"
      export GAH_MACOS_GATEWAY_URL="$GAH_GATEWAY_URL" GAH_MACOS_GATEWAY_KEY="${GAH_GATEWAY_API_KEY:-}"
      auth_header=()
      [ -z "$GAH_MACOS_GATEWAY_KEY" ] || auth_header=(-H "Authorization: Bearer $GAH_MACOS_GATEWAY_KEY")
      curl -fsS -m 10 "${auth_header[@]}" -H 'Content-Type: application/json' -d '{"query":"gah install reachability check","session_key":"gah:install-check"}' "$GAH_GATEWAY_URL/recall" >/dev/null
      [ -n "$GAH_MACOS_GATEWAY_KEY" ] || { echo 'ERROR: set GAH_GATEWAY_API_KEY before remote memory setup.' >&2; exit 1; }
      printf '%s' "$GAH_MACOS_GATEWAY_KEY" | "${gah_cli[@]}" installer env-set --file "$HOME/.config/gah/tdai-gateway.env" TDAI_GATEWAY_API_KEY
      ;;
    "") ;;
    *) echo "ERROR: unknown GAH_GATEWAY_MODE='$GAH_GATEWAY_MODE'" >&2; exit 1 ;;
  esac
  if [ -n "${GAH_MACOS_GATEWAY_URL:-}" ]; then
    printf '%s' "$GAH_MACOS_GATEWAY_URL" | "${gah_cli[@]}" installer env-set --file "$HOME/.config/gah/server.env" TDAI_GATEWAY_URL
  fi
elif [ -n "${GAH_GATEWAY_MODE:-}" ]; then
  echo 'ERROR: configure the TDAI gateway on a central node, not a worker.' >&2
  exit 1
fi

bash "$repo_root/scripts/configure-node-role.sh" "$role" "${gah_cli[@]}"
cargo run --locked --bin gah -- update --repo "$repo_root" --role "$role" --yes ${GAH_INSTALL_AGENT:+--agent "$GAH_INSTALL_AGENT"}

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

echo "GAH installed. Update with: gah update --pull --repo $repo_root --role $role"
if [ "$role" = worker ] && [ -f "$gateway_env_file" ]; then
  echo "Worker credentials are in $gateway_env_file; launchd loads them when the desktop starts the worker."
fi
