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
  central|standalone|worker) ;;
  *) echo "ERROR: unknown GAH_NODE_ROLE='$role' (expected 'central', 'standalone', or 'worker')" >&2; exit 1 ;;
esac


# Fresh installs and routine upgrades use the same Rust update implementation.
# --bin gah is required: Cargo.toml declares a second [[bin]]
# (generate-cli-capabilities) with no default-run set, so a bare
# `cargo run` is ambiguous and errors instead of picking one.
bash "$repo_root/scripts/configure-node-role.sh" "$role" cargo run --locked --bin gah --
# The update command also enables user lingering (issue #1347) so the user
# units it installs survive reboots without a login session. Only central,
# which already needs sudo here, may prompt; a worker host without root uses
# `sudo -n`, so it still installs cleanly and only gets a warning.
cargo run --locked --bin gah -- update --repo "$repo_root" --role "$role"

# tailscale-dns-guard:start -- extracted verbatim by
# tests/source_structure.rs::standalone_install_never_touches_tailscale,
# keep this block self-contained (only $role as input).
if [ "$role" != "standalone" ]; then
  # MagicDNS is useful only when this client accepts the tailnet DNS settings.
  # The tailnet-wide toggle still belongs to the Tailscale admin console.
  if command -v tailscale >/dev/null 2>&1; then
    sudo tailscale set --accept-dns=true
  else
    echo 'WARNING: Tailscale is not installed; join the tailnet, then run tailscale set --accept-dns=true.' >&2
  fi
fi
# tailscale-dns-guard:end

# Persistent server bind-host override (issue #643). Created only on first
# central install; later central installs and `gah update --restart-server`
# leave it untouched. Standalone reasserts loopback on
# every install, including a switch from an existing networked server. Set
# GAH_SERVER_HOST to explicitly choose another bind address.
# server-env-config:start -- exercised by
# tests/source_structure.rs::standalone_rebinds_an_existing_server_env.
server_env_file=/etc/gah/server.env
if [ "$role" = "central" ] || [ "$role" = "standalone" ]; then
  if [ ! -f "$server_env_file" ]; then
    sudo install -d -m 0755 /etc/gah
    # server-host-default:start -- exercised without sudo/file writes by
    # tests/source_structure.rs::install_linux_prefers_the_tailnet_bind_host.
    server_host="${GAH_SERVER_HOST:-}"
    if [ -z "$server_host" ]; then
      if [ "$role" = "standalone" ]; then
        server_host=127.0.0.1
      else
        server_host="$(gah tailscale-ip 2>/dev/null || true)"
        if [ -z "$server_host" ]; then server_host=127.0.0.1; fi
      fi
    fi
    # server-host-default:end
    printf 'HOST=%s\n' "$server_host" | sudo tee "$server_env_file" >/dev/null
    sudo chmod 0644 "$server_env_file"
    echo "Created $server_env_file (set HOST= there to change the bind address without editing the unit)"
  else
    if [ "$role" = "standalone" ]; then
      server_host="${GAH_SERVER_HOST:-127.0.0.1}"
      printf '%s' "$server_host" | sudo "$(command -v gah || echo "$HOME/.cargo/bin/gah")" installer env-set --file "$server_env_file" HOST
      echo "Set HOST=$server_host in $server_env_file for standalone mode"
    else
      echo "Preserving existing $server_env_file"
    fi
  fi
fi
# server-env-config:end

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
if [ "$role" = "central" ] || [ "$role" = "standalone" ]; then
  gateway_env_files=("$server_env_file" "$HOME/.config/gah/gah-loop.env")
  gateway_env_sudo=("sudo" "")
else
  gateway_env_files=("$HOME/.config/gah/gah-loop.env")
  gateway_env_sudo=("")
fi

upsert_env_line() {
  local file="$1" key="$2" value="$3" as="$4"
  # Quoted for both shell sourcing and systemd EnvironmentFile. The value
  # travels on stdin, never in sed replacement syntax or process arguments;
  # sudo needs gah's full path.
  printf '%s' "$value" | $as "$(command -v gah || echo "$HOME/.cargo/bin/gah")" installer env-set --file "$file" "$key"
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
    # gateway-url-default:start -- extracted verbatim by
    # tests/source_structure.rs::install_linux_defaults_gateway_url_to_central_host.
    # Issue #947: use the already-configured central host; `tailscale ip -4`
    # would return this worker's own address, not the remote gateway's.
    if [ -z "${GAH_GATEWAY_URL:-}" ]; then
      gah_config="${GAH_CONFIG:-$HOME/.config/gah/config.toml}"
      registry_central_url=""
      if [ -f "$gah_config" ]; then
        registry_central_url="$(sed -n 's/^[[:space:]]*registry_central_url[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$gah_config" | head -1)"
      fi
      : "${registry_central_url:?GAH_GATEWAY_MODE=remote requires GAH_GATEWAY_URL or registry_central_url in $gah_config; a worker cannot infer which Tailscale peer hosts the gateway}"
      case "$registry_central_url" in
        http://*|https://*) ;;
        *) echo "ERROR: registry_central_url in $gah_config must be an http(s) URL" >&2; exit 1 ;;
      esac
      central_authority="${registry_central_url#*://}"
      central_authority="${central_authority%%/*}"
      central_host="${central_authority%%:*}"
      : "${central_host:?registry_central_url in $gah_config has no host}"
      GAH_GATEWAY_URL="http://${central_host}:8420"
      echo "GAH_GATEWAY_URL not set; defaulting to configured central host: $GAH_GATEWAY_URL" >&2
    fi
    # gateway-url-default:end
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
      upsert_env_line "$HOME/.config/gah/tdai-gateway.env" TDAI_GATEWAY_API_KEY "$GAH_GATEWAY_API_KEY" ""
    fi
    echo "Remote gateway confirmed reachable; wired into: ${gateway_env_files[*]}"
    ;;
  colocated)
    : "${GAH_GATEWAY_MEMORYCORE_PATH:?GAH_GATEWAY_MODE=colocated requires GAH_GATEWAY_MEMORYCORE_PATH (path to a TencentDB-Agent-Memory/MemoryCore checkout)}"
    if [ ! -f "$GAH_GATEWAY_MEMORYCORE_PATH/src/gateway/server.ts" ]; then
      echo "ERROR: $GAH_GATEWAY_MEMORYCORE_PATH doesn't look like a TencentDB-Agent-Memory/MemoryCore checkout (missing src/gateway/server.ts)." >&2
      exit 1
    fi

    gateway_local_config="$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.local.yaml"
    if [ ! -f "$gateway_local_config" ]; then
      cp "$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.standalone.yaml" "$gateway_local_config"
      # gateway-yaml-mutation:start
      if [ -n "${GAH_GATEWAY_PROVIDER:-}" ]; then
        node --input-type=module - "$GAH_GATEWAY_MEMORYCORE_PATH" "$gateway_local_config" "$GAH_GATEWAY_PROVIDER" "${GAH_GATEWAY_ENDPOINT:-}" "${GAH_GATEWAY_LLM_MODEL:-}" "${GAH_GATEWAY_EMBEDDING_MODEL:-}" <<'JAVASCRIPT'
import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
const require = createRequire(resolve(process.argv[2], 'package.json'));
const yaml = require('yaml');
const configPath = process.argv[3];
const provider = process.argv[4];
const endpoint = process.argv[5];
const llmModel = process.argv[6];
const embedModel = process.argv[7];
const doc = yaml.parseDocument(readFileSync(configPath, 'utf8'));
if (provider === 'ollama') {
  doc.setIn(['llm', 'baseUrl'], endpoint || 'http://127.0.0.1:11434');
  doc.setIn(['llm', 'model'], llmModel || 'llama3');
  doc.setIn(['embedding', 'provider'], 'ollama');
  doc.setIn(['embedding', 'baseUrl'], endpoint || 'http://127.0.0.1:11434');
  doc.setIn(['embedding', 'model'], embedModel || 'nomic-embed-text');
} else if (provider === 'openai') {
  doc.setIn(['llm', 'baseUrl'], endpoint || 'https://api.openai.com/v1');
  doc.setIn(['llm', 'model'], llmModel || 'gpt-4o');
  doc.setIn(['embedding', 'provider'], 'openai');
  doc.setIn(['embedding', 'baseUrl'], endpoint || 'https://api.openai.com/v1');
  doc.setIn(['embedding', 'model'], embedModel || 'text-embedding-3-small');
}
writeFileSync(configPath, String(doc));
JAVASCRIPT
      fi
      # gateway-yaml-mutation:end
      echo "Seeded $gateway_local_config from the tracked standalone template (OpenAI-compatible LLM, embedding off / BM25-only -- edit directly for a different backend)"
    else
      echo "Preserving existing $gateway_local_config"
    fi

    # gateway-env-setup:start
    gateway_env_file="$HOME/.config/gah/tdai-gateway.env"
    install -d -m 0700 "$(dirname "$gateway_env_file")"
    # Preserve model/provider credentials and existing gateway authentication.
    if [ -n "${GAH_GATEWAY_API_KEY:-}" ]; then
      upsert_env_line "$gateway_env_file" TDAI_GATEWAY_API_KEY "$GAH_GATEWAY_API_KEY" ""
      echo "Wrote the given gateway API key to $gateway_env_file"
    elif ! "$(command -v gah || echo "$HOME/.cargo/bin/gah")" installer env-has --file "$gateway_env_file" TDAI_GATEWAY_API_KEY >/dev/null 2>&1; then
      upsert_env_line "$gateway_env_file" TDAI_GATEWAY_API_KEY "$(openssl rand -hex 24)" ""
      echo "Generated a gateway API key in $gateway_env_file"
    else
      echo "Kept the existing gateway API key in $gateway_env_file"
    fi
    if [ -n "${GAH_GATEWAY_LLM_API_KEY:-}" ]; then
      upsert_env_line "$gateway_env_file" TDAI_LLM_API_KEY "$GAH_GATEWAY_LLM_API_KEY" ""
    fi
    if [ -n "${GAH_GATEWAY_EMBEDDING_API_KEY:-}" ]; then
      upsert_env_line "$gateway_env_file" TDAI_EMBEDDING_API_KEY "$GAH_GATEWAY_EMBEDDING_API_KEY" ""
      echo "Wrote the given Embedding API key to $gateway_env_file"
      echo "Wrote the given LLM API key to $gateway_env_file"
    fi
    chmod 0600 "$gateway_env_file"
    # gateway-env-setup:end

    node_dir="$(dirname "$(command -v node)")"
    gateway_unit_dst="$HOME/.config/systemd/user/tdai-memory-gateway.service"
    install -d -m 0755 "$(dirname "$gateway_unit_dst")"
    # gateway-unit-render:start
    node --input-type=module - "$GAH_GATEWAY_MEMORYCORE_PATH" "$node_dir" "$gateway_unit_dst" <<'JAVASCRIPT'
import { readFileSync, writeFileSync } from 'node:fs';
import { isAbsolute } from 'node:path';

let template = readFileSync('packaging/gateway/tdai-memory-gateway.service', 'utf8');
for (const [placeholder, value] of [['@MEMORYCORE@', process.argv[2]], ['@NODE_DIR@', process.argv[3]]]) {
  if (!isAbsolute(value) || /["\\$\x00-\x1f\x7f]/.test(value)) {
    throw new Error('Gateway paths must be absolute and contain no quotes, backslashes, dollar signs, or control characters');
  }
  if (placeholder === '@NODE_DIR@' && value.includes(':')) {
    throw new Error('Gateway Node directory cannot contain a colon');
  }
  template = template.replaceAll(placeholder, value.replaceAll('%', '%%'));
}
writeFileSync(process.argv[4], template);
JAVASCRIPT
    # gateway-unit-render:end
    systemctl --user daemon-reload
    systemctl --user enable --now tdai-memory-gateway.service

    echo "Waiting for co-located gateway to come up..."
    gateway_ready=0
    for _ in $(seq 1 15); do
      if curl -fsS -m 2 http://127.0.0.1:8420/health >/dev/null 2>&1; then
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
    echo "Co-located gateway is healthy; wired into: ${gateway_env_files[*]}"
    ;;
  "")
    ;;
  *)
    echo "ERROR: unknown GAH_GATEWAY_MODE='$GAH_GATEWAY_MODE' (expected 'remote', 'colocated', or unset)" >&2
    exit 1
    ;;
esac

# server-service-start:start -- tested with stubbed systemctl by
# tests/source_structure.rs::standalone_service_restart_and_start_failures_report_journal.
if [ "$role" = "central" ] || [ "$role" = "standalone" ]; then
  # `gah update` above rendered and installed the unit for this account (#1322).
  server_start_failed=0
  sudo systemctl enable --now gah-server.service || server_start_failed=1
  if [ "$server_start_failed" = 0 ] && [ "$role" = "standalone" ]; then
    # enable --now does not restart a service that was already running with
    # the former network bind address.
    sudo systemctl restart gah-server.service || server_start_failed=1
  fi
  # Restart=always hides a crash loop from a single is-active check: require
  # the service to stay up without restarting, and show why when it does not.
  if [ "$server_start_failed" = 0 ]; then
    sleep 5
    if ! sudo systemctl is-active --quiet gah-server.service \
      || [ "$(systemctl show -p NRestarts --value gah-server.service)" != "0" ]; then
      server_start_failed=1
    fi
  fi
  if [ "$server_start_failed" != 0 ]; then
    echo "ERROR: gah-server.service did not stay running." >&2
    sudo systemctl status --no-pager gah-server.service >&2 || true
    sudo journalctl -u gah-server.service -n 50 --no-pager >&2 || true
    exit 1
  fi
else
  echo "Role is 'worker': skipping gah-server.service (this host doesn't serve the control plane)."
fi
# server-service-start:end

# Existing-project context import (opt-in, issue seen 2026-08-08). Reuses
# whatever gateway this host is now wired to (colocated above, remote above,
# or an inherited $TDAI_GATEWAY_URL). Set GAH_IMPORT_REPO to seed a project's
# handoff/memory docs into the gateway at install time; unset skips this
# entirely, same convention as GAH_GATEWAY_MODE.
if [ -n "${GAH_IMPORT_REPO:-}" ]; then
  : "${GAH_IMPORT_SESSION_KEY:?GAH_IMPORT_REPO requires GAH_IMPORT_SESSION_KEY (e.g. gah:manager:github.com/org/repo)}"
  echo "Importing project context from $GAH_IMPORT_REPO under $GAH_IMPORT_SESSION_KEY..."
  "$(command -v gah || echo "$HOME/.cargo/bin/gah")" installer import-docs \
    --repo "$GAH_IMPORT_REPO" \
    --session-key "$GAH_IMPORT_SESSION_KEY" \
    ${GAH_IMPORT_DOCS:+--docs "$GAH_IMPORT_DOCS"}
fi

if [ "$role" = "central" ] || [ "$role" = "standalone" ]; then
  echo "GAH installed. Update with: gah update --repo $repo_root --role $role --restart-server"
else
  echo "GAH installed. Update with: gah update --repo $repo_root --role worker"
fi
