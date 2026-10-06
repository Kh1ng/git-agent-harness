#!/usr/bin/env bash
# Validate the installer's provider YAML against a MemoryCore gateway checkout.
# Seeds tdai-gateway.local.yaml from the checkout's tracked standalone template
# (as the installers do), applies install-linux.sh's gateway-yaml-mutation
# block for GAH_GATEWAY_PROVIDER=ollama, starts the real gateway on scratch
# ports with scratch data, and exercises /health, /capture, and /recall.
#
#   scripts/validate-gateway-provider.sh <memorycore-checkout>
#
# By default the backend is scripts/ollama-api-stub.mjs. Set
# VALIDATION_ENDPOINT=http://127.0.0.1:11434/v1 to validate a live Ollama.
# Output is sanitized: no credentials, only request shapes and status fields.
# Requires node, npx (MemoryCore dependencies installed), and curl.
set -euo pipefail
mc="$(cd "$1" && pwd)"
gah="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
stub_port="${VALIDATION_STUB_PORT:-11534}"
gw_port="${VALIDATION_GATEWAY_PORT:-18420}"
key="validation-gateway-key-$RANDOM$RANDOM"
gateway_pid=""
cleanup() {
  if [ -n "$gateway_pid" ]; then kill -- "-$gateway_pid" 2>/dev/null || true; fi
  kill $(jobs -p) 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

echo "== MemoryCore revision: $(git -C "$mc" rev-parse HEAD 2>/dev/null || echo unknown)"
endpoint="${VALIDATION_ENDPOINT:-}"
if [ -z "$endpoint" ]; then
  : >"$work/stub.log"
  node "$gah/scripts/ollama-api-stub.mjs" "$stub_port" "$work/stub.log" &
  endpoint="http://127.0.0.1:$stub_port/v1"
  echo "== backend: Ollama-compatible stub (not a live Ollama)"
else
  echo "== backend: $endpoint"
fi

cp "$mc/tdai-gateway.standalone.yaml" "$work/tdai-gateway.local.yaml"
block="$(awk '/# gateway-yaml-mutation:start/{f=1} /# gateway-yaml-mutation:end/{f=0} f' "$gah/scripts/install-linux.sh")"
env -i PATH="$PATH" HOME="$work" \
  GAH_GATEWAY_MEMORYCORE_PATH="$mc" gateway_local_config="$work/tdai-gateway.local.yaml" \
  GAH_GATEWAY_PROVIDER=ollama GAH_GATEWAY_ENDPOINT="$endpoint" \
  bash -euc "$block"

json() { (cd "$mc" && node -e "$1" "${@:2}"); }
echo "== installer-written provider configuration"
json "const y=require('yaml');const c=y.parse(require('fs').readFileSync(process.argv[1],'utf8'));console.log(JSON.stringify({llm:{baseUrl:c.llm.baseUrl,model:c.llm.model,apiKey:c.llm.apiKey},embedding:c.memory.embedding},null,2))" "$work/tdai-gateway.local.yaml"

# Its own process group, so cleanup also stops the processes npx spawns.
(cd "$mc" && exec setsid env -i PATH="$PATH" HOME="$work" \
  TDAI_GATEWAY_CONFIG="$work/tdai-gateway.local.yaml" TDAI_GATEWAY_PORT="$gw_port" \
  TDAI_DATA_DIR="$work/data" TDAI_GATEWAY_API_KEY="$key" \
  npx tsx src/gateway/server.ts >"$work/gateway.log" 2>&1) &
gateway_pid=$!

for _ in $(seq 1 90); do
  curl -fsS -m 2 "http://127.0.0.1:$gw_port/health" >/dev/null 2>&1 && break
  sleep 1
done
echo "== GET /health"
curl -fsS "http://127.0.0.1:$gw_port/health" >"$work/health.json"
json "const h=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'));console.log(JSON.stringify({status:h.status,stores:h.stores}))" "$work/health.json"

auth=(-H "Authorization: Bearer $key" -H 'Content-Type: application/json')
echo "== POST /capture"
curl -fsS "${auth[@]}" -d '{"session_key":"gah:validation","user_content":"The deploy target for project lighthouse is staging-west.","assistant_content":"Noted: lighthouse deploys to staging-west."}' \
  "http://127.0.0.1:$gw_port/capture" >"$work/capture.json"
json "const r=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'));console.log(JSON.stringify({l0_recorded:r.l0_recorded,scheduler_notified:r.scheduler_notified}))" "$work/capture.json"
sleep 2
echo "== POST /recall"
curl -fsS "${auth[@]}" -d '{"query":"Where does lighthouse deploy?","session_key":"gah:validation"}' \
  "http://127.0.0.1:$gw_port/recall" >"$work/recall.json"
json "const r=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'));console.log(JSON.stringify({code:r.code,message:r.message,strategy:r.strategy,memory_count:r.memory_count}))" "$work/recall.json"
sleep 2
if [ -f "$work/stub.log" ]; then
  echo "== backend requests observed by the stub"
  sort "$work/stub.log" | uniq -c
fi
echo "== gateway embedding log lines"
grep -oE 'Using remote embedding.*|Embedding has been disabled.*|\[hybrid-embedding\] Embedding OK, dims=[0-9]+|Background embedding complete: [0-9/]+ vectors updated' "$work/gateway.log" | sort | uniq -c || true
grep -q '"embeddingService":true' "$work/health.json"
