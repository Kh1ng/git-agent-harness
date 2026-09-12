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
#     builds or starts either. It installs the `gah` CLI and
#     configures central memory access; running `gah loop`/`gah dispatch` is left to the
#     operator (foreground, tmux, or your own launchd plist).
#   - No sudo, no /etc -- everything lives under $HOME.
#   - Coordinator credentials go in ~/.config/gah/gah-loop.env, the same file
#     gah-loop@.service reads on Linux via EnvironmentFile=. Nothing on this
#     host auto-loads it (no systemd), so it must be sourced into the shell
#     before running gah -- see the printed instructions at the end.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "ERROR: scripts/install-macos.sh is for macOS only (uname -s reported $(uname -s)). Use scripts/install-linux.sh, or scripts/install.sh to auto-detect." >&2
  exit 1
fi

if [ "${GAH_NODE_ROLE:-worker}" != worker ]; then
  echo 'ERROR: macOS central service installation is not available yet; use a central Linux host.' >&2
  exit 1
fi

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Same Rust update implementation Linux uses. Role is always "worker" here --
# see the file header for why macOS never runs the control-plane server.
# --bin gah is required: Cargo.toml declares a second [[bin]]
# (generate-cli-capabilities) with no default-run set, so a bare `cargo run`
# is ambiguous and errors instead of picking one.
bash "$repo_root/scripts/configure-node-role.sh" worker cargo run --bin gah --
cargo run --bin gah -- update --repo "$repo_root" --role worker

if command -v tailscale >/dev/null 2>&1; then
  tailscale set --accept-dns=true
else
  echo 'WARNING: Tailscale is not installed; join the tailnet, then enable Use Tailscale DNS settings.' >&2
fi

gateway_env_file="$HOME/.config/gah/gah-loop.env"

echo "GAH installed. Update with: gah update --repo $repo_root --role worker"
if [ -f "$gateway_env_file" ]; then
  echo "Worker credentials are in $gateway_env_file. Nothing auto-loads it (no systemd on macOS) --"
  echo "export it into your shell before running gah, e.g.:"
  echo "  set -a; source $gateway_env_file; set +a; gah loop --profile <profile>"
fi
