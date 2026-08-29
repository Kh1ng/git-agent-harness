#!/usr/bin/env bash
# Deterministic first install for a GAH CLI/control-plane host on Linux.
# Uses systemd throughout (gah-loop@.service, gah-server.service, the TDAI
# gateway unit) -- there is no macOS equivalent of any of this. See
# scripts/install-macos.sh for that host, and scripts/install.sh for the
# OS-detecting entrypoint that picks between the two.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Host role (issue seen 2026-08-08: installing on a remote/worker machine
# unconditionally built and started the full control-plane server, and
# unconditionally tried to reload systemd -- which doesn't exist on macOS).
# "central" (default) preserves prior behavior exactly. "worker" installs
# just the CLI + dispatch-loop unit: no apps/server build, no
# gah-server.service.
role="${GAH_NODE_ROLE:-central}"
case "$role" in
  central|worker) ;;
  *) echo "ERROR: unknown GAH_NODE_ROLE='$role' (expected 'central' or 'worker')" >&2; exit 1 ;;
esac

# Fresh installs and routine upgrades use the same Rust update implementation.
# --bin gah is required: Cargo.toml declares a second [[bin]]
# (generate-cli-capabilities) with no default-run set, so a bare
# `cargo run` is ambiguous and errors instead of picking one.
cargo run --bin gah -- update --repo "$repo_root" --role "$role"

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
# behaves exactly as before. Upserts individual keys rather than HOST's
# create-once-then-never-touch semantics, so re-running install.sh with
# different GAH_GATEWAY_* values updates just those keys and leaves
# everything else alone.
#
# Targets depend on role: gah-server.service (central only) reads
# /etc/gah/server.env; gah-loop@.service (worker, and central's own dispatch
# loop) reads ~/.config/gah/gah-loop.env -- so central must upsert into
# *both* files, worker only the second (issue #919: central's own loop had
# zero gateway creds because only server.env was written).
# gateway-target-mapping:start -- extracted verbatim by
# tests/source_structure.rs::install_linux_gateway_mapping_writes_both_targets_for_central,
# keep this block self-contained (only $role/$server_env_file/$HOME as inputs).
if [ "$role" = "central" ]; then
  gateway_env_files=("$server_env_file" "$HOME/.config/gah/gah-loop.env")
  gateway_env_sudo=("sudo" "")
else
  gateway_env_files=("$HOME/.config/gah/gah-loop.env")
  gateway_env_sudo=("")
fi

upsert_env_line() {
  local file="$1" key="$2" value="$3" as="$4"
  $as install -d -m 0755 "$(dirname "$file")"
  if [ ! -f "$file" ]; then
    $as install -m 0644 /dev/null "$file"
  fi
  if $as grep -q "^${key}=" "$file" 2>/dev/null; then
    $as sed -i "s|^${key}=.*|${key}=${value}|" "$file"
  else
    printf '%s=%s\n' "$key" "$value" | $as tee -a "$file" >/dev/null
  fi
}

# Used by both the remote and colocated branches below so they can't drift
# apart on which files get written.
upsert_gateway_env_line() {
  local key="$1" value="$2" i
  for i in "${!gateway_env_files[@]}"; do
    upsert_env_line "${gateway_env_files[$i]}" "$key" "$value" "${gateway_env_sudo[$i]}"
  done
}
# gateway-target-mapping:end

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
    upsert_gateway_env_line TDAI_GATEWAY_URL "$GAH_GATEWAY_URL"
    if [ -n "${GAH_GATEWAY_API_KEY:-}" ]; then
      upsert_gateway_env_line TDAI_GATEWAY_API_KEY "$GAH_GATEWAY_API_KEY"
    fi
    echo "Remote gateway confirmed reachable; wired into: ${gateway_env_files[*]}"
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

    upsert_gateway_env_line TDAI_GATEWAY_URL "http://127.0.0.1:8420"
    upsert_gateway_env_line TDAI_GATEWAY_API_KEY "$gateway_api_key"
    echo "Co-located gateway is healthy; wired into: ${gateway_env_files[*]}"
    ;;
  "")
    ;;
  *)
    echo "ERROR: unknown GAH_GATEWAY_MODE='$GAH_GATEWAY_MODE' (expected 'remote', 'colocated', or unset)" >&2
    exit 1
    ;;
esac

if [ "$role" = "central" ]; then
  sudo install -m 0644 packaging/systemd/gah-server.service /etc/systemd/system/gah-server.service
  sudo systemctl daemon-reload
  sudo systemctl enable --now gah-server.service
  sudo systemctl is-active --quiet gah-server.service
else
  echo "Role is 'worker': skipping gah-server.service (this host doesn't serve the control plane)."
fi

# Existing-project context import (opt-in, issue seen 2026-08-08). Reuses
# whatever gateway this host is now wired to (colocated above, remote above,
# or an inherited $TDAI_GATEWAY_URL). Set GAH_IMPORT_REPO to seed a project's
# handoff/memory docs into the gateway at install time; unset skips this
# entirely, same convention as GAH_GATEWAY_MODE.
if [ -n "${GAH_IMPORT_REPO:-}" ]; then
  : "${GAH_IMPORT_SESSION_KEY:?GAH_IMPORT_REPO requires GAH_IMPORT_SESSION_KEY (e.g. gah:manager:github.com/org/repo)}"
  echo "Importing project context from $GAH_IMPORT_REPO under $GAH_IMPORT_SESSION_KEY..."
  python3 "$repo_root/scripts/backfill_project_docs.py" \
    --repo "$GAH_IMPORT_REPO" \
    --session-key "$GAH_IMPORT_SESSION_KEY" \
    ${GAH_IMPORT_DOCS:+--docs "$GAH_IMPORT_DOCS"}
fi

if [ "$role" = "central" ]; then
  echo "GAH installed. Update with: gah update --repo $repo_root --role central --restart-server"
else
  echo "GAH installed. Update with: gah update --repo $repo_root --role worker"
fi
