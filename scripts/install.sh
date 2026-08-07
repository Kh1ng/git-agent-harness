#!/usr/bin/env bash
# Deterministic first install for a GAH CLI/control-plane host.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Fresh installs and routine upgrades use the same Rust update implementation.
cargo run -- update --repo "$repo_root"

# Persistent server bind-host override (issue #643). Created only on first
# install; every later run of this script, and every `gah update
# --restart-server`, leaves an existing file untouched so an operator's HOST
# choice survives reinstall/update. Set GAH_SERVER_HOST=127.0.0.1 (or another
# interface address) before running this script on first install to seed it;
# 0.0.0.0 remains the application default when unset.
server_env_file=/etc/gah/server.env
if [ ! -f "$server_env_file" ]; then
  sudo install -d -m 0755 /etc/gah
  if [ -n "${GAH_SERVER_HOST:-}" ]; then
    printf 'HOST=%s\n' "$GAH_SERVER_HOST" | sudo tee "$server_env_file" >/dev/null
    sudo chmod 0644 "$server_env_file"
  else
    sudo install -m 0644 /dev/null "$server_env_file"
  fi
  echo "Created $server_env_file (set HOST= there to change the bind address without editing the unit)"
else
  echo "Preserving existing $server_env_file"
fi

# Memory gateway placement (issue #880). Opt-in: GAH_GATEWAY_MODE unset (the
# default) skips this whole section, so a plain `scripts/install.sh` run
# behaves exactly as before. Uses the same server_env_file as HOST above,
# but upserts individual keys rather than HOST's create-once-then-never-
# touch semantics, so re-running install.sh with different GAH_GATEWAY_*
# values updates just those keys and leaves everything else (including an
# operator's own HOST override) alone.
upsert_env_line() {
  local file="$1" key="$2" value="$3"
  sudo install -d -m 0755 "$(dirname "$file")"
  if [ ! -f "$file" ]; then
    sudo install -m 0644 /dev/null "$file"
  fi
  if sudo grep -q "^${key}=" "$file" 2>/dev/null; then
    sudo sed -i "s|^${key}=.*|${key}=${value}|" "$file"
  else
    printf '%s=%s\n' "$key" "$value" | sudo tee -a "$file" >/dev/null
  fi
}

case "${GAH_GATEWAY_MODE:-}" in
  remote)
    : "${GAH_GATEWAY_URL:?GAH_GATEWAY_MODE=remote requires GAH_GATEWAY_URL}"
    echo "Checking remote gateway reachability and auth: $GAH_GATEWAY_URL/recall"
    auth_header=()
    if [ -n "${GAH_GATEWAY_API_KEY:-}" ]; then
      auth_header=(-H "Authorization: Bearer $GAH_GATEWAY_API_KEY")
    fi
    # GET /health needs no auth, so it can't catch a wrong API key -- POST
    # /recall requires auth on every configured gateway and is read-only, so
    # it doubles as a safe reachability+credential check in one call.
    if ! curl -fsS -m 10 "${auth_header[@]}" -H "Content-Type: application/json" \
      -d '{"query":"gah install reachability check","session_key":"gah:install-check"}' \
      "$GAH_GATEWAY_URL/recall" >/dev/null; then
      echo "ERROR: remote gateway at $GAH_GATEWAY_URL is not reachable (or rejected the API key). Aborting install -- fix reachability/credentials and re-run." >&2
      exit 1
    fi
    upsert_env_line "$server_env_file" TDAI_GATEWAY_URL "$GAH_GATEWAY_URL"
    if [ -n "${GAH_GATEWAY_API_KEY:-}" ]; then
      upsert_env_line "$server_env_file" TDAI_GATEWAY_API_KEY "$GAH_GATEWAY_API_KEY"
    fi
    echo "Remote gateway confirmed reachable; wired into $server_env_file"
    ;;
  colocated)
    : "${GAH_GATEWAY_MEMORYCORE_PATH:?GAH_GATEWAY_MODE=colocated requires GAH_GATEWAY_MEMORYCORE_PATH (path to a TencentDB-Agent-Memory/MemoryCore checkout)}"
    : "${GAH_GATEWAY_LLM_API_KEY:?GAH_GATEWAY_MODE=colocated requires GAH_GATEWAY_LLM_API_KEY (an OpenAI-compatible API key for the gateways own LLM calls)}"
    if [ ! -f "$GAH_GATEWAY_MEMORYCORE_PATH/src/gateway/server.ts" ]; then
      echo "ERROR: $GAH_GATEWAY_MEMORYCORE_PATH doesn't look like a TencentDB-Agent-Memory/MemoryCore checkout (missing src/gateway/server.ts)." >&2
      exit 1
    fi

    gateway_local_config="$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.local.yaml"
    if [ ! -f "$gateway_local_config" ]; then
      cp "$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.standalone.yaml" "$gateway_local_config"
      echo "Seeded $gateway_local_config from the tracked standalone template (OpenAI-compatible LLM, embedding off / BM25-only -- edit directly for a different backend)"
    else
      echo "Preserving existing $gateway_local_config"
    fi

    gateway_env_file="$HOME/.config/gah/tdai-gateway.env"
    install -d -m 0700 "$(dirname "$gateway_env_file")"
    gateway_api_key="${GAH_GATEWAY_API_KEY:-}"
    if [ -z "$gateway_api_key" ] && [ -f "$gateway_env_file" ]; then
      gateway_api_key="$(sed -n 's/^TDAI_GATEWAY_API_KEY=//p' "$gateway_env_file" | head -1)"
    fi
    if [ -z "$gateway_api_key" ]; then
      gateway_api_key="$(openssl rand -hex 24)"
    fi
    {
      printf 'TDAI_GATEWAY_API_KEY=%s\n' "$gateway_api_key"
      printf 'TDAI_LLM_API_KEY=%s\n' "$GAH_GATEWAY_LLM_API_KEY"
    } > "$gateway_env_file"
    chmod 0600 "$gateway_env_file"
    echo "Wrote $gateway_env_file"

    node_dir="$(dirname "$(command -v node)")"
    gateway_unit_dst="$HOME/.config/systemd/user/tdai-memory-gateway.service"
    install -d -m 0755 "$(dirname "$gateway_unit_dst")"
    sed \
      -e "s|%h/workspace/agent-lab/repos/github/Kh1ng/TencentDB-Agent-Memory/MemoryCore|$GAH_GATEWAY_MEMORYCORE_PATH|g" \
      -e "s|^Environment=PATH=.*|Environment=PATH=$node_dir:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin|" \
      -e "s|^ExecStart=npx tsx|ExecStart=$node_dir/npx tsx|" \
      packaging/systemd/tdai-memory-gateway.service > "$gateway_unit_dst"
    systemctl --user daemon-reload
    systemctl --user enable --now tdai-memory-gateway.service

    echo "Waiting for co-located gateway to come up..."
    gateway_ready=0
    for _ in $(seq 1 15); do
      if curl -fsS -m 2 -H "Authorization: Bearer $gateway_api_key" http://127.0.0.1:8420/health >/dev/null 2>&1; then
        gateway_ready=1
        break
      fi
      sleep 2
    done
    if [ "$gateway_ready" != "1" ]; then
      echo "ERROR: tdai-memory-gateway.service did not become healthy. Check: journalctl --user -u tdai-memory-gateway.service -n 50" >&2
      exit 1
    fi

    upsert_env_line "$server_env_file" TDAI_GATEWAY_URL "http://127.0.0.1:8420"
    upsert_env_line "$server_env_file" TDAI_GATEWAY_API_KEY "$gateway_api_key"
    echo "Co-located gateway is healthy; wired into $server_env_file"
    ;;
  "")
    ;;
  *)
    echo "ERROR: unknown GAH_GATEWAY_MODE='$GAH_GATEWAY_MODE' (expected 'remote', 'colocated', or unset)" >&2
    exit 1
    ;;
esac

sudo install -m 0644 packaging/systemd/gah-server.service /etc/systemd/system/gah-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now gah-server.service
sudo systemctl is-active --quiet gah-server.service

echo "GAH installed. Update with: gah update --repo $repo_root --restart-server"
