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

# --bin gah is required: Cargo.toml declares a second [[bin]]
# (generate-cli-capabilities) with no default-run set, so a bare `cargo run`
# is ambiguous and errors instead of picking one. The first call builds gah.
gah_cli=(cargo run --locked -q --bin gah --)

if [ "$role" = central ]; then
  case "${GAH_GATEWAY_MODE:-}" in
    colocated)
      : "${GAH_GATEWAY_MEMORYCORE_PATH:?GAH_GATEWAY_MODE=colocated requires GAH_GATEWAY_MEMORYCORE_PATH}"
      [ -f "$GAH_GATEWAY_MEMORYCORE_PATH/src/gateway/server.ts" ] || { echo 'ERROR: GAH_GATEWAY_MEMORYCORE_PATH is not a MemoryCore checkout.' >&2; exit 1; }
      gateway_config="$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.local.yaml"
      if [ ! -f "$gateway_config" ]; then
        cp "$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.standalone.yaml" "$gateway_config"
      fi
      # gateway-yaml-mutation:start
      if [ -n "${GAH_GATEWAY_PROVIDER:-}" ]; then
          node --input-type=module - "$GAH_GATEWAY_MEMORYCORE_PATH" "$gateway_config" "$GAH_GATEWAY_PROVIDER" "${GAH_GATEWAY_ENDPOINT:-}" "${GAH_GATEWAY_LLM_MODEL:-}" "${GAH_GATEWAY_EMBEDDING_MODEL:-}" <<'JAVASCRIPT'
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
  doc.setIn(['memory', 'embedding', 'provider'], 'ollama');
  doc.setIn(['memory', 'embedding', 'baseUrl'], endpoint || 'http://127.0.0.1:11434');
  doc.setIn(['memory', 'embedding', 'model'], embedModel || 'nomic-embed-text');
} else if (provider === 'openai') {
  doc.setIn(['llm', 'baseUrl'], endpoint || 'https://api.openai.com/v1');
  doc.setIn(['llm', 'model'], llmModel || 'gpt-4o');
  doc.setIn(['memory', 'embedding', 'provider'], 'openai');
  doc.setIn(['memory', 'embedding', 'baseUrl'], endpoint || 'https://api.openai.com/v1');
  doc.setIn(['memory', 'embedding', 'model'], embedModel || 'text-embedding-3-small');
}
writeFileSync(configPath, String(doc));
JAVASCRIPT
      fi
      # gateway-yaml-mutation:end
      # gateway-env-setup:start
      gateway_env="$HOME/.config/gah/tdai-gateway.env"
      export GAH_MACOS_GATEWAY_URL=http://127.0.0.1:8420
      # Values travel on stdin; preserve existing provider and access credentials.
      if [ -n "${GAH_GATEWAY_API_KEY:-}" ]; then
        printf '%s' "$GAH_GATEWAY_API_KEY" | "${gah_cli[@]}" installer env-set --file "$gateway_env" TDAI_GATEWAY_API_KEY
      elif ! "${gah_cli[@]}" installer env-has --file "$gateway_env" TDAI_GATEWAY_API_KEY >/dev/null 2>&1; then
        openssl rand -hex 24 | tr -d '\n' | "${gah_cli[@]}" installer env-set --file "$gateway_env" TDAI_GATEWAY_API_KEY
      fi
      if [ -n "${GAH_GATEWAY_LLM_API_KEY:-}" ]; then
        printf '%s' "$GAH_GATEWAY_LLM_API_KEY" | "${gah_cli[@]}" installer env-set --file "$gateway_env" TDAI_LLM_API_KEY
      fi
      if [ -n "${GAH_GATEWAY_EMBEDDING_API_KEY:-}" ]; then
        printf "%s" "$GAH_GATEWAY_EMBEDDING_API_KEY" | "${gah_cli[@]}" installer env-set --file "$gateway_env" TDAI_EMBEDDING_API_KEY
      fi
      chmod 0600 "$gateway_env"
      # gateway-env-setup:end
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
cargo run --locked --bin gah -- update --repo "$repo_root" --role "$role"

if [ "$role" = central ] && [ "${GAH_GATEWAY_MODE:-}" = colocated ]; then
  gateway_ready=0
  for _ in $(seq 1 15); do
    if curl -fsS -m 2 http://127.0.0.1:8420/health >/dev/null 2>&1; then
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
