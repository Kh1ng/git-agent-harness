#!/bin/bash
tmp=$(mktemp -d)
echo '#!/bin/sh' > "$tmp/tailscale"
echo '#!/bin/sh' > "$tmp/sudo"
chmod +x "$tmp/tailscale" "$tmp/sudo"
role=standalone
PATH=$tmp:$PATH
if [ "$role" != "standalone" ]; then
  if command -v tailscale >/dev/null 2>&1; then
    sudo tailscale set --accept-dns=true
  else
    echo 'WARNING: Tailscale is not installed; join the tailnet, then run tailscale set --accept-dns=true.' >&2
  fi
fi
rm -rf "$tmp"
