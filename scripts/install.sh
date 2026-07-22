#!/usr/bin/env bash
# Deterministic first install for a GAH CLI/control-plane host.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Fresh installs and routine upgrades use the same Rust update implementation.
cargo run -- update --repo "$repo_root"

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

sudo install -m 0644 packaging/systemd/gah-server.service /etc/systemd/system/gah-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now gah-server.service
sudo systemctl is-active --quiet gah-server.service

echo "GAH installed. Update with: gah update --repo $repo_root --restart-server"
