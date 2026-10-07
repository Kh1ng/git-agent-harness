#!/usr/bin/env bash
# Persist the installer's role and worker credential before services can restart.
# Remaining arguments select the current CLI (including cargo run on a fresh host).
set -euo pipefail
role="${1:?Missing node role}"
shift
if [ "$role" = worker ]; then
  if [ -n "${GAH_GATEWAY_MODE:-}" ]; then
    echo 'ERROR: workers use the central memory relay. Configure GAH_CENTRAL_URL and COORDINATOR_TOKEN; configure GAH_GATEWAY_MODE on central only.' >&2
    exit 1
  fi
  if [ -n "${GAH_IMPORT_REPO:-}" ]; then
    echo 'ERROR: import shared project memory on the central node.' >&2
    exit 1
  fi
  credentials="$HOME/.config/gah/gah-loop.env"
  if [ -n "${COORDINATOR_TOKEN:-}" ]; then
    # The token travels on stdin; the file is written with mode 0600.
    printf '%s' "$COORDINATOR_TOKEN" | "$@" installer env-set --file "$credentials" COORDINATOR_TOKEN
  elif "$@" installer env-has --file "$credentials" COORDINATOR_TOKEN 2>/dev/null; then
    chmod 600 "$credentials"
  else
    echo 'ERROR: worker installation requires COORDINATOR_TOKEN for central claims and memory.' >&2
    exit 1
  fi
fi
args=(config set --node-role "$role")
# Existing configs retain legacy factory behavior. Fresh standalone apps opt out.
# macOS has no standalone role: its desktop installs central with GAH_STANDALONE=1.
factory="${GAH_FACTORY_ENABLED:-}"
standalone="${GAH_STANDALONE:-}"
[ "$role" != standalone ] || standalone=1
if [ -z "$factory" ] && [ "$standalone" = 1 ] && [ ! -f "${GAH_CONFIG:-$HOME/.config/gah/config.toml}" ]; then
  factory=false
fi
if [ -n "$factory" ]; then args+=(--factory-enabled "$factory"); fi
if [ -n "${GAH_CENTRAL_URL:-}" ]; then args+=(--registry-central-url "$GAH_CENTRAL_URL"); fi
"$@" "${args[@]}"
