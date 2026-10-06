#!/usr/bin/env bash
# Build the release server bundle consumed by `gah update --from-release`
# (issue #1416). Runs nothing itself: build the workspace outputs first
# (`npm run build:server && npm run build:web`; the MCP server is a Rust
# binary shipped as its own release asset since #1436),
# then this script validates them and archives the exact tree the release
# install path extracts over a checkout:
#
#   apps/server/dist          the node control-plane server
#   apps/web/dist            the dashboard
#   packaging/opencode/agents  the agent configs copied to ~/.config on update
#   package.json/package-lock.json  dependency-drift detection on install
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:?usage: build-release-bundle.sh <output.tar.gz>}"

expected=(
  apps/server/dist/bin.js
  apps/web/dist/index.html
  packaging/opencode/agents/gah-reviewer.md
  packaging/opencode/agents/gah-implementer.md
  package.json
  package-lock.json
)
for path in "${expected[@]}"; do
  if [ ! -f "$root/$path" ]; then
    echo "missing build output: $path" >&2
    echo "run: npm run build:server && npm run build:web" >&2
    exit 1
  fi
done

mkdir -p "$(dirname "$out")"
tar -czf "$out" -C "$root" \
  apps/server/dist \
  apps/web/dist \
  packaging/opencode/agents \
  package.json \
  package-lock.json
echo "wrote $out"
