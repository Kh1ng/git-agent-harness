#!/bin/bash
tmp=$(mktemp -d)
echo '#!/bin/sh' > "$tmp/gah"
echo '[ "$1" = tailscale-ip ] && echo 100.118.97.79' >> "$tmp/gah"
chmod +x "$tmp/gah"
PATH=$tmp:$PATH
unset GAH_SERVER_HOST
role=central
    server_host="${GAH_SERVER_HOST:-}"
    if [ -z "$server_host" ]; then
      if [ "$role" = "standalone" ]; then
        server_host=127.0.0.1
      else
        server_host="$(gah tailscale-ip 2>/dev/null || true)"
        if [ -z "$server_host" ]; then server_host=127.0.0.1; fi
      fi
    fi
printf '%s' "$server_host"
rm -rf "$tmp"
