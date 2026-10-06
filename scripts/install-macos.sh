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
  echo '  - Persist node role and optional central URL in GAH configuration; write worker coordinator credentials to ~/.config/gah/gah-loop.env when supplied.'
  echo '  - Install only gah into $CARGO_HOME/bin (default ~/.cargo/bin); build dependencies and outputs in the checkout.'
  echo '  - Install npm dependencies and build the role server; install the desktop app under ~/Applications and role LaunchAgents under ~/Library/LaunchAgents (replace old role agents).'
  if [ "$role" = central ]; then
    echo '  - Build MCP and web UI in the checkout; start the central server LaunchAgent.'
    case "${GAH_GATEWAY_MODE:-}" in
      remote|colocated)
        echo '  - Store gateway credentials in ~/.config/gah/tdai-gateway.env and gateway URL in ~/.config/gah/server.env.'
        if [ "$GAH_GATEWAY_MODE" = colocated ]; then
          echo "  - Seed ${GAH_GATEWAY_MEMORYCORE_PATH:-<MemoryCore checkout>}/tdai-gateway.local.yaml if absent; install and start the memory-gateway LaunchAgent."
        else
          echo '  - Check remote gateway reachability and authentication.'
        fi
        ;;
    esac
  else
    echo '  - Configure worker identity and desktop settings under ~/.local/share/gah/worker and ~/.config/gah; install the worker LaunchAgent stopped until a profile is configured.'
  fi
  echo '  - Enable Tailscale accept-dns when Tailscale is installed.'
  case "${GAH_INSTALL_AGENT:-}" in
    opencode) echo '  - Install OpenCode files in the user config directory opencode/agents/.' ;;
    codex|vibe) echo '  - Install and enable user gah-quota-refresh.service/timer where systemd is available.' ;;
  esac
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
      [ -f "$GAH_GATEWAY_MEMORYCORE_PATH/src/gateway/server.ts" ] || { echo 'ERROR: GAH_GATEWAY_MEMORYCORE_PATH is not a MemoryCore checkout.' >&2; exit 1; }
      gateway_config="$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.local.yaml"
      if [ ! -f "$gateway_config" ]; then
        cp "$GAH_GATEWAY_MEMORYCORE_PATH/tdai-gateway.standalone.yaml" "$gateway_config"
      fi
      # gateway-yaml-mutation:start
      if [ -n "${GAH_GATEWAY_PROVIDER:-}" ]; then
          node --input-type=module - "$GAH_GATEWAY_MEMORYCORE_PATH" "$gateway_config" "$GAH_GATEWAY_PROVIDER" "${GAH_GATEWAY_ENDPOINT:-}" "${GAH_GATEWAY_LLM_MODEL:-}" "${GAH_GATEWAY_EMBEDDING_MODEL:-}" "${GAH_GATEWAY_EMBEDDING_DIMENSIONS:-}" "${GAH_GATEWAY_EMBEDDING_API_KEY:+given}" <<'JAVASCRIPT'
import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
const require = createRequire(resolve(process.argv[2], 'package.json'));
const yaml = require('yaml');
const [configPath, provider, endpoint, llmModel, embedModel, dimensionsArg, embeddingKeyGiven] = process.argv.slice(3);
// Supported contract (Kh1ng/TencentDB-Agent-Memory MemoryCore src/config.ts):
// every embedding provider except none/local/qclaw is an OpenAI-compatible
// service that requests `${baseUrl}/embeddings` and is disabled unless
// apiKey, baseUrl, model, and dimensions are all set.
const providers = {
  ollama: { baseUrl: 'http://127.0.0.1:11434/v1', llmModel: 'llama3', embedModel: 'nomic-embed-text' },
  openai: { baseUrl: 'https://api.openai.com/v1', llmModel: 'gpt-4o', embedModel: 'text-embedding-3-small' },
};
const defaults = Object.hasOwn(providers, provider) ? providers[provider] : undefined;
if (!defaults) {
  console.error(`ERROR: GAH_GATEWAY_PROVIDER must be openai or ollama, not '${provider}'.`);
  process.exit(1);
}
const knownDimensions = new Map([
  ['nomic-embed-text', 768], ['mxbai-embed-large', 1024], ['all-minilm', 384],
  ['text-embedding-3-small', 1536], ['text-embedding-3-large', 3072], ['text-embedding-ada-002', 1536],
]);
const baseUrl = endpoint || defaults.baseUrl;
const model = embedModel || defaults.embedModel;
const dimensions = dimensionsArg ? Number(dimensionsArg) : knownDimensions.get(model);
if (!Number.isInteger(dimensions) || dimensions <= 0) {
  console.error(`ERROR: set GAH_GATEWAY_EMBEDDING_DIMENSIONS to the vector size of embedding model '${model}'.`);
  process.exit(1);
}
const doc = yaml.parseDocument(readFileSync(configPath, 'utf8'));
doc.setIn(['llm', 'baseUrl'], baseUrl);
doc.setIn(['llm', 'model'], llmModel || defaults.llmModel);
// Ollama ignores bearer credentials, but the gateway enables generation and
// embedding only with a non-empty key. The literal placeholder is not a
// secret; TDAI_LLM_API_KEY still overrides llm.apiKey when it is set.
doc.setIn(['llm', 'apiKey'], provider === 'ollama' ? 'ollama' : '${TDAI_LLM_API_KEY}');
doc.setIn(['memory', 'embedding', 'provider'], provider);
doc.setIn(['memory', 'embedding', 'baseUrl'], baseUrl);
doc.setIn(['memory', 'embedding', 'model'], model);
doc.setIn(['memory', 'embedding', 'dimensions'], dimensions);
doc.setIn(['memory', 'embedding', 'apiKey'], provider === 'ollama' && !embeddingKeyGiven ? 'ollama' : '${TDAI_EMBEDDING_API_KEY}');
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
cargo run --locked --bin gah -- update --repo "$repo_root" --role "$role" --yes ${GAH_INSTALL_AGENT:+--agent "$GAH_INSTALL_AGENT"}

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

echo "GAH installed. Update with: gah update --pull --repo $repo_root --role $role"
if [ "$role" = worker ] && [ -f "$gateway_env_file" ]; then
  echo "Worker credentials are in $gateway_env_file; launchd loads them when the desktop starts the worker."
fi
