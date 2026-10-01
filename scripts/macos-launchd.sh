#!/usr/bin/env bash
# Own the macOS control-plane/worker lifecycle from one deterministic place.
set -euo pipefail
trap 'echo "ERROR: macOS LaunchAgent setup failed at line $LINENO." >&2' ERR

action="${1:?Usage: macos-launchd.sh install|start|stop|status central|worker [repo] [profile]}"
role="${2:?Missing node role}"
case "$role" in central|worker) ;; *) echo "ERROR: invalid node role '$role'" >&2; exit 1 ;; esac

server_label=dev.git-agent-harness.server
worker_label=dev.git-agent-harness.worker
tunnel_label=dev.git-agent-harness.worker-tunnel
gateway_label=dev.git-agent-harness.memory-gateway
label="$server_label"
[ "$role" = worker ] && label="$worker_label"
domain="gui/${GAH_LAUNCHD_UID:-$(id -u)}"
agents_dir="${GAH_LAUNCH_AGENTS_DIR:-$HOME/Library/LaunchAgents}"
plist="$agents_dir/$label.plist"
# gah writes and reads the agents' files (`gah installer`).
cli="${GAH_CLI_PATH:-$(command -v gah || echo "$HOME/.cargo/bin/gah")}"

run_launchctl() {
  if [ "${GAH_LAUNCHD_DRY_RUN:-}" != 1 ]; then launchctl "$@"; fi
}

bootstrap_agent() {
  if [ "${GAH_LAUNCHD_DRY_RUN:-}" = 1 ]; then return; fi
  for attempt in 1 2 3; do
    launchctl bootstrap "$domain" "$1" && return
    [ "$attempt" = 3 ] || sleep 1
  done
  return 1
}

plist_value() {
  "$cli" installer plist-get --file "$plist" "$1"
}

# Startup time varies with the machine and with what the server loads first,
# so give it a minute and show progress. On timeout, the worker log explains
# the failure better than the installer can.
wait_for_worker_health() {
  local url="$1" limit="${GAH_WORKER_HEALTH_TIMEOUT:-60}" log="$HOME/.local/state/gah/worker.log"
  local started="$SECONDS" reported=0
  until /usr/bin/curl "${curl_args[@]}" "$url" >/dev/null 2>&1; do
    local waited=$((SECONDS - started))
    if [ "$waited" -ge "$limit" ]; then
      echo "ERROR: the macOS worker did not become healthy at ${url%/health} within ${limit}s" >&2
      if [ -f "$log" ]; then
        echo "Last 20 lines of $log:" >&2
        tail -n 20 "$log" >&2
      fi
      return 1
    fi
    if [ "$waited" -ge $((reported + 5)) ]; then
      reported="$waited"
      echo "Waiting for the macOS worker at ${url%/health} (${waited}s of ${limit}s)..."
    fi
    sleep 1
  done
}

register_worker() {
  [ "${GAH_LAUNCHD_DRY_RUN:-}" = 1 ] && return
  gah_path="$(plist_value GAH_BINARY)"
  identity_path="$(plist_value GAH_COORDINATOR_IDENTITY_PATH)"
  host="$(plist_value HOST)"
  port="$(plist_value PORT)"
  transport_mode="$(plist_value GAH_REGISTRY_TRANSPORT_MODE)"
  set -a
  # shellcheck disable=SC1090
  [ ! -f "$HOME/.config/gah/gah-loop.env" ] || . "$HOME/.config/gah/gah-loop.env"
  set +a
  curl_args=(-fsS -m 2)
  [ -z "${COORDINATOR_TOKEN:-}" ] || curl_args+=(-H "Authorization: Bearer $COORDINATOR_TOKEN")
  wait_for_worker_health "http://$host:$port/health" || return 1
  registration="$(GAH_COORDINATOR_IDENTITY_PATH="$identity_path" "$gah_path" node register --transport-mode "$transport_mode")"
  printf '%s\n' "$registration"
  central_url="$(printf '%s\n' "$registration" | sed -n 's/^Registered node against //p' | tail -n 1)"
  [ -n "$central_url" ] || { echo 'ERROR: gah node register did not report the central URL.' >&2; return 1; }
  node_id="$("$cli" installer json --file "$identity_path" --pointer /node_id)"
  health_url="${central_url%/}/api/registry/nodes/$node_id/health"
  health_args=(-fsS -m 15)
  [ -z "${COORDINATOR_TOKEN:-}" ] || health_args+=(-H "Authorization: Bearer $COORDINATOR_TOKEN")
  health=''
  for _ in 1 2 3; do
    health="$(/usr/bin/curl "${health_args[@]}" "$health_url" 2>/dev/null || true)"
    if [ "$(printf '%s' "$health" | "$cli" installer json --pointer /status 2>/dev/null || true)" = healthy ]; then
      return
    fi
    sleep 1
  done
  detail="$(printf '%s' "$health" | "$cli" installer json --pointer /error/message 2>/dev/null \
    || printf '%s' "$health" | "$cli" installer json --pointer /state 2>/dev/null \
    || echo 'no health response')"
  echo "ERROR: central registered this worker but cannot reach its advertised URL: $detail" >&2
  return 1
}

configure_worker_transport() {
  [ "${GAH_LAUNCHD_DRY_RUN:-}" = 1 ] && return
  tunnel_plist="$agents_dir/$tunnel_label.plist"
  if [ -f "$tunnel_plist" ] && [ "$(plist_value GAH_REGISTRY_TRANSPORT_MODE)" = loopback ]; then
    run_launchctl bootout "$domain/$tunnel_label" >/dev/null 2>&1 || true
    bootstrap_agent "$tunnel_plist"
  fi
  [ "$(plist_value GAH_TAILSCALE_SERVE)" = 1 ] || return 0
  tailscale_path="$(plist_value GAH_TAILSCALE_CLI)"
  port="$(plist_value PORT)"
  "$tailscale_path" serve --bg --yes --https="$port" "http://127.0.0.1:$port" >/dev/null
}

disable_worker_transport() {
  [ "${GAH_LAUNCHD_DRY_RUN:-}" = 1 ] && return
  run_launchctl bootout "$domain/$tunnel_label" >/dev/null 2>&1 || true
  worker_plist="$agents_dir/$worker_label.plist"
  [ -f "$worker_plist" ] || return 0
  [ "$("$cli" installer plist-get --file "$worker_plist" GAH_TAILSCALE_SERVE 2>/dev/null || true)" = 1 ] || return 0
  tailscale_path="$("$cli" installer plist-get --file "$worker_plist" GAH_TAILSCALE_CLI)"
  port="$("$cli" installer plist-get --file "$worker_plist" PORT)"
  "$tailscale_path" serve --https="$port" off >/dev/null 2>&1 || true
}

case "$action" in
  status)
    run_launchctl print "$domain/$label" >/dev/null 2>&1
    exit $?
    ;;
  start)
    [ "$role" != worker ] || configure_worker_transport
    run_launchctl kickstart -k "$domain/$label"
    [ "$role" != worker ] || register_worker
    exit
    ;;
  stop)
    [ "$role" != worker ] || disable_worker_transport
    run_launchctl bootout "$domain/$label" >/dev/null 2>&1 || true
    exit
    ;;
  install) ;;
  *) echo "ERROR: invalid action '$action'" >&2; exit 1 ;;
esac

[ "$role" != worker ] || disable_worker_transport

repo="${3:?Install requires the GAH repository path}"
profile="${4:-}"
repo="$(cd "$repo" && pwd -P)"
[ -f "$repo/Cargo.toml" ] || { echo "ERROR: $repo is not a GAH checkout" >&2; exit 1; }

mkdir -p "$agents_dir" "$HOME/.local/state/gah" "$HOME/.cache/gah/tmp" "$HOME/.config/gah"
node_path="${GAH_NODE_PATH:-$(command -v node || true)}"
gah_path="${GAH_CLI_PATH:-$(command -v gah || true)}"
if [ -z "$node_path" ]; then echo "ERROR: node is required for $role mode" >&2; exit 1; fi
if [ "$role" = worker ] && [ -z "$gah_path" ]; then echo 'ERROR: gah is required for worker mode' >&2; exit 1; fi
[ -x "$cli" ] || { echo "ERROR: gah is required to write the LaunchAgents (looked for $cli)." >&2; exit 1; }

explicit_port="${GAH_DESKTOP_SERVER_PORT:-}"
port="$explicit_port"
explicit_advertised_url="${GAH_NODE_ADVERTISED_URL:-}"
explicit_transport_mode="${GAH_NODE_TRANSPORT_MODE:-}"
advertised_url="$explicit_advertised_url"
transport_mode="$explicit_transport_mode"
worker_identity="$HOME/.local/share/gah/worker/identity.json"
if [ "$role" = worker ] && [ -z "$port" ] && [ -f "$plist" ]; then
  port="$(plist_value PORT 2>/dev/null || true)"
fi
[ -n "$port" ] || port=3774
if [ "$role" = worker ] && [ -z "$advertised_url" ] && [ -f "$plist" ]; then
  advertised_url="$(plist_value GAH_NODE_ADVERTISED_URL 2>/dev/null || true)"
fi
if [ "$role" = worker ] && [ -z "$advertised_url" ] && [ -f "$worker_identity" ]; then
  advertised_url="$("$cli" installer json --file "$worker_identity" --pointer /advertised_url 2>/dev/null || true)"
fi
if [ "$role" = worker ] && [ -z "$explicit_advertised_url" ] && [ -z "$transport_mode" ] && [ -f "$plist" ]; then
  transport_mode="$(plist_value GAH_REGISTRY_TRANSPORT_MODE 2>/dev/null || true)"
fi
tunnel_target="${GAH_NODE_SSH_TARGET:-}"
tunnel_remote_port="${GAH_NODE_SSH_REMOTE_PORT:-}"
if [ "$role" = worker ] && [ -n "$tunnel_target" ]; then
  [ -n "$tunnel_remote_port" ] || { echo 'ERROR: GAH_NODE_SSH_REMOTE_PORT is required with GAH_NODE_SSH_TARGET.' >&2; exit 1; }
  [ -n "$explicit_advertised_url" ] || advertised_url="http://127.0.0.1:$tunnel_remote_port"
  [ -n "$explicit_transport_mode" ] || transport_mode=loopback
fi
tailscale_path="${GAH_TAILSCALE_PATH:-$(command -v tailscale || true)}"
if [ -z "$tailscale_path" ] && [ -x /Applications/Tailscale.app/Contents/MacOS/Tailscale ]; then
  tailscale_path=/Applications/Tailscale.app/Contents/MacOS/Tailscale
fi
if [ "$role" = worker ] && [ -z "$advertised_url" ]; then
  [ -n "$tailscale_path" ] || { echo 'ERROR: Tailscale is required for a macOS worker.' >&2; exit 1; }
  tailscale_ip="$("$tailscale_path" status --json | "$cli" installer json --pointer /Self/TailscaleIPs --lines | grep -v : | head -n 1 || true)"
  [ -n "$tailscale_ip" ] || { echo 'ERROR: cannot find this Mac tailnet IPv4 address.' >&2; exit 1; }
  advertised_url="http://$tailscale_ip:$port"
fi

"$cli" installer launchd \
  --role "$role" --repo "$repo" --profile "$profile" --label "$label" --plist "$plist" \
  --node "$node_path" --gah "$gah_path" --port "$port" --advertised-url "$advertised_url" \
  --tailscale "$tailscale_path" --transport-mode "$transport_mode" \
  --tunnel-target="$tunnel_target" --tunnel-remote-port="$tunnel_remote_port" \
  --npx "${GAH_NPX_PATH:-$(command -v npx || true)}" \
  --memorycore "${GAH_GATEWAY_MEMORYCORE_PATH:-}"

other="$worker_label"
[ "$role" = worker ] && other="$server_label"
[ "$role" != central ] || disable_worker_transport
run_launchctl bootout "$domain/$other" >/dev/null 2>&1 || true
rm -f "$agents_dir/$other.plist"
[ "$role" != central ] || rm -f "$agents_dir/$tunnel_label.plist"
if [ -f "$plist" ]; then
  run_launchctl bootout "$domain/$label" >/dev/null 2>&1 || true
  bootstrap_agent "$plist"
fi
gateway_plist="$agents_dir/$gateway_label.plist"
if [ "$role" = worker ]; then
  run_launchctl bootout "$domain/$gateway_label" >/dev/null 2>&1 || true
elif [ -f "$gateway_plist" ]; then
  run_launchctl bootout "$domain/$gateway_label" >/dev/null 2>&1 || true
  bootstrap_agent "$gateway_plist"
fi
[ "$role" != worker ] || configure_worker_transport
[ "$role" != worker ] || register_worker
