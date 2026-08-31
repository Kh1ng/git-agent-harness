#!/usr/bin/env bash
# Deterministic first install for a GAH worker host on macOS.
#
# macOS has no systemd, so this is a genuinely separate script from
# scripts/install-linux.sh, not that script with the systemd calls patched
# out. Consequences that follow from that:
#
#   - Worker-only. The control-plane server (apps/server / gah-server.service)
#     and gah-loop@.service are both systemd-unit-shaped in this repo today;
#     there is no launchd equivalent implemented yet, so this script never
#     builds or starts either. It installs the `gah` CLI and (optionally)
#     wires it to a gateway; running `gah loop`/`gah dispatch` is left to the
#     operator (foreground, tmux, or your own launchd plist).
#   - No sudo, no /etc -- everything lives under $HOME.
#   - Gateway config goes in ~/.config/gah/gah-loop.env, the same file
#     gah-loop@.service reads on Linux via EnvironmentFile=. Nothing on this
#     host auto-loads it (no systemd), so it must be sourced into the shell
#     before running gah -- see the printed instructions at the end.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "ERROR: scripts/install-macos.sh is for macOS only (uname -s reported $(uname -s)). Use scripts/install-linux.sh, or scripts/install.sh to auto-detect." >&2
  exit 1
fi

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Same Rust update implementation Linux uses. Role is always "worker" here --
# see the file header for why macOS never runs the control-plane server.
# --bin gah is required: Cargo.toml declares a second [[bin]]
# (generate-cli-capabilities) with no default-run set, so a bare `cargo run`
# is ambiguous and errors instead of picking one.
cargo run --bin gah -- update --repo "$repo_root" --role worker

gateway_env_file="$HOME/.config/gah/gah-loop.env"

upsert_env_line() {
  local file="$1" key="$2" value="$3"
  install -d -m 0755 "$(dirname "$file")"
  if [ ! -f "$file" ]; then
    install -m 0644 /dev/null "$file"
  fi
  if grep -q "^${key}=" "$file" 2>/dev/null; then
    sed -i '' "s|^${key}=.*|${key}=${value}|" "$file"
  else
    printf '%s=%s\n' "$key" "$value" >>"$file"
  fi
}

# Memory gateway placement (issue #880, extended to macOS 2026-08-09).
# Opt-in: GAH_GATEWAY_MODE unset (the default) skips this whole section.
case "${GAH_GATEWAY_MODE:-}" in
  remote)
    # gateway-url-default:start -- extracted verbatim by
    # tests/source_structure.rs::install_scripts_default_gateway_url_to_central_host.
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
    upsert_env_line "$gateway_env_file" TDAI_GATEWAY_URL "$GAH_GATEWAY_URL"
    if [ -n "${GAH_GATEWAY_API_KEY:-}" ]; then
      upsert_env_line "$gateway_env_file" TDAI_GATEWAY_API_KEY "$GAH_GATEWAY_API_KEY"
    fi
    echo "Remote gateway confirmed reachable; wired into $gateway_env_file"
    ;;
  colocated)
    echo "ERROR: GAH_GATEWAY_MODE=colocated is not supported on macOS yet -- it depends on a systemd user unit (packaging/systemd/tdai-memory-gateway.service) with no launchd equivalent built. Use GAH_GATEWAY_MODE=remote against a Linux-hosted gateway instead." >&2
    exit 1
    ;;
  "")
    ;;
  *)
    echo "ERROR: unknown GAH_GATEWAY_MODE='$GAH_GATEWAY_MODE' (expected 'remote' or unset on macOS)" >&2
    exit 1
    ;;
esac

# Existing-project context import (opt-in, issue seen 2026-08-08). Set
# GAH_IMPORT_REPO to seed a project's handoff/memory docs into the gateway
# at install time; unset skips this entirely.
if [ -n "${GAH_IMPORT_REPO:-}" ]; then
  : "${GAH_IMPORT_SESSION_KEY:?GAH_IMPORT_REPO requires GAH_IMPORT_SESSION_KEY (e.g. gah:manager:github.com/org/repo)}"
  echo "Importing project context from $GAH_IMPORT_REPO under $GAH_IMPORT_SESSION_KEY..."
  python3 "$repo_root/scripts/backfill_project_docs.py" \
    --repo "$GAH_IMPORT_REPO" \
    --session-key "$GAH_IMPORT_SESSION_KEY" \
    ${GAH_IMPORT_DOCS:+--docs "$GAH_IMPORT_DOCS"}
fi

echo "GAH installed. Update with: gah update --repo $repo_root --role worker"
if [ -f "$gateway_env_file" ]; then
  echo "Gateway config is in $gateway_env_file. Nothing auto-loads it (no systemd on macOS) --"
  echo "export it into your shell before running gah, e.g.:"
  echo "  set -a; source $gateway_env_file; set +a; gah loop --profile <profile>"
fi
