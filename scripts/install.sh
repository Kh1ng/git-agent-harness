#!/usr/bin/env bash
# Deterministic first install for a GAH CLI/control-plane host.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Fresh installs and routine upgrades use the same Rust update implementation.
cargo run -- update --repo "$repo_root"

sudo install -m 0644 packaging/systemd/gah-server.service /etc/systemd/system/gah-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now gah-server.service
sudo systemctl is-active --quiet gah-server.service

echo "GAH installed. Update with: gah update --repo $repo_root --restart-server"
