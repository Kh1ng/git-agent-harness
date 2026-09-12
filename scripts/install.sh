#!/usr/bin/env bash
# OS-detecting entrypoint. The actual install logic lives in two genuinely
# separate scripts. install-linux.sh uses systemd. install-macos.sh uses
# launchd and does not use sudo. Patching one Linux-shaped script to skip
# steps on macOS produced a script that looked like it worked on both and
# didn't (2026-08-08/09). Run the OS-specific script directly if you know
# which one you want; this just picks for you.
set -euo pipefail

dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

case "$(uname -s)" in
  Linux)
    exec "$dir/install-linux.sh" "$@"
    ;;
  Darwin)
    exec "$dir/install-macos.sh" "$@"
    ;;
  *)
    echo "ERROR: unsupported OS '$(uname -s)'. Run scripts/install-linux.sh or scripts/install-macos.sh directly if one applies, or file an issue." >&2
    exit 1
    ;;
esac
