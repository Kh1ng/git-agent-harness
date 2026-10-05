#!/usr/bin/env bash
# Shared bootstrap for macOS, Linux, and Windows through WSL2.
set -eo pipefail
umask 077
: "${GAH_PILOT_SOURCE_URL:?}" "${GAH_PILOT_SOURCE_SHA256:?}"
GAH_PILOT_PEER_ID="${GAH_PILOT_PEER_ID:-}"
: "${GAH_SUPABASE_URL:?}" "${GAH_SUPABASE_ANON_KEY:?}" "${GAH_SUPABASE_WORKSPACE_ID:?}"
export GAH_ORCHESTRATION_MODE=hosted
[[ -r /dev/tty ]] || { echo 'Run this command from an interactive terminal.' >&2; exit 1; }
for tool in cargo cc git curl tar; do
  command -v "$tool" >/dev/null || { echo "Install $tool, then paste the command again." >&2; exit 1; }
done
checksum() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi
}
ask() {
  local reply
  printf '%s [%s]: ' "$1" "$2" > /dev/tty
  IFS= read -r reply < /dev/tty
  REPLY="${reply:-$2}"
}
state="$HOME/.config/gah/cloud-pilot"
mkdir -p "$state"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl --fail --silent --show-error --proto '=https' --tlsv1.2 "$GAH_PILOT_SOURCE_URL" -o "$work/source.tar.gz"
actual="$(checksum "$work/source.tar.gz")"; actual="${actual%% *}"
[[ "$actual" == "$GAH_PILOT_SOURCE_SHA256" ]] || { echo 'Source checksum mismatch.' >&2; exit 1; }
tar -xzf "$work/source.tar.gz" -C "$work"
echo 'Building the pilot CLI. The existing gah installation stays available.'
cargo build --jobs 1 --locked --release --bin gah --manifest-path "$work/source/Cargo.toml"
cp "$work/source/target/release/gah" "$state/gah.new"
mv "$state/gah.new" "$state/gah"
gah="$state/gah"
ask 'Share compute, inference, or both' compute; resource="$REPLY"
case "$resource" in compute|inference|both) ;; *) echo 'Choose compute, inference, or both.' >&2; exit 1;; esac
ask 'Node identifier (unique in your team)' "cloud-$(hostname | tr -cd 'a-zA-Z0-9-' | cut -c1-40)"; node="$REPLY"
[[ "$node" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$ ]] || { echo 'Invalid node identifier.' >&2; exit 1; }
ask 'Node display name' 'Shared worker'; name="$REPLY"
ask 'Concurrent job limit' 1; concurrency="$REPLY"
ask 'Memory reservation budget in MiB' 4096; memory="$REPLY"
profiles=(); inference=(); worker=(); share=()
if [[ "$resource" != inference ]]; then
  ask 'Existing GAH profile to share' gah; profile="$REPLY"
  if ! "$gah" profile show "$profile" >/dev/null 2>&1; then
    command -v gh >/dev/null || { echo 'Install GitHub CLI and sign in, then rerun.' >&2; exit 1; }
    gh auth status >/dev/null 2>&1 || gh auth login
    ask 'Repository directory' "$HOME/Developer/git-agent-harness"; repo_dir="$REPLY"
    if [[ ! -d "$repo_dir/.git" ]]; then git clone https://github.com/Kh1ng/git-agent-harness.git "$repo_dir"; fi
    "$gah" init --profile "$profile" --display-name 'Git Agent Harness' --provider github --repo Kh1ng/git-agent-harness --local-path "$repo_dir"
  fi
  ask 'Installed compute backend (codex, claude, opencode, vibe, hermes, agy, openhands)' codex; backend="$REPLY"
  case "$backend" in codex|claude|opencode|vibe|hermes|agy|openhands) ;; *) echo 'Unsupported compute backend.' >&2; exit 1;; esac
  command -v "$backend" >/dev/null || { echo "Install and sign in to $backend, then rerun." >&2; exit 1; }
  profiles=(--profiles "$profile")
  worker=(--profiles "$profile" --backend "$backend")
  if [[ "$backend" == opencode ]]; then worker+=(--allow-inference-relay); fi
fi
if [[ "$resource" != compute ]]; then
  echo 'AI sharing requires an OpenAI-compatible API endpoint. Native subscription logins are not relayed.'
  ask 'Provider base URL ending in /v1' ''; export GAH_CLOUD_INFERENCE_URL="$REPLY"
  ask 'Allowed model IDs, separated by commas' ''; models="$REPLY"
  [[ -n "$models" ]] || { echo 'At least one model is required.' >&2; exit 1; }
  printf 'Provider API key (hidden): ' > /dev/tty
  IFS= read -r -s GAH_CLOUD_INFERENCE_KEY < /dev/tty; printf '\n' > /dev/tty
  [[ -n "$GAH_CLOUD_INFERENCE_KEY" ]] || { echo 'A provider key is required.' >&2; exit 1; }
  export GAH_CLOUD_INFERENCE_KEY
  inference=(--inference-models "$models")
  worker+=(--inference-models "$models")
fi
# Even an inference-only worker needs a valid local configuration for CLI startup.
if ! "$gah" profile list >/dev/null 2>&1; then
  "$gah" init --profile cloud-inference --display-name 'Inference worker' --provider github --repo Kh1ng/git-agent-harness --local-path "$state"
fi
"$gah" cloud login --password < /dev/tty
# This is a per-node credential; it never replaces a local coordinator identity.
device="$state/$node.device.json"
if [[ ! -e "$device" ]]; then
  "$gah" cloud enroll --node "$node" --name "$name" "${profiles[@]}" "${inference[@]}" --max-concurrent "$concurrency" --memory-budget-mb "$memory" --device-file "$device"
fi
if [[ "$resource" != inference && -n "$GAH_PILOT_PEER_ID" ]]; then
  "$gah" cloud share --node "$node" --member "$GAH_PILOT_PEER_ID" --resource compute --permission use
fi
if [[ "$resource" != compute && -n "$GAH_PILOT_PEER_ID" ]]; then
  "$gah" cloud share --node "$node" --member "$GAH_PILOT_PEER_ID" --resource inference --models "$models"
fi
# The restart file contains settings, never the provider key.
restart="$state/$node.start.sh"
{
  printf '#!/usr/bin/env bash\nset -eo pipefail\numask 077\n'
  printf '%s\n' 'if [[ -z "${COORDINATOR_TOKEN:-}" && -f "$HOME/.config/gah/gah-loop.env" ]]; then' '  COORDINATOR_TOKEN="$(source "$HOME/.config/gah/gah-loop.env" > /dev/null; printf "%s" "${COORDINATOR_TOKEN:-}")"' '  export COORDINATOR_TOKEN' 'fi'
  if [[ "$resource" != compute ]]; then
    printf 'export GAH_CLOUD_INFERENCE_URL=%q\n' "$GAH_CLOUD_INFERENCE_URL"
    printf '%s\n' 'if [[ -z "${GAH_CLOUD_INFERENCE_KEY:-}" ]]; then' '  printf "Provider API key (hidden): " > /dev/tty' '  IFS= read -r -s GAH_CLOUD_INFERENCE_KEY < /dev/tty; printf "\n" > /dev/tty' '  export GAH_CLOUD_INFERENCE_KEY' 'fi'
  fi
  printf 'pid_file=%q\n' "$state/$node.worker.pid"
  printf '%s\n' 'if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then echo "Worker is already running."; exit 0; fi'
  printf 'nohup %q cloud worker --device-file %q ' "$gah" "$device"
  for arg in "${worker[@]}" --max-concurrent "$concurrency" --memory-budget-mb "$memory"; do printf '%q ' "$arg"; done
  printf '> %q 2>&1 < /dev/null &\n' "$state/$node.worker.log"
  printf '%s\n' 'pid=$!' 'printf "%s\n" "$pid" > "$pid_file"' 'sleep 2' 'kill -0 "$pid" 2>/dev/null || { echo "Worker stopped. Read its private log." >&2; exit 1; }' 'echo "Shared node started. Worker PID: $pid"'
} > "$restart"
chmod 700 "$restart"
bash "$restart"
echo "Private log: $state/$node.worker.log"
if [[ -n "$GAH_PILOT_PEER_ID" ]]; then
  echo 'The selected teammate has use access. Change permissions in the console.'
else
  echo 'The node is private. Choose sharing in the console.'
fi
echo 'This pilot worker survives terminal closure but needs restarting after a reboot.'
printf 'Restart command: bash %q\n' "$restart"
